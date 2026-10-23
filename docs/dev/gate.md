# The gate and sign-in

Every box runs the gate. On the gateway box it is the front door: nginx
asks it about every request to a protected service, and it serves the
sign-in pages, the home page and the pages behind the bar's menu. On
every other box it keeps the box's copy of the directory of people, and
little else.

The code is [servers/gate](../../servers/gate) (about 10,000 lines of
Rust); its NixOS module is
[nix/modules/gate/verify.nix](../../nix/modules/gate/verify.nix). The
crate, the binary, the systemd unit and its user still carry the old
name: crate `verify`, binary `verify`, unit `dd-verify.service`, user
`dd-verify`, state in `/var/lib/dd-verify`, environment variables
`VERIFY_*`. This page says "the gate" for all of them.

## What it is for

Two questions, answered on the box itself, with no identity server to
ask:

1. **Who is this?** A request carries one of two credentials and nothing
   else counts: a biscuit token signed by one of the devices in the
   person's own signed entry, or this box's session cookie from a passkey
   sign-in.
2. **May they come in here?** Being someone is not being let in. A
   person is a member when the release's member list names their root, or
   their entry carries an invite the release key signed. The demo account
   and guests (people a member brought in through a friend link) get less.

The gate holds no key that can sign as anyone. It checks signatures
against public keys in the directory, and issues a cookie that only this
box accepts. The comment at the top of
[lib.rs](../../servers/gate/src/lib.rs) puts it this way: nothing here
can sign as anyone, so nothing here is worth taking.

## A request's path

```
browser / app / dd
   │  https://git.<domain>/david/repo     (cookie dd_session, or Authorization: Bearer <biscuit>)
   ▼
Cloudflare tunnel ─┐   only for the public names (home, accounts, headscale,
   (outsiders)     │   demo-<service>, the bare domain): modules/gate/public.nix
fleet network ─────┤
                   ▼
nginx  ─ $dd_shut? ── outside connection to a private name: 444, closed
   │   ─ limit_req dd_user / dd_visitor (flood limits)
   │
   │  location with `auth_request /_dd/verify`
   │      │
   │      │  subrequest GET /verify  (no body; X-Original-URI, -Method, -Host)
   │      ▼
   │   gate (127.0.0.1:4181)
   │      identify: biscuit against the entry's devices, or the cookie's HMAC
   │      role:     member / demo / guest / none
   │      allowed:  member anywhere but a demo door; demo by the tile's
   │                allowance; guest on games.<domain> only
   │      ◄── 200 + X-Auth-Request-User, X-Auth-Request-Preferred-Username, X-DD-Role
   │      ◄── 401 nobody (403 on a demo door)    ◄── 403 known but not allowed
   │
   │  401 → @login: 302 /_dd/login?rd=<uri>
   │  403 → @waiting: 302 https://home.<domain>/
   │  200 → proxy to the service, with the user header set by nginx
   │        and the dd_session cookie taken out (strip-cookie.nix)
   ▼
service (Forgejo, llama-swap, the games manager, WebDAV, ...)
```

Paths under `/_dd/` on every gated host go to the gate itself, not
through `auth_request`: the gate checks the caller inside each handler.

```
https://<host>.<domain>/_dd/...  ──nginx /_dd/──►  gate :4181/_dd/...
                                                  (handler decides who)
```

## Two roles

`dd.verify.role` (env `VERIFY_ROLE`) picks one:

```
full        the gateway (roles/gateway.nix). auth_request, sign-in, every
            page, every service handler, and the directory. Needs a domain,
            the pages, the member list.
directory   every other box (roles/core.nix sets it as the default). The
            directory, and the house's Wi-Fi relay and status. No domain, no
            sessions, no secrets. Routes: /health, /_dd/directory*,
            /_dd/invite/*, /_dd/house/here, /_dd/house/wifi/relay.
```

`start` in lib.rs builds the directory first for both, then returns early
with the small router when there is no domain.

## Who is asking

### Credentials

`App::identify` ([lib.rs](../../servers/gate/src/lib.rs)) is the one
place a request becomes a name:

- **A biscuit** in `Authorization: Bearer`. The user is read from the
  token's authority block (`user("…")`, `peek_user`), the user's entry is
  looked up, and the token is checked against each device key in it.
  The authorizer gets `time`, `operation` and `here` as facts. `here` is
  the name the request was asked under (`X-Original-Host`, else `Host`)
  only when that is one of the box's own names (`Directory::here`: the
  domain, `home.<domain>`, `VERIFY_HOSTS`, `VERIFY_NAMES`), and empty
  otherwise: a header picks among the configured names and never adds
  one. nginx sets `X-Original-Host $host` on every gate location
  (strip-cookie.nix). Two policies: deny if the token names an
  `audience` other than `here`, then allow if the user matches and, for
  any operation but `access`, the token's `purpose` is that operation.
  So an enrol link cannot read a file and an access token cannot enrol a
  passkey. The datalog gets 200 ms. A device key's use is noted in the
  sign-ins registry.
- **The session cookie** `dd_session`
  ([session.rs](../../servers/gate/src/session.rs)). Its value is
  `<user>~<random id>.<expiry>.<hmac>`, HMAC-SHA256 with a 32-byte secret
  made on first start (`session.secret`). It lives 8 hours, is set for
  `Domain=<domain>`, and is `Secure; HttpOnly; SameSite=Lax`. A cookie
  from before session ids (`<user>.<exp>.<mac>`) still verifies.

A session is live only if it has not been ended from the Devices tab,
and, if a device signed it in (the app), that device is still in the
person's entry (`session_live`). A session whose device is gone is ended
as soon as it is seen.

### Roles

`App::role` turns a name into one of three, or none:

```
Member  some root the person has had (the root, and those it was
        recovered from) on the member list by member_id and none revoked,
        or the entry carries a grant (an invite) the release key signed
        and none is revoked (identity::is_member).
        With no member list at all (tests), everyone is a member.
Demo    the account named "demo", when any home tile grants the demo
        something (pages::Service.demo).
Guest   came in through a friend link, is not a member, and some member
        is still their friend (friends.rs).
none    signed in but not let in: a held sign-up, a revoked member, a
        guest with no member friends left. They see the waiting page.
```

`member_id` is the SHA-256 of the root, so the member list
(`fleet/members.json`, `{"members": [...], "revoked": [...]}` or a bare
list) carries no names in the clear: `names` beside them holds a hash of
each id with its name (`identity::name_tag`), which the gate does not
read and clients hold entries to ([client.md](client.md#identity-and-the-directory)).

**A recovery keeps membership.** Recovering with the paper key installs a
new root, so the root's id is no longer the one on the list. The entry it
signs names the roots it replaced (`recovered_from`, oldest first), and
membership, the invite it was let in by, and a removal all follow every
root in it. No box can forge one: `identity::accept` takes a new root only
co-signed by the recovery key the entry on file names, and holds
`recovered_from` to exactly the earlier list plus the root on file; a
first sight of a name carries none. Every box replays every entry of the
agreed log through that rule, so what it holds is the chain the fleet
agreed. A removal of any of those roots shuts the person out, recovered
or not. The mail Worker counts earlier roots only as far as it held them
to the entry it pinned before.

A client cannot replay the log, so each box keeps every recovery it takes
(`Directory::keep_handover`: the entry on file and the one that replaced
it, the newest 32, `recoveries/<name>.json` beside `keys/`) and serves
them for the person's own recovery to follow
([client.md](client.md#identity-and-the-directory), `identity::follows`). A box that replays the log from the start keeps
them all; one that had replayed past a recovery before this was kept has
none for it.

### The answer to nginx

`verify()` in lib.rs:

- no credential: 401, or 403 on a `demo-*` host (a 401 there would let
  some services treat the visitor as anonymous);
- Member: allowed anywhere except a demo door;
- Demo: `demo_allows` (below), and never `metrics.<domain>`: the fleet's
  figures are not the internet's, so the demo has no Metrics page, query
  or menu line whatever a tile says;
- Guest: only `games.<domain>`, never a demo door.

Disk images are not behind `auth_request`: nginx sends
`files.<domain>/images/` to the gate's `/_dd/images`. There, a browser's
change needs a passkey shown in the last five minutes. Anything but GET,
HEAD, OPTIONS or PROPFIND counts as a change, and so does a request
carrying `x-dd-upload` (`images` in library.rs). The older check in
`verify()` (`deletes_image`) is still there, but nginx no longer asks
`verify()` about `/images/`.

On 200 it sets `X-Auth-Request-User`, `X-Auth-Request-Preferred-Username`
and `X-DD-Role`. Services read the name through
`auth_request_set ... $upstream_http_x_auth_request_preferred_username`.

## Sign-in and sessions

### Once for every box: the id Worker

With `dd id setup` done (fleet/id-worker.json, `dd.idWorker`, `VERIFY_ID`),
sign-in is not any box's. It is the id Worker's
([servers/system/id](../../servers/system/id)), at `id.<domain>`: static
pages from the release, no secret, nothing stored. Whichever box served the
sign-in page could ask the passkey for a sign-in good on every other box,
so none does. The identity side is
[client/identity/src/session.rs](../../client/identity/src/session.rs); the
gate's is [sso.rs](../../servers/gate/src/sso.rs).

```
a box, no session for this name
  /_dd/login?rd= ─► /_dd/sso/start?rd=   a one-off challenge for this name
  ─► https://id.<domain>/go?host=<this name>&nonce=<challenge>

the id page (id.js)
  signed in here already? (IndexedDB on id.<domain>: a P-256 key the page
  cannot read out, and the passkey's delegation to it)
    no ─► username ─► GET https://<host>/_dd/sso/creds?user= (that box's
          copy of the passkeys) ─► make the key ─► the passkey signs
          sha256("commonty session v1\0" user \0 spki \0 sid \0 iat \0 exp)
          on this page (8 hours)
  the key signs "commonty handoff v1\0" <host> \0 <nonce>
  ─► POST https://<host>/_dd/sso {d, sig, nonce}   (to that name, never a
     place a parameter says)

the box (sso.rs)
  Origin is the id page; the challenge is one it gave this name, unused,
  under 5 minutes; the delegation checks against the passkey in ITS OWN
  copy of the directory (identity::check_login, origin = the id page),
  unexpired, not longer than 8 hours; the key signed this name and this
  challenge; the sign-in not ended here
  ─► __Host-dd: a cookie for this name alone, signed by this box's own
     secret, carrying the sign-in's id; ─► rd
```

The box's own id for the session is made from the person and the
sign-in's id (`sso::session_id`), never the browser's choice alone. With
sessions per name, only `__Host-dd` is read: the domain-wide `dd_session`
from before is not, since any name of the domain can set one in a browser
(its owner's own session, to sign the browser in as them). Signing out
clears both.

**Metrics** are a box's owner's (`box.json` owner, `VERIFY_OWNER`) and the
admin's (`policy.json` owner, `VERIFY_ADMIN`): `metrics.<domain>` and a
box's own `metrics.<box>.<domain>` answer nobody else. **A box a member
set up** (docs/dev/joining.md) is that member's and the admin's, who sees
every box: its series are at `metrics.<domain>/joined/<its name>/api/v1/...`,
which `/verify` lets through only for the admin and the member the gates
name as its owner (`member_boxes::sees_metrics`), and only for a path
already in the one
form nginx and the gate both read (no escapes, dot segments or doubled
slashes). **A box that
serves its own pages alone** runs the gate as role `box`: `/verify` for its
owner and the admin, the sign-in hand-off, `/_dd/back`, and the directory
and house routes - no member pages, sign-up or demo.

What a box receives works nowhere else: the hand-off names its host and
its challenge, and the cookie is this box's and this name's. That is why
sessions are per name with sign-in at the Worker: a cookie for the whole
domain is sent to every box's names, and any one of them could replay it.
Each name of a box makes the trip once, with no prompt. The domain-wide
`dd_session` from before is still read until it runs out.

A passkey shown to confirm something (the Devices tab, deleting a disk
image) is still this box's own ceremony below; in a session that is
already this person's, it marks that session fresh and keeps it. Sign out
(`/_dd/logout`) ends this box's session and sends the browser to
`id.<domain>/out`, which forgets the key there. Checked by `checks.sso`:
the real gate and Worker in a browser at their https names, the passkey
answered by the test member.

### A box's own sign-in

Passkeys are WebAuthn (webauthn-rs). The relying party is the bare
domain, so one passkey covers every host. Only this box's own pages may
run a ceremony: the allowed origins are `https://<h>.<domain>` for each
of `VERIFY_HOSTS`, plus `https://home.<domain>` (`own_hosts`). A page on
another host could be another box relaying this box's challenge.

Each ceremony's state is kept in memory for 300 s under a random id; the
browser sends it back in `x-dd-ceremony`.

```
sign in      POST /_dd/login/start {username}
               → the passkeys in that name's entry, as a challenge
             POST /_dd/login/finish (assertion)
               → checked; the old session in this browser is ended;
                 a new cookie; the passkey is "fresh" for 5 minutes

a browser   POST /_dd/join/start {username, agree} name free, not "demo",
account                                             not guest* (unless x-dd-probe);
                                                    400 unless agree: the page's
                                                    "I agree" to the terms
             POST /_dd/join/finish (new credential)
               → an entry with that passkey as root; the browser is asked
                 to sign the entry's hash with the same passkey. 400 for an
                 invite claim (x-dd-grant) or a library_key: a code is typed
                 only into the app or dd, and a passkey holds no keys
             POST /_dd/join/sign (assertion)
               → already on the member list: admitted to the directory.
                 Otherwise HELD on this box (below). Either way: a cookie.

a member     made in the app (or `dd identity new --code`): a root and a
             paper key made on the device, the invite redeemed there, the
             entry published to the directory like any other. No page of
             a box sees the code or a key.

enrol        the app or dd prints a link with a token for purpose "enrol"
             POST /_dd/enrol/start   (Bearer enrol token)
             POST /_dd/enrol/finish  the new passkey is kept in memory for
                                     10 minutes under sha256(token); never
                                     stored; 400 if a library_key comes with it
             GET  /_dd/enrol/result  dd collects it (204 until ready) and
                                     signs it into the entry with the root

the app      POST /_dd/app/signin (form: token)    Origin must be absent,
                                                    "null", commonty://localhost
                                                    or http://commonty.localhost;
                                                    a device token → a session
                                                    tied to that device; with
                                                    then=vault, back to the
                                                    app's own /_dd/app/vault
                                                    (the origin the form came
                                                    from), else /_dd/home
```

**Every sign-up agrees to the terms.** A new entry carries `terms`, the
date of the Terms of use it agreed to (`identity::TERMS`, which
`checks.terms-version` holds to what docs/policies/terms.md says), signed
with the rest: by the root on a device, or by the passkey's assertion in a
browser, where the gate puts it in the entry the person signs.
`PUT /_dd/directory/{name}` for a name this box has never seen, and a held
sign-up, refuse an entry made since sign-ups were asked
(`identity::TERMS_ASKED_FROM`) without it (`identity::agreed_terms`).
Updates, and entries from before, need none; the log's replay never asks.
The age question (month and year of birth; under 13 refused and the
refusal kept on that device; 13 to 17 with a parent's or guardian's
agreement) is the page's or `dd`'s alone ([age.js](../../servers/web-shared/age.js),
[consent.rs](../../client/cli/src/consent.rs)): the birth date never
reaches a box.

**A passkey signs in, and holds no keys.** Any page under the relying
party - any box's - can ask a passkey for its PRF secret, with any salt,
during an ordinary prompt. So nothing is ever sealed to or made from one:
library keys, the Photos password and the archive password are sealed to
device keys alone ([libraries.md](libraries.md#where-the-keys-live)).
`PUT /_dd/directory/{name}` and a held sign-up refuse an entry that still
carries a key for a passkey (`directory::no_passkey_keys`): a `passkey:`
seal, or a passkey's `library_key`. Entries from before still read and
move between boxes; their next version drops them.

**One sign-in.** The passkey sign-in here gives the browser the session
cookie every box page and the vault's single sign-on take. The pages that
hold a key are served by the member's app (in its window, or to a browser
it let in, [app.md](app.md#the-sites-key-pages-photos-and-passwords-in-a-browser)),
which asks the boxes with its device's token: so `/_dd/signins`,
`/_dd/signins/end` and `/_dd/devices/names` take a device token as well as
a cookie, and ending sessions with a token needs no passkey shown again.
Nothing the app serves asks the passkey for anything.

The signature counter is not written back after a sign-in: the
credential lives in the signed entry, which the gate cannot change. Clone
detection by counter is given up for that (comment in `login_finish`).

**Held sign-ups.** A browser sign-up with no code and not on the member
list is kept on this box only, in `held/`, never served or pulled. The
name counts as taken here and the person can sign in; they land on the
waiting page. It becomes an entry when a friend link is accepted (as a
guest), or when the member list names it: a task
checks every 300 s. Unfollowed, it is dropped after 7 days (`HELD_TTL`).
Names starting `guest` are probes' and last 15 minutes (`GUEST_TTL`).

**The sign-ins registry** ([signins.rs](../../servers/gate/src/signins.rs),
`signins.json`): every session by id (what it is, from the user agent;
first and last seen; expiry; the device that signed it in) and when each
device key and passkey was last used. Ending a session puts its id in
`ended` until the cookie would have run out. Writes are batched (at most
once a minute unless something new happened) and done after the lock is
let go.

**Ending sessions.** `POST /_dd/signins/end` ends one session or all the
others, and needs the passkey shown in the last five minutes. "All the
others" also writes the member's name into `VERIFY_ENDED` (`/run/dd-ended`),
which Passwords reads to end its own sessions. A new entry that drops a
device or passkey does the same (`Directory::commit`, `lost_a_key`).

**Signing out.** `POST /_dd/logout` ends the session, clears the cookie,
and goes by way of the photos host's `/_dd/photos/forget`
(`Clear-Site-Data: "storage"`) back to home. It takes a POST from our own
pages only (`from_our_site`: no `Sec-Fetch-Site: cross-site`, and an
`Origin`, if any, that is one of the fleet's names), so another site's
link or form cannot sign anyone out. `GET /_dd/logout` is a page with the
button; the bar's menu posts straight away (shell.js `posting`).

## The directory

[directory.rs](../../servers/gate/src/directory.rs). One signed entry per
person (`identity::SignedEntry`: name, root, recovery key, devices,
passkeys, an optional grant, libraries, version), stored as
`keys/<name>.json`. Entries hold only public keys, but together they
are who the members are, so reading them takes a credential.

### Who may read it

`Directory::may_read`:

```
GET /_dd/directory          a member (session cookie or device token). Nobody else.
GET /_dd/directory/{name}   a member; the person themselves (their session, even
                            before they are a member; their token; an enrol
                            link's token); a key in that entry, by a read proof
                            (its root, its paper key, one of its devices); a
                            reader key (the mail Worker's)
GET /_dd/directory/{name}/recoveries
                            the recoveries this box took for the name, oldest
                            first (identity::Handover); read as the entry is
everyone else               401, whether or not the name exists
GET /_dd/members            anyone: the signed member list from the releases
                            branch (VERIFY_MEMBER_LIST), as published, if it
                            verifies here (one fetch at a time, 1 MiB at most,
                            kept ten minutes); ids only, which the open repo's
                            fleet/members.json carries already
```

- **Device tokens for reads** are minted by `account::read_token`
  (`dd`, the app, git-remote-dd) with `purpose("directory-read")` and
  `audience(<the directory url's host>)`, for five minutes. No other
  route takes that purpose, and no other box takes that audience. The
  gate accepts an `access` or `enrol` token on these routes too (the
  app's pages, the enrol page watching its own entry); `read_operation`
  picks which by the token's purpose.
- **A read proof** is `x-dd-key-proof: <public key>.<unix time>.<sig>`,
  ed25519 over `dd-directory-read\n<name>\n<time>`, good for one name and
  five minutes either way (`identity::read_proof`). `dd` sends one by the
  root for its own entry, and recovery sends one by the paper key, so a
  machine that is no device yet still reads its own entry.
- **Reader keys** (`VERIFY_DIRECTORY_READERS`, verify.nix
  `directoryReaders`, from `reader` in fleet/mail.json): any single entry,
  never the list. The mail Worker is the one; it signs a read proof with
  `DIRECTORY_KEY` (`dd mail reader-key`).
- **Until a reader key is configured** a single entry is still anyone's
  (the Worker reads with nothing until it has its key); the list is
  members' from the start. The release that carries fleet/mail.json's
  `reader` closes single entries too.
- **Sign-in needs no entry in the browser**: `/_dd/login/start` reads the
  passkeys on the box and sends their ids with the challenge. That, the
  sign-up's "that name is taken" and a PUT's refusal say whether a name
  exists, as any sign-in by typed name must. nginx counts the directory
  and the ceremonies by address for a request with no credential
  (`dd_directory`, `dd_ceremony`: a second each, ten in hand).
- **Still readable by Cloudflare**: the tunnel ends TLS there, so it sees
  every entry a member's device reads, and the control plane's relay
  carries every directory item (names, public keys, passkey ids) in
  plaintext. README's TODO, "Nothing readable by Cloudflare".

### The rules

Every write, from any source, goes through the same checks:

```
identity::accept   signed by the root it names; an update is a strictly
                   newer version by the same root, or a new root co-signed
                   by the recovery key on file, which adds the root on file
                   to recovered_from; a first sight has none (client/identity)
check_grant        a grant: signed by the release key, proved by the root
                   (or one it was recovered from),
                   inside its window at the time it is judged, and not
                   already used by another root (one code, one person)
claims_free        a library id belongs to the first entry that named it
```

Invites (`identity::SignedInvite`) are kept in `invites/`, by their
public key, until they expire; only one the release key signs is kept.
`PUT /_dd/invite/{key}` takes one from `dd invite`; `GET` returns it so a
browser can prove a code.

### Agreed through the control plane

On real boxes the directory is the control plane's agreed log
([control-plane.md](control-plane.md), "The directory"). The gate is in
this mode when `VERIFY_CONTROL` is set (verify.nix sets it wherever
`dd.control.enable` is on).

```
write (PUT /_dd/directory/x, a join, PUT /_dd/invite/k)
  │  this gate's checks pass
  ▼
dd-control submit socket (/run/dd-control/submit.sock, POST /submit)
  │  {"entry": ...} | {"invite": ...} | {"ack": "<claim hash>"}
  ▼
validators order it (they judge none of it) ──► the agreed log
                                                    │
every gate, every second: GET <control>/directory?since=<next>
  │  replay each item at its agreed time through the rules above
  │    ack     counted if it came from a voting box
  │    invite  stored if the release key signed it and it was not expired then
  │    entry   name already in the log: an update, lands at once
  │            new name: a CLAIM; pending until every voting box's gate
  │            has acked that exact claim; this gate acks it if it is one
  │            of the voters and the claim is good
  │  settle: a fully acked claim becomes the entry; first in log order
  │          wins the name, the rest are refused
  ▼
replay.json  (next item, pending claims, acks, outcomes, migrated)
```

The voters are `validators` in `/etc/dd-control/config.json` that are
also in its `boxes` (the witness has no gate). This box's name is
`VERIFY_BOX` or the hostname.

A write waits up to 45 s for its own outcome. If the log has not agreed
by then it answers **202** "sent; not agreed yet". While the control
plane is down nobody signs up or changes an entry; sign-in and every
service keep working from the box's own copy.

**Migration.** On the first start in agreed mode the old `keys/` is
renamed `directory-before-log/`. Entries there are still served until
the log has them; `follow` submits each one (and every live invite) once.
When none is missing, `migrated` is set and the old copy is no longer
read.

### No peers

Boxes used to pull each other's directories (`VERIFY_PEERS`). That is
gone: the agreed log is the only way an entry reaches another box, and a
box without `VERIFY_CONTROL` (tests, a box on its own) holds just what
was published to it.

### Leaving

A member's own signed statement that they leave is a directory item too
(`Item::Leave`), replayed at its agreed time into `leaving.json`; someone
who left is no member from then on, and everything of theirs is deleted
at once. A coming back is no longer taken; old ones in the log still
replay. What happens
next, and when, is [leaving.md](leaving.md).

### Who reads it on the box

- The gate, on every request (entries are cached by file time and size).

### Who is who, read from the control plane

Being moved (docs/dev/identity-state.md): the control plane judges every
signed item once and the gate reads the result. `VERIFY_IDENTITY_FROM`
(`dd.verify.identityFrom`) says where the gate's answers come from:

- `replay` (the default for now): the gate's own replay of the log, as
  above;
- `shadow`: both; the replay's answer is served and every difference is
  logged, names and public fields only: an entry's
  (`identity shadow: <name>: <field>`), and each check's
  (`identity shadow: <name>: <check>: replay <answer>, agreed <answer>
  (agreed status <s>, access <a>, boxes: <box> <why>; ...)`, the same line
  once in ten minutes). The checks: `member` (the services), `house member`
  (a house's Wi-Fi on a directory-only box), `registered`, `suspended`,
  `left`, `sign-up let in`. The Workers log the same as one JSON line
  (`{"identity-shadow": <check>, name, own, agreed, why}`);
- `agreed`: dd-control's `/identity` alone (ident.rs). It is read in the
  background, `?since` answering 304 when nothing changed, so a request
  never waits on it; until the first read, it lets nobody in.

**Not known is not "no".** Letting someone in goes by what was last read:
until the first read nobody is let in, and after it, with dd-control
quiet, the last state still serves. Anything that cannot be undone goes
by more: the agreed state read, current (its agreed time within two
minutes of now, so not a dd-control still replaying the log) and recent
(read within 90 s) - `Directory::settled`, `ident::Agreed::fresh`. A
state never goes back to a lower height. Until it is settled the answer
about anyone is `Unknown` (`ident::Answer`, `App::member_answer`,
`App::network_answer`); settled, it is Yes or No, and a name the agreed
state does not have is No. On Unknown nothing is taken away and nothing
new is given: no network key (`/_dd/network/join`, `/controller`,
`/computer` answer 503) and no new session (sign-in, sign-up and the id
Worker's hand-off answer 503); what was given before goes on. Every
destructive path acts on a No alone:

- the network reaper takes a device off only on a No (below);
- `leaving::tick` ends and deletes nothing, and `/internal/leaving` and
  `/internal/leaving/done` answer 503, so photos, passwords, forge and
  games delete nothing either;
- a session a device signed in ends only when the person's entry is
  there without the device (`device_kept`), never for want of an entry;
- `/internal/storage/members` answers 503, so the ledger leaves every
  Photos limit as it is;
- a pending backup goes to the trash only on a fresh state.

dd-verify starts after dd-control and waits up to 30 s for it to answer,
then starts anyway: the code is safe either way.

**A PUT to the directory or leaving**, in order (directory.rs `read_put`,
`submit_envelope`):
1. the visitor's allowance, 20 a minute by `X-Dd-Peer` (nginx sets it from the
   connection, the tunnel's visitor on the public door) - before a byte is read;
2. the body, read only up to 64 KiB;
3. v1: as before. v2 (`/_dd/config` says `"envelope": 2` only where the control
   plane judges): of the path's kind and name, else 400;
4. not sent in the last day (sha256 of kind and body) - a list that cannot be
   read or written refuses every v2 PUT with 503 - and not among the agreed
   state's recent outcomes (its item hash; state not read yet: no answer);
5. signed by the key that speaks for the name (identity crate), else 403;
6. 6 a minute for the name and 6 for the signing key - one key may be the
   root of several names - counted only for items that verified;
7. remembered (file first, then memory), then sent to dd-control as it came: 202.

`GET /_dd/directory/<name>` still answers v1-shaped (`{entry, signature}`),
which the app's `devices.js` tells from an envelope by its `signature` and
no `v`. When the GETs answer envelopes instead - once every entry is v2 -
readers must take the entry from the envelope's base64 `body` and
`devices.js` must stop reading `.signature`; nothing on the boxes checks a
signature on a GET either way.

## Pages

The gate serves every signed-in page, and the HTML, CSS and JavaScript
are files, never strings in Rust ([ui.md](ui.md)).

- **Static pages and their files** are listed in
  [pages.rs](../../servers/gate/src/pages.rs): `STATIC` (what
  `/_dd/static/{name}` serves, with its path in the tree and its type),
  `PAGES` (home, files, media, friends, friend, chat, git, metrics,
  adblock, demo-photos, settings, send, add, docs) and the icons. They
  are read from `VERIFY_PAGES_DIR`, a directory the flake builds with the
  same layout as the repository (`servers/web-shared/...`,
  `servers/<service>/web/...`). All are read once at start; **a missing
  file stops the gate starting** rather than a page 404ing. A page edit is
  a new pages directory, not a new gate binary.
- **Templates** (askama) are the few pages that carry a value from the
  server: `login.html`, `join.html`, `enrol.html`, `waiting.html`,
  `download.html` in [servers/gate/templates](../../servers/gate/templates),
  and Photos' move page, `photos.html` in `servers/photos/templates`
  ([askama.toml](../../servers/gate/askama.toml)). Their scripts are in
  [servers/gate/web](../../servers/gate/web).
- **The wasm bundle** (client/web, built by the flake) is served from
  `VERIFY_WEB_DIR` at `/_dd/web/{file}`, `.js` and `.wasm` only.
- **Every static page fills itself from `/_dd/me`**: the name, role, the
  bar's menu and the tiles (`pages::me_json`), plus `tmdb` for members and
  the demo, `guestOf` for a guest, and the ad-blocking switch for the
  household. `no-store`. The app carries the same pages.
- **The site map** ([sitemap.rs](../../servers/gate/src/sitemap.rs)):
  `/_dd/sitemap.json`, for members and the demo, is every place they can
  open - the home tiles (`dd.home.services`, menu-only ones left out; the
  demo's own doors for the demo), Friends, Settings and its tabs read from
  `settings.html` itself, Git's tabs from `git-repo.js`, every view a
  service has an address for (`VIEWS`: every route of the Git pages, a
  test fails on one that is missing; a repository's views carry
  `{repo}`, filled in by the assistant, the fleet's own `repo` unless
  another is named), the tabs a page has without one (`SUBS`), and every
  docs page with its `##` headings from the docs' index - and `model`,
  Chat's `/v1/chat/completions`. The bare name answers it too, for the
  docs' own pages (roles/gateway.nix). Places only: nothing about who asked, no admin
  or internal route. Monty, the assistant in each page's corner
  ([guide.js](../../servers/web-shared/guide.js)), goes by it, and the
  docs draw it ([The site, drawn](../guide/site-map.md)). The model's
  answer is data: words shown as text, places as ids looked up in the
  map, anything else dropped (`checked`). It asks the model from the
  page's own name with the member's session on `llm.<domain>`; nginx lets
  the site's own names (home, files, git, games, metrics and their demo
  doors, and the bare name) read `/v1/models` and `/v1/chat/completions`
  there and nothing else ([llm.nix](../../nix/modules/llm/llm.nix)).
  Without a session there, the page goes through that name's sign-in and
  back (`/_dd/back`). The conversation follows the tab across the site's
  names in a frame of home it never shows, `/_dd/guide-keep`
  ([guide-keep.js](../../servers/web-shared/guide-keep.js)): that tab's
  sessionStorage on home, answered only to the same names (checked in the
  script, and in its `frame-ancestors`), never sent to a box. It looks up
  a member's own, read only and as them
  ([guide-look.js](../../servers/web-shared/guide-look.js)): their Git
  through the forge's API and a job's log, which the forge lets the same
  names read with a GET and nothing else (`$dd_forge_cors`,
  [forgejo.nix](../../nix/modules/forge/forgejo.nix)), and `/_dd/house`,
  `/_dd/fleet.json` and `/_dd/storage/mine`, which the bare name answers
  too. Nothing Locked (privacy.md); only a failed job's log goes to the model.
- `/_dd/config` gives `rpId` to any page, and the demo's library id and
  key to the demo.
- **Caching**: static files and pages are `no-cache` with an ETag (the
  first 12 bytes of their SHA-256); a browser that has them gets 304.
  Fonts are cached for a week.
- **The docs** ([docs.rs](../../servers/gate/src/docs.rs)): every `.md`
  under `docs/` in the pages tree, read once. `/_dd/docs/index.json` is
  the nav (from `docs/README.md`'s links), each page's title, headings and
  up to 20,000 characters of text, and `repo` (`VERIFY_REPO`) for edit
  links. `/_dd/docs/raw/{path}` is a page's markdown. Anyone may read
  them, on the bare domain at `/docs` too. This page is one of them.

The home tiles come from `dd.home.services`, which each role declares
(name, url, icon, colour, blurb, rank, `menuOnly`, `demo`, `demoUrl`);
verify.nix sorts them by rank and passes them as `VERIFY_HOME`.

## Headers, CSP and the cookie

[headers.nix](../../nix/modules/gate/headers.nix) adds to every nginx
location, and to every server for what no location takes (nginx's own
404), always (error answers too): HSTS, `nosniff`,
`Referrer-Policy: strict-origin-when-cross-origin`.
Locations that proxy to the gate (or the games manager) get the content
policy:

```
default-src 'self'
script-src  'self' 'wasm-unsafe-eval'
style-src   'self' 'unsafe-inline'
img-src     'self' data: blob: https:
media-src   'self' blob: https://*.<domain>
connect-src 'self' https://<domain> https://*.<domain> wss://*.<domain>
            https://api.themoviedb.org https://image.tmdb.org
font-src    'self' data:
frame-src   'self' blob: https://*.<domain>
worker-src  'self' blob:
object-src 'none'; base-uri 'self'; form-action 'self' https://*.<domain>
frame-ancestors 'self'; report-uri /_dd/csp
```

A location may set its whole policy instead (`ddCsp`): the forge's public
door does, stricter than this one ([forge.md](forge.md#the-public-door)).

No inline script anywhere. A browser's violation reports go to
`POST /_dd/csp`, which logs one line (the directive, the blocked url and
the page's path, without query strings; at most 8 KiB read). Movies &
TV's player reports its errors the same way, to `POST /_dd/player-error`:
one `player-error:` line with the host, the browser family ("Chrome/154
Android"), hls.js's error type and details or the video element's error
code, and the kind of film (size, container, codecs, picture size, the
last segment). Each field is kept only in the shape the player gives it
(an hls.js name, a codec string, a number, a known file ending) and
anything else is "-", so no film name, path or library reaches the log
([libraries.md](libraries.md#transcoding)). Vaultwarden
sends its own headers and gets only HSTS; other services get
`frame-ancestors 'self'`.

[strip-cookie.nix](../../nix/modules/gate/strip-cookie.nix): every
location that proxies anywhere but the gate forwards `Cookie` with
`dd_session` taken out (the `$dd_cookie_stripped` map in verify.nix), so
a compromised backend never holds a live session.

## The services

Grouped by what they do. Every handler checks its own caller; the route
table below says who.

### Photos ([photos.rs](../../servers/gate/src/photos.rs), [demo_photos.rs](../../servers/gate/src/demo_photos.rs))

Ente's client and the photo account's password are the member's app's
([app.md](app.md)); a browser at `/_dd/photos` gets the page that hands it
to the app (`use_app`). `/_dd/photos/config` (a session, or a device
token) gives the app's Photos page museum's address and the person's ente
address (`<user><VERIFY_PHOTOS_SUFFIX>`); the demo also gets its password.
The two museum calls that need the fleet's verification code
(`verify-email`, `change-email`) go through `/_dd/photos/museum/{op}`, by
session or device token: the gate checks the address is the caller's own
and adds the code, which never reaches a page.

**The move.** An account made before the app held its password was opened
with what a passkey's PRF made. Its member's app moves it once: it opens
a **handoff** slot (device token), sends the browser to
`/_dd/photos/move` (`photos-move.js`, the one page that asks a PRF, for
the old label `dd-photos` only), which fills the slot with that old
password sealed to a key only the app holds; the app takes it once and its
Photos page re-keys the account at once. At most 256 slots, 10 minutes
each, 1 KiB each.

The demo's photos are not Ente's app: the gate signs in to Ente as the
demo, lists and decrypts the demo account's photos, and serves a gallery
at `/_dd/photos/demo` (listing kept 10 minutes; originals over 25 MiB show
their thumbnail).

### Libraries and storage ([library.rs](../../servers/gate/src/library.rs), [storage.rs](../../servers/gate/src/storage.rs))

`/_dd/dav/{lib}/...` is WebDAV over each library's prefix in the
libraries bucket; the storage and WebDAV themselves are
[servers/library](../../servers/library) (crate `library-gate`). The gate
decides who: the id must be 32 hex characters; the caller (device token
or session) must be a member; the **owner** has it in their entry, a
**reader** is named in some owner's entry for it. The demo reads only the
box's demo library (`VERIFY_DEMO_LIBRARY_ID`), and nobody's entry may
claim that one. A daily task empties what has been in the trash for 30
days (an hour later if the bucket is not up).

`/_dd/library/{lib}/notes/{kind}` (GET, PUT) keeps what Movies & TV knows
about a library outside it, so a place saved every half minute does not
leave a file in the trash each time. The browser seals each note with the
library's key; the gate stores the bytes, replaced whole (tmp file and
rename), and cannot read them. The same owner-or-reader check as WebDAV,
members only (not the demo). `progress` is each viewer's own (512 KiB);
`shelf` is the library's: its owner writes it, its readers read it
(1 MiB). Any other kind is 400.

Every write by an owner (PUT, COPY, a piece of an upload) is held to the
member's allowance: room is set aside first, and the body is counted as
it streams and cut off with 507 past that room (`metered`). The allowance
and each member's use come from `VERIFY_STORAGE_LEDGER`, which
`dd storage-view` writes on every box from the fleet's heartbeats
([storage/view.nix](../../nix/modules/storage/view.nix)). The Photos box's
`dd storage-ledger` asks `GET /internal/storage/members` for each
member's library and disk-image bytes. `/_dd/storage/mine` is the Storage
tab's figure: the fleet-wide count.

`/_dd/images/...` is a member's disk images: the same WebDAV over the
prefix `images-<member>`, own images only, members only, a fresh passkey
for a browser's change.

### The network's door ([network.rs](../../servers/gate/src/network.rs))

With `VERIFY_HEADSCALE_API` set (the gateway, `dd.verify.network`):

- `POST /_dd/network/join`, device token: a single-use Headscale
  pre-auth key in the member's (or guest's) name, 10 minutes.
- `POST /_dd/network/box` `{box, time, sig}`: a box's own single-use
  key, tagged `tag:box`. The box signs
  `commonty box join v1\0<name>\0<time>` with the P-256 key in its TPM;
  the time must be within 120 s, once a minute per box, and each signed
  time only once. The key comes back age-encrypted to the box's TPM
  secrets key, so whoever carries the answer holds only ciphertext. The control box mints its own
  ([headscale.nix](../../nix/modules/net/headscale.nix)); there is no
  reusable box key.
- A box's owner reaches that box through headscale's ACL plus a per-box
  ipset, `dd-owner` ([owner.nix](../../nix/modules/net/owner.nix)).
- The fleet's names resolve publicly to the gateway's fleet address,
  100.64.0.2 (`address` in [fleet-names.nix](../../nix/lib/fleet-names.nix)).
- Every 300 s, devices of people who are no longer members or guests are
  deleted from Headscale. Tagged nodes (boxes) are left alone, and so is
  everything when the member list is empty (a broken release, not
  everyone revoked). A device comes off only on a known No: the person
  known to be no member (left, suspended, revoked, no access, or a name
  the settled state does not have), and not a guest a member still
  vouches for. While who is who is not known (agreed mode, the state not
  settled) the device stays and the log says `kept <device> - whether
  <name> is a member is not known`. Then the guard (`network::Guard`):
  nobody the release names (`VERIFY_PEOPLE`, the admin, the box owners)
  comes off, whatever they read as (`NOT removing`); nothing comes off
  until who is who has been known for 120 s without a break (a start, or
  a return after an outage; `NOT reaping`). A person known to be no
  member loses every device, however many. The brake (`network::Brake`)
  holds the state itself: if who has access has shrunk sharply since the
  last state the reaper acted on (none at all, or more than a quarter of
  them gone at once), or more than 2 people would be turned out in one
  pass, nothing comes off, the last good state is kept
  (`reap-acted.json`), and every pass logs `ALERT: NOT reaping, the brake
  is on` and sets `dd_reap_brake 1` in the box's facts, which
  dd-fleet-alerts mails. It comes off when the state is consistent again,
  or when the owner says that state is right: `GET
  /_dd/network/reap-brake` gives why and the refused state's hash, and
  `POST /_dd/network/reap-brake {"state": "<hash>"}` acknowledges it.
  Only the fleet's owner (the admin), by a device of theirs or a browser
  that showed the passkey in the last five minutes; being on the box is
  nothing. It is taken only while the brake is on, only for that exact
  state (who has access and who would be turned out): if the next pass
  sees another state the ack is void. It is spent on that pass and runs
  out after ten minutes. A state that gives nobody access is never reaped
  from, acknowledged or not (2026-10-22: a gate started before dd-control
  answered took the owner's laptop off for two hours).

  Nothing new is given while who is who is not known: every place the
  gate makes a session, an OIDC code or token, a network key, a friend
  link, a guest, or a device being added asks `App::no_new_session`
  first, and `every_issuer_asks_first` finds each such place in the
  source and fails on one that does not.

### Home controllers ([controller.rs](../../servers/gate/src/controller.rs))

A household's home controller is no member and holds no token; it signs
each request with its own key, carrying its pairing member's root's word
for its name, in `x-dd-controller`
([home-controller.md](home-controller.md#the-gate-for-a-controller)).
With it, the controller reads one person's entry
(`/_dd/controller/directory/<name>`, never the list) and, on the gate
holding the libraries bucket (`files.<domain>`, over the network), gets
presigned urls for its one backup, `controllers/<label>/backup`, for 10
minutes. The gate keeps label -> member and controller key in
`controller-backups.json` (first word holds the label; another is 409); a
member restores from their own labels alone, and leaving deletes them.

A passkey root names a controller on `/_dd/controller-name`
(`controller-name.html`): the page signs
`controller_name_message(<label>.home.<domain>, <key>)` with the root
passkey and leaves the answer here for the app, which collects it once
with a device token of the same member, within 10 minutes. Kept in memory
only.

### The house ([home.rs](../../servers/gate/src/home.rs), [adddevice.rs](../../servers/gate/src/adddevice.rs))

Settings' Network tab. `GET /_dd/house` lists this box and asks each house
peer's `/_dd/house/here` (4 s each). That answers on a directory box's
port 4181, which the firewall opens to the house's boxes alone; the full
gate answers it to its own box only (`this_box_only`), and nginx 404s it. A Wi-Fi change is signed in the
browser by the member's passkey over `commonty wifi v1\0` + the payload's
exact bytes (`{user, ssid, psk, at, nonce}`). Every box checks it against
the member's own entry and `VERIFY_HOUSE_ORIGINS`, refuses it if older
than 10 minutes or its nonce was seen, keeps the seen nonces in
`seen.json`, and writes the request into `VERIFY_HOUSE` (`/run/dd-wifi`)
for a root unit that tries it and falls back. The gate that took the
change relays it to the house peers on `:4181/_dd/house/wifi/relay`,
where each checks the signature for itself; nginx answers 404 to that
path from outside.

**Adding a device by QR code.** A signed-in member or guest starts a code
(`XXX-XXX` from an alphabet without 0/O/1/I/L, one at a time per person,
10 minutes) and shows `https://home.<domain>/add#<code>`. The new device
offers its public key and a commitment; the approving device sends its
random number; the new device reveals its own. Both screens show six
digits worked out on each device, so a box that swapped the key cannot
make them match. The gate only carries the messages; the account's root
signs the device into the entry as for any device.

### Friends and guests ([friends.rs](../../servers/gate/src/friends.rs))

A member makes a link (`/_dd/friend/<secret>`, a day, once; at most 20
waiting). Whoever opens it signed in becomes their friend both ways. A
non-member who accepts comes in as the maker's guest, and a held sign-up
is admitted to the directory then. Kept in `friends.json` on this box
only (links by the hash of their secret), never in the directory. The
secret is the link's path, so nginx keeps no access log for
`/_dd/friend/` and cuts a referrer at `/_dd/friend/`.

The games manager asks, on the gate's port directly:
`POST /internal/games/access` (for each server, who may play now and
their devices' network addresses) and `GET /internal/games/friends/{user}`.

### Ad blocking ([adblock.rs](../../servers/gate/src/adblock.rs))

The gate is Pi-hole's only door. It signs in to Pi-hole's api with a
password only it holds (`VERIFY_ADBLOCK_PASSWORD_FILE`) and lets the
household (`VERIFY_ADBLOCK_HOUSEHOLD`, member names) read totals and
lists, switch blocking on, off or pause it, and allow or unallow a site.
The household's on/off is remembered in `VERIFY_ADBLOCK_STATE` and put
back at start, since every release rewrites Pi-hole's settings.

### Fleet, Git and Metrics ([fleet.rs](../../servers/gate/src/fleet.rs), [forge_events.rs](../../servers/gate/src/forge_events.rs))

`/_dd/fleet.json` (members): each box in `VERIFY_FLEET` with its release,
the agent's last result and its last good backup. Read from dd-control's
agreed state (`/state`; a box unseen for 2 minutes is down), or from
Thanos on this box where there is no control plane. Kept 15 s.

`/_dd/git/events?repo=<id>` (members): Server-Sent Events of Forgejo's
run, job and status changes. A trigger in Forgejo's database announces
them on the Postgres channel `dd_forge` (modules/forge/live.nix); the
gate LISTENs as `dd-verify` (`VERIFY_FORGE_EVENTS`) and reconnects every
5 s when the database goes. Each announcement says whether its repository
is hidden from members (private, or its owner's profile private); the
gate passes on only those that say `"private": false`.

The Git and Metrics pages are served to anyone signed in; the forge's
API and the metrics queries (behind `auth_request`) decide what each sees.

### Status ([status.rs](../../servers/gate/src/status.rs))

`/_dd/status.json` (anyone): whether each service members use works now,
and each of the last 90 days. Uptime alone: no release, box or place in
it. Every box notes its own services each minute
([uptime.nix](../../nix/modules/observe/uptime.nix)): a service is up there
when every unit it needs on that box runs and, where it has one, its own
address on loopback answers short of a 5xx. A placed service (needs
`placed` or `lease`) is noted only on the box the plan runs it on, where
its flag `/run/dd-placed/<service>` is; elsewhere the box says nothing of
it. The note is `dd_service_up{service="<id>"}` in
`/var/lib/dd-facts/uptime.prom`, read by node_exporter like the other
facts. No box asks another.

The gate reads the notes from thanos on its own box (`VERIFY_THANOS`), by
service and box, and counts a box's note on a service only when the
release builds that box with it (`VERIFY_SERVICE_BOXES`,
`dd.verify.serviceBoxes`, which uptime.nix works out from
`nix/lib/fleet-placement.nix` builtWith): no box is trusted, and one not
built with it cannot make a service look up or down. A service noted on
two boxes is as up as the worse of them.
Now is the instant value (a service noted before and silent now is down);
when it changed is from the last six hours by the minute, else from the
quarter hours; each day is the share of its quarter hours up
(`avg_over_time(...[15m])`), a missing quarter hour counting as down,
except the three hours after a service's newest note, which another box's
block may still bring. Days are UTC days; a day before a service's first
note is `null`.

```
{"services": [{"id": "photos", "label": "Photos", "up": true,
               "since": <unix secs>, "days": [1.0, 0.99, null, ...]}],
 "checked": <unix secs>, "start": <the first day's midnight, UTC>}
```

The ids and labels are the gate's own list (`SERVICES`): site, files,
movies, photos, passwords, chat, games, git. A service a box names that is
not on it is dropped, and nothing of a box (its name, address, site, how
many there are) is in the answer, for members or anyone else. No thanos on
this box: 503. Mail forwarding is not on it: it runs at Cloudflare, and
nothing a box can see says whether it delivers.

The page is `/_dd/status` ([servers/status](../../servers/status/web)),
at `<domain>/status`, linked from every page's footer.

### Chat search

`GET /_dd/chat/search?q=` (members only): asks SearXNG on this box
(`VERIFY_SEARCH`), returns at most 6 results, each clipped. The model has
no network; the page hands it these.

### Passwords' sign-in ([oidc.rs](../../servers/gate/src/oidc.rs))

A per-box OpenID Connect issuer for Vaultwarden, the one client it knows
(`VERIFY_OIDC_*`, set by modules/vault/vaultwarden.nix). Members only.
ES256 id tokens from `oidc.key`, made on first start; codes live 5
minutes, access tokens an hour, both in memory. PKCE S256 when the client
sends a challenge. The email it vouches for is `<user>@<domain>`.

### Downloads

`/_dd/download` offers exactly the files a manifest signed by the release
key names (`VERIFY_APP_MANIFEST`, the releases branch's `app.json`), with
their hashes. A good manifest is kept 10 minutes, the lack of one 1
minute. Anything that does not verify is as good as absent.

## The zone: names and certificates

No box holds a token for the fleet's zone at Cloudflare. The token is a
secret of the DNS Worker
([servers/system/dns/worker.js](../../servers/system/dns/worker.js)), at
`dns.<domain>`, and nowhere else, so no box can point a name anywhere. The
Worker does two things, both from what the release says.

**The fleet's names.** `*` and the bare name at the gateway's address on
the fleet's network; each public host (and demo door) a proxied CNAME to
the tunnel while the door is open, and the bare name too when it is public
([fleet-names.nix](../../nix/lib/fleet-names.nix), from the gateway's own
config, built into the Worker as `RECORDS`). Every five minutes (a cron
trigger `dd dns deploy` sets) the zone is put in step: a name no longer
public loses its CNAME so the wildcard answers again. Only an A at the
gateway's address or a CNAME to the fleet's tunnel is ever changed or
removed; mail, TXT, CAA, another tunnel's names are never touched, and a
name with someone else's record is left alone. At the zone's own name,
mail's records stay beside the flattened CNAME. A name Cloudflare refuses
is reported and the rest go on. dd-dns did this from the gateway, with the
gateway's own token, until the Worker took over.

**Certificates.** A box's certificate is for the names its own nginx serves and no others
([https-names.nix](../../nix/lib/https-names.nix): every host on the
certificate and its aliases, under the domain). Let's Encrypt asks for a
TXT record at `_acme-challenge.<name>`, which only the Worker writes.

```
lego (acme-order-renew-<domain>, user acme)
  └─ exec: acme-exec.sh ─► /run/dd-acme.sock (acme's alone)
       └─ dd-acme@ (root): a name this box serves? ─► sign with the TPM
            └─ POST dns.<domain>/api/challenge {box, at, op, name, value, sig}
                 └─ Worker: signed by that box's key (BOXES), the name is
                    one the release says it serves (NAMES), within 5 min,
                    the value is a challenge ─► one TXT, written or removed
```

The message signed is `commonty acme v1\0<box>\0<at>\0<op>\0<name>\0<value>`
by the box's TPM key under its signed boot, as dd-control signs. The
Worker's `fleet.js` is built from the release (`.#dns-worker`): each
box's key from boxes.json and its names from its own evaluated nginx, so a
box gets a certificate for a new name only in the release that gives it
that name, and `dd release publish` uploads the Worker before it nudges
the boxes. A box that asks for anything else, or a box that is not in the
fleet, gets nothing written. What a box can do with its key is get a
certificate for the names it already serves.

**The switch** is `dd.dnsWorker` ([domain.nix](../../nix/modules/box/domain.nix)),
set when fleet/dns-worker.json exists. Until `dd dns setup` writes it, the
gateway keeps the names (dd-dns) and its wildcard with its own token
(`cloudflare-token`). With it, there is no dd-dns, the gateway is given no
zone token at all, each box asks for its own names, and node1's
certificate lists its 21. Then that token is deleted at Cloudflare.
Hourly, the gateway checks the Worker at Cloudflare is this release's
(`dd-dns-worker-check`, the mail Worker's check with `WORKER=dns`).

Checked by `checks.dns-worker` (the Worker against a fake Cloudflare: two
boxes' keys and the box's helper, refused and accepted; the names with the
door open and shut, beside someone else's records, and nothing written
once in step) and the `acme` VM test (pebble issues a box's certificate
through the whole path).

## Public names, the bare domain and the demo

The house line is carrier NAT. [public.nix](../../nix/modules/gate/public.nix)
opens a Cloudflare tunnel outward and lists which names answer outsiders.
On the gateway ([roles/gateway.nix](../../nix/roles/gateway.nix)) those
are `home`, `accounts`, `headscale`, the bare domain, and a
`demo-<service>` door for each tile that grants the demo something
(except Photos); and `code`, the forge's public door, which the forge adds
([door.nix](../../nix/modules/forge/door.nix), [forge.md](forge.md#the-public-door)). Every other name closes the connection (444) to anyone
not on the fleet network.

On a public gated host, the ceremonies a stranger may start
(`/_dd/(join|login|enrol|network|add)/`) are limited to one a
second per address, ten in hand; so are `/_dd/directory` and
`/_dd/invite` for a request that carries no credential at all (no
session, no token, no read proof).

**The bare domain** serves the front page at `/` to anyone, signed in or
not (`/_dd/landing`, [servers/landing](../../servers/landing/web)), sends
`/download` to the downloads page and any other address it does not serve
to the same path on `https://home.<domain>`, serves release files from the
forge as nobody under
`/download/vX.Y.Z/<file>`, and serves the docs (`/docs`, `/docs/...`)
with only the gate paths their page needs: `/_dd/static/`, `/_dd/web/`,
`/_dd/docs/`, `/_dd/me`, `/_dd/config`, `/_dd/csp`, and
`/_dd/releases.json` (Monty's what-is-new). And the uptime page,
`/status`, with its one answer `/_dd/status.json`.

It also serves `/.well-known/security.txt` (RFC 9116,
[security-txt.nix](../../nix/modules/gate/security-txt.nix)): Contact
`mailto:security@<domain>`, Preferred-Languages `en`, Canonical its own
URL, Policy `https://<domain>/docs/policies/security`, and Expires
`dd.securityTxtExpires`, a fixed date moved on by hand: CI fails 60 days
before it, or if it is over a year out. nix/tests/nginx.nix checks the
location is served and the fields (security-txt.py).

**Mail** is the mail Worker at Cloudflare (client/mail/worker), not the
gate. It reads a member's entry from `home.<domain>/_dd/directory/<name>`
with a read proof by its `DIRECTORY_KEY` (`dd mail reader-key`), whose
public half is `reader` in fleet/mail.json and so a reader key on every
box. Besides each member's `<name>@<domain>` rule it keeps the release's
aliases, `mailAliases` in fleet/policy.json (`{"security": "david"}`):
`<alias>@<domain>` forwards to that member's confirmed address (no rule
while it is unconfirmed), and a rule for an alias no longer listed is
removed. An alias's rule is named `Commonty alias <alias>`; only those
are touched, never a `Commonty member <name>` rule. An alias is
lowercase `a-z0-9-` and never a person's name: the build refuses one that
is the owner's or a person's in policy.json, and the Worker skips one
that is a member's address already (a `Commonty member <alias>` rule:
that member keeps it) and otherwise gives no member an address under an
alias's name. It keeps them in step on the
lists it reads anyway: a member's `/api/me` and the shared state each
minute.

**The demo** is one account, `demo`, that anyone can be:
`GET /_dd/demo` gives a fresh session (each click is its own session id)
and lands on the home page. Someone already signed in (but the demo) is
shown a page with the button instead, a `POST /_dd/demo` from our own
pages, so another site cannot swap their session for the demo's. It exists when any tile has a `demo`
allowance, and what it may do is decided per host, by the tile whose door
that host is (`demo_allowance`; `demoUrl` when the demo has its own door):

```
full       the service's own permissions are the limit
read       GET, HEAD, OPTIONS, PROPFIND only (and starting a film:
           POST /_dd/transcode/session)
rate:N     reads free; N other requests an hour for each demo session
rate:N/M   and M an hour for all demo sessions on that host together
(none)     nothing; the tile is greyed
```

The demo has no passkey, so its photos password and library key come
from the box (`VERIFY_PHOTOS_DEMO_FILE`, `VERIFY_DEMO_LIBRARY_*`). Neither
is secret: nothing private is in the demo's account.

**The forge's public door** (`code.<domain>`) is not a gated host: no
`/_dd/` but the Git page's files (`/_dd/static/`, `/_dd/web/`), the
public Git page (`/_dd/git/public`, served for any path the page draws),
and `/_dd/me`, which nginx answers 401 itself so the bar shows "Sign in".
No cookie and no `Authorization` reach the gate from there.

**A demo door** (`demo-<service>.<domain>`) is the service's own site
under a second, public name. `verify` lets only the demo through there,
and the `demo_door` middleware does the same for the gate's own paths on
that name: a member's session is sent to the service's real name, a
stranger is sent to start the demo, and only `/_dd/static/` and
`/_dd/web/` answer without a session.

## Limits

nginx (zones in verify.nix and public.nix; all answer 429):

```
dd_user      per credential (cookie or bearer)  30 r/s, burst 300   every /_dd/ location
dd_visitor   per address                        50 r/s, burst 500   /_dd/ and public servers
dd_ceremony  per address                         1 r/s, burst 10    public sign-in/up paths
dd_model     per credential                      1 r/s, burst 20    the model's api (llm.nix)
dd_conn      per address                        64 connections      public servers
dd_code*     per address, and all together      see forge.md        the forge's public door
public servers: client_max_body_size 1m, body and header timeouts 15 s
```

In the gate:

```
session cookie               8 h
passkey "fresh"              5 min (signins/end, changing a disk image)
ceremony state               5 min        pending enrol passkey   10 min
held sign-up                 7 days       guest* probe names      15 min
directory write waits        45 s, then 202
add-device code              10 min, one per person
friend link                  24 h, once, 20 waiting per member
photos handoff               10 min, 256 slots, 1 KiB
network join key             10 min, single use
box network key              once a minute per box, clock within 120 s
Wi-Fi change                 signed within 10 min; nonce kept 20 min
OIDC code / access token     5 min / 1 h
device names blob            64 KiB
CSP report                   8 KiB read, fields cut to 160 chars
player error report          4 KiB, 30 lines a minute, fields kept only in their shape
chat search                  query under 300 chars, 6 results, 12 s
biscuit authorizer           200 ms
app manifest                 kept 10 min (1 min when missing)
fleet.json                   kept 15 s
status.json                  worked out once a minute, its days every 10 min; public, max-age 30 s
```

## Routes

From the router in `start` ([lib.rs](../../servers/gate/src/lib.rs)) and
[directory.rs](../../servers/gate/src/directory.rs) `router`. "Session"
means the cookie; "token" a device biscuit; "member" excludes the demo
unless said. Paths under `/_dd/` reach the gate through nginx on every
gated host; `/verify`, `/health` and `/internal/` only from the box.
`/internal/*` and the full gate's `/_dd/house/here` also check it: from
loopback, and without the headers nginx adds to what it forwards.

```
path                                        method   what                                      who
/verify                                     GET      auth_request answer                       nginx (internal location /_dd/verify)
/health                                     GET      "ok"                                      local
-- sign-in
/_dd/login                                  GET      sign-in page                              anyone
/_dd/login/start, /finish                   POST     passkey sign-in, sets the cookie          anyone
/_dd/logout                                 GET      a page with the sign-out button           anyone
/_dd/use-app                                GET      hands photos. and vault. to the app       anyone
/_dd/logout                                 POST     end session, clear photo storage          our own pages
/_dd/join                                   GET      a browser account's page (no code)        anyone
/_dd/join/start, /finish, /sign             POST     account from a passkey, no code           anyone
/_dd/enrol                                  GET      enrol page                                anyone
/_dd/enrol/start, /finish                   POST     make a passkey for the app or dd          token with purpose "enrol"
/_dd/enrol/result                           GET      dd collects the passkey                   the same token
/_dd/app/signin                             POST     device token -> session                   the app's page; member
/_dd/demo                                   GET      a demo session; signed in: the button     anyone, if the demo is on
/_dd/demo                                   POST     a demo session                            our own pages, if the demo is on
-- pages and what every page needs
/_dd/home                                   GET      home, or waiting page                     session (else -> login)
/_dd/me                                     GET      who, role, menu, tiles                    session or token; 403 without a role
/_dd/sitemap.json                           GET      every place to open, Chat's model         member or demo; session or token
/_dd/back?to=                               GET      back to a page after this name's sign-in  anyone; https, our page names only
/_dd/guide-keep                             GET      Monty's conversation frame (no data)      anyone; framed by our page names only
/_dd/config                                 GET      rpId (+ demo library)                     anyone
/_dd/static/{file}                          GET      css, js, fonts, icons                     anyone
/_dd/web/{file}                             GET      wasm bundle                               anyone
/_dd/files, /_dd/media                      GET      the demo's Files, Movies & TV; a          member: the use-app page;
                                                     member's browser is handed to the app     demo if a demo library
/_dd/settings                               GET      Settings: a member's or guest's whole;    any session with an account here,
                                                     anyone else's, Leave alone                not the demo
/_dd/boxes, /backups, /devices,             GET      redirect to /_dd/settings#<tab>           anyone
  /network, /storage
/_dd/chat                                   GET      Chat (the demo's; a member's: use-app)    member or demo
/_dd/chat/search                            GET      web search for Chat                       member, session or token
/_dd/git                                    GET      Git                                       any session
/_dd/metrics                                GET      Metrics                                   member who owns a box (the release's,
                                                                                               or one they joined and enlisted); admin
/_dd/friends                                GET      Friends                                   member or guest
/_dd/adblock                                GET      Ad blocking                               household member, session or token
/_dd/add                                     GET      add-device page (QR target)               anyone
/_dd/send                                   GET      Passwords' Send page                      anyone
/_dd/download                               GET      app downloads                             anyone
/_dd/docs/page, /_dd/docs/page/{*}          GET      docs page                                 anyone
/_dd/git/public                             GET      Git, read-only (the forge's public door)  anyone
/_dd/docs/index.json, /_dd/docs/raw/{*}     GET      docs index, a page's markdown             anyone
/_dd/releases.json                          GET      the newest release notes, from the        anyone
                                                     public repository's releases
/_dd/csp                                    POST     CSP report, logged                        anyone
/_dd/player-error                           POST     Movies & TV's player error, logged        anyone
-- directory
/_dd/directory                              GET      names and versions                        member (cookie or token); else 401
/_dd/directory/{name}                       GET      an entry                                  member; the person; a key in it; a reader key; else 401 (anyone until a reader key is set)
/_dd/directory/{name}                       PUT      publish an entry                          anyone; the signature decides
/_dd/invite/{key}                           GET      an invite                                 anyone
/_dd/invite/{key}                           PUT      keep an invite                            anyone; release key must sign it
-- sessions and devices
/_dd/signins                                GET      sessions, key uses                        member or guest, session or token
/_dd/signins/end                            POST     end one, or all others                    member or guest: token, or passkey fresh
/_dd/devices/names                          GET,PUT  sealed device names                       member or guest, session or token
/_dd/controllers                            GET,PUT  sealed list of home controllers           member, session or token
/_dd/add/start, /status, /cancel, /theirs   *        the approving device's side               member or guest, session or token
/_dd/add/offer, /wait, /reveal              *        the new device's side                     anyone with the code
-- photos
/_dd/photos                                 GET      hands a browser to the app (use-app)      anyone
/_dd/photos/move                            GET      the one-time move off a passkey's PRF     member
/_dd/photos/config                          POST     museum, address                           member or demo; session or device token
/_dd/photos/museum/{op}                     POST     verify-email, change-email + code         member, own address; session or token
/_dd/photos/forget                          GET      Clear-Site-Data, then on                  anyone
/_dd/photos/handoff                         POST     open a slot                               member (session or token)
/_dd/photos/handoff/{id}                    PUT      browser fills it                          session of the slot's owner
/_dd/photos/handoff/{id}                    GET      app takes it                              the slot's owner
/_dd/photos/demo, /list, /thumb/{id},       GET      the demo's gallery                        demo session
  /photo/{id}
-- libraries and storage
/_dd/dav/{lib}, /{lib}/, /{lib}/{*}         any      WebDAV over a library                     owner, reader; demo on its library
/_dd/library/{lib}/notes/progress           GET,PUT  the viewer's place in each film, sealed   owner, reader (each their own)
/_dd/library/{lib}/notes/shelf              GET,PUT  the library's shelf, sealed               GET owner, reader; PUT owner
/_dd/images, /_dd/images/, /_dd/images/{*}  any      a member's disk images                    member, own; a browser's change
                                                     (files.<domain>/images/)                  needs a fresh passkey
/_dd/storage/mine                           GET      allowance and use                         member, session or token
/internal/storage/members                   GET      library and disk-image bytes per member   local (the Photos box's dd storage-ledger)
-- network and house
/_dd/network/join                           POST     Headscale join key                        token; member or guest
/_dd/network/box                            POST     a box's key, sealed to it                 a listed box's TPM signature
/_dd/house                                  GET      every box in the house                    member, session or token
/_dd/house/here                             GET      this box's link                           local; a directory box: its house
/_dd/house/wifi                             POST     a signed Wi-Fi change                     member + passkey signature
/_dd/house/wifi/relay                       POST     a peer passing it on                      port 4181 only (nginx 404s it); signature
-- home controllers (controller.rs, home-controller.md)
/_dd/controller/directory/{name}            GET      one person's entry                        a controller (x-dd-controller)
/_dd/controller/backup                      POST     {put, get} for its backup, 10 min         a controller; 404 without the bucket
/_dd/controller/backup/{label}              GET      {get}, to restore                         member (session or token), their own label
/_dd/controller-name                        GET      the passkey's naming page                 anyone (the page signs in)
/_dd/controller-name/{label}                POST     leave the passkey's {sig}                 member session, our own pages
/_dd/controller-name/{label}                GET      {sig}, once, 10 min                       the same member's device token
-- friends and games
/_dd/friends/list                           GET      friends, links                            member or guest
/_dd/friends/link                           POST     a new link                                member
/_dd/friends/link/{id}                      DELETE   cancel a link                             its maker
/_dd/friends/remove/{name}                  DELETE   unfriend                                  session
/_dd/friend/{secret}                        GET      the link's page                           anyone
/_dd/friend/{secret}                        POST     accept                                    session, not the demo
/_dd/friend/{secret}/about                  GET      whose link, who is looking                anyone with the link
/internal/games/access                      POST     who may play, and their addresses         local (games manager)
/internal/games/friends/{user}              GET      who they may invite                       local
-- leaving (leaving.rs, leaving.md)
/_dd/leave                                  GET      where the signed-in person stands         session or token
/_dd/leave/start                            POST     a statement for the root passkey          session; anyone not removed
                                                                                               and not left already
                                                                                               (member, guest, waiting)
/_dd/leave/sign                             POST     the passkey's answer, into the log        session + the ceremony
/_dd/leaving/{name}                         GET      the newest signed statement               as its entry: member; the person; a key in it; a reader key (the mail Worker)
/_dd/leaving/{name}                         PUT      a statement `dd leave` signed             the root's signature
/internal/leaving                           GET      who left, when, due, done by whom         local (the deleting units)
/internal/leaving/done                      POST     a service deleted its share               local
-- remote computers (computers.rs, remote-computers.md)
/_dd/computers/{kind}                       PUT      one signed item for the control plane:    signed in as its owner; signed by their
                                                     computer-config, computer,                root (a start or stop: or a device of
                                                     computer-power, computer-delete           theirs on file now); then as the
                                                                                               directory's PUTs (one limiter, 64 KiB,
                                                                                               name and key allowances, sent once)
-- the rest
/_dd/fleet.json                             GET      boxes' release, agent, backup             member (session or token)
/_dd/status                                 GET      the uptime page                           anyone
/_dd/status.json                            GET      each service up, and its 90 days          anyone
/_dd/git/events                             GET      forge changes (SSE)                       member (session or token)
/_dd/chat/search                            GET      web search                                member
/_dd/adblock/state                          GET      on/off, totals, lists                     household member, session or token
/_dd/adblock/switch, /allow                 POST     switch, pause, allow a site               household member, session or token
/_dd/adblock/allow/{domain}                 DELETE   stop allowing                             household member, session or token
/_dd/oidc/.well-known/openid-configuration  GET      discovery                                 anyone
/_dd/oidc/authorize                         GET      a code for Vaultwarden                    member session (else -> login)
/_dd/oidc/token                             POST     id token                                  the client, by secret
/_dd/oidc/userinfo                          GET      claims                                    access token
/_dd/oidc/jwks                              GET      the signing key                           anyone
```

A directory-only box serves `/health`, the directory routes,
`/_dd/house/here`, `/_dd/house/wifi/relay`, `/_dd/leaving/{name}` and
`/internal/leaving`, and nothing else.

## Configuration

### NixOS options

In [verify.nix](../../nix/modules/gate/verify.nix) unless noted:

```
dd.verify.enable          run it (default true; VM tests that do not involve the gate turn it off)
dd.verify.role            "full" | "directory" (core.nix: directory; gateway.nix: full)
dd.verify.port            4181
dd.verify.hosts           the subdomains it stands in front of:
                          files llm metrics git home photos games
dd.verify.names           extra names a token's audience may be (default: dd.box.net)
dd.verify.directoryReaders  keys that read single entries: fleet/mail.json reader (core.nix)
dd.verify.housePeers      the same house's boxes (flake.nix)
dd.verify.fleet           box name -> fleet-network address (flake.nix)
dd.verify.releasePublicKey  fleet/release.pub (core.nix)
dd.members                fleet/members.json (core.nix)
dd.home.services          the home tiles, from each role
dd.verify.appManifest     the signed app.json url (gateway.nix)
dd.verify.tmdbKeyFile     TMDB token (gateway.nix, sops)
dd.verify.demoLibrary     {id, key}, not secret
dd.verify.library         {s3, bucket, keyFile}  the libraries bucket
dd.verify.photos          {api, emailSuffix, codeFile, demoPasswordFile}
dd.verify.network         {api, url, keyFile, boxes}  Headscale and the boxes' TPM keys
```

### Environment

Read in [main.rs](../../servers/gate/src/main.rs) unless noted. "file"
means the variable names a file to read.

```
VERIFY_ROLE                    full | directory
VERIFY_BIND                    127.0.0.1:4181; 0.0.0.0:4181 on a directory box with house peers
VERIFY_DIR                     /var/lib/dd-verify/keys (its parent is the state directory)
VERIFY_DOMAIN                  full only: the relying party and cookie domain
VERIFY_HOSTS                   origins allowed to run a passkey ceremony (lib.rs own_hosts)
VERIFY_MEMBERS                 full only, required: the member list as json
VERIFY_RELEASE_PUB             the release key (invites, grants, app manifest)
VERIFY_PAGES_DIR               full only, required: the pages tree (pages.rs)
VERIFY_WEB_DIR                 the wasm bundle
VERIFY_HOME                    the tiles, json
VERIFY_REPO                    the repository on the forge, for docs links (docs.rs)
VERIFY_CONTROL                 dd-control's address (http://127.0.0.1:4195): agreed mode
VERIFY_CONTROL_SUBMIT          its submit socket, /run/dd-control/submit.sock
VERIFY_CONTROL_CONFIG          /etc/dd-control/config.json (the voters)
VERIFY_BOX                     this box's name, if not the hostname (tests)
VERIFY_HOUSE_PEERS             the house's boxes
VERIFY_NAMES                   names besides the domain's a token may be for (the box's fleet address)
VERIFY_DIRECTORY_READERS       keys that may read one entry, never the list (the mail Worker's)
VERIFY_FLEET                   box -> address, json
VERIFY_THANOS                  Thanos query, when there is no control plane, and the uptime page (status.rs)
VERIFY_SERVICE_BOXES           service -> the boxes the release builds with it, json (status.rs)
VERIFY_HOUSE                   /run/dd-wifi (home-network.nix; home.rs)
VERIFY_HOUSE_ORIGINS           where a Wi-Fi change may be signed (home.rs)
VERIFY_ENDED                   /run/dd-ended, for Passwords (vaultwarden.nix; directory.rs)
VERIFY_APP_MANIFEST            the signed app.json
VERIFY_TMDB_KEY_FILE           file
VERIFY_SEARCH                  SearXNG (llm/search.nix)
VERIFY_FORGE_EVENTS            libpq string for LISTEN (forge/live.nix)
VERIFY_STORAGE_LEDGER          the fleet's count, json (storage/view.nix, every box)
VERIFY_DEMO_LIBRARY_ID, _KEY   the demo's library
VERIFY_PHOTOS_API, _SUFFIX     museum, the address suffix
VERIFY_PHOTOS_CODE_FILE        file: museum's verification code
VERIFY_PHOTOS_DEMO_FILE        file: the demo's ente password
VERIFY_LIBRARY_S3, _BUCKET,    the bucket (servers/library from_env); LIBRARY_ID and
  _REGION                      LIBRARY_SECRET from the unit's EnvironmentFile
VERIFY_HEADSCALE_API, _URL     Headscale's api, and what a device joins (network.rs)
VERIFY_HEADSCALE_KEY_FILE      file: the api key
VERIFY_NETWORK_BOXES           file: {box: {key, recipient}}
VERIFY_AGE                     age with the TPM plugin on its path
VERIFY_PEOPLE                  the release's people (policy.json owner, people): never reaped
VERIFY_REAP_SECS               how often the network reaper runs, 300 (network.rs; tests)
VERIFY_REAP_GRACE_SECS         who is who known this long before anything comes off, 120
VERIFY_REAP_PEOPLE             the most people one pass turns out before the brake, 2
VERIFY_REAP_FACTS              where the brake's dd_reap_brake fact is written
VERIFY_ADBLOCK, _HOUSEHOLD,    Pi-hole's api, the household, the box's LAN address,
  _LAN, _STATE                 where on/off is remembered (adblock/pihole.nix)
VERIFY_ADBLOCK_PASSWORD_FILE   file: Pi-hole's api password
VERIFY_OIDC_ISSUER, _CLIENT_ID, Passwords' issuer (vault/vaultwarden.nix)
  _CLIENT_SECRET_FILE, _REDIRECT
```

### State

```
/var/lib/dd-verify/          (StateDirectory, 0700, in the box's backup: core.nix)
  keys/<name>.json           the directory: signed entries
  directory-before-log/      entries from before the agreed log, until migrated
  invites/<key>.json         signed invites, until they expire
  held/<name>.json           sign-ups not yet followed through (this box only)
  replay.json                agreed-log replay position, claims, acks, outcomes
  session.secret             the cookie's HMAC key
  signins.json               sessions, ended sessions, key uses
  oidc.key                   Passwords' issuer signing key
  friends.json               links (hashed), friend pairs, guests
  leaving.json               who left by their own word (the signed statement, when
                             agreed), and what this box has done for each leaving
  names/<user>.bin           device names, sealed in the browser
  notes/<lib>/shelf.bin      Movies & TV's shelf, sealed in the browser
  notes/<lib>/progress/<user>.bin   each viewer's place, sealed in the browser
/run/dd-wifi/                request, status.json, result.json, seen.json
/run/dd-ended/<name>         a member whose other sessions are to end
```

In memory only, lost on restart: ceremonies, pending enrol passkeys,
photos handoffs, add-device codes, passkey freshness, demo rate counts,
OIDC codes and tokens.

The unit runs as `dd-verify` with `ProtectSystem=strict`,
`MemoryDenyWriteExecute`, `@system-service` syscalls, and only
`AF_INET`/`AF_INET6` (plus `AF_UNIX` for the control socket).

### Background tasks

```
directory follow       every 1 s (agreed mode): replay, ack, migrate
held sign-ups          every 300 s: publish those the member list now names
network reap           every 300 s: remove known non-members' devices from Headscale (guarded)
library trash          daily (hourly after a failure): empty 30-day-old trash
leaving                every 60 s: end the ways in of whoever left or was removed,
                           and delete this gate's share once due: at once for
                           their own word, 30 days after a removal (leaving.md)
forge events           LISTEN, reconnecting every 5 s
ad blocking            at start: put the household's last on/off back
```

## Tests

**Unit tests** (`#[test]` in the crate, about 50): the session format
and forging (session.rs), the directory's cache, held sign-ups, library
claims, a write racing a write, and the agreed log (a new name needs
every voter, two claims to one name, an owner's update lands at once,
every box replaying one log ends the same, migration) in directory.rs;
the demo's allowance by door, image deletion paths, usernames, who may
read the directory (members, owners, keys, the reader key, read tokens and
their audience) (lib.rs);
the pages and the downloads page (pages.rs); docs indexing (docs.rs);
friends (friends.rs); reaping and box keys against a fake Headscale
(network.rs); user agents in words and ended sessions (signins.rs); OIDC
(oidc.rs).

**End-to-end in-process**:
[client/cli/tests/e2e.rs](../../client/cli/tests/e2e.rs) runs real gates
with `verify::start` on localhost and drives them with `dd` and a soft
passkey: tokens and recovery, enrol links vs access tokens, membership,
browser sign-up and waiting, invite codes, a closed directory (sign-up,
admitting, recovery and sign-in with no anonymous reads), the demo's permission set, the
download page, friend links and guests, the photos code, demo doors,
deleting an image, sessions ending with their device, Wi-Fi changes and
adding a device. Both run in CI's `test` job
(`nix build .#checks.x86_64-linux.tests`).

**VM and process tests** that involve the gate, in
[nix/tests](../../nix/tests):

```
directory.nix   two boxes: publish, admit a device, a restart; the list refused to
                a stranger; single entries refused once a reader key is set
control.nix     the agreed log end to end: relay, three validators, a gate
                per box in directory role (control/run.py)
library.nix     full gate + garage: rclone over the gate's WebDAV; disk images
                under /_dd/images; the allowance held by what an upload carries
vault.nix       Vaultwarden signed in only through the gate's issuer; Send
network.nix     Headscale: a box joins as a box, a member's device gets a key; in agreed
                mode with dd-control down at the start nothing is reaped or deleted,
                and once it answers the revoked member's and an unknown name's
                devices come off, the owner's and a member's stay; a state that
                drops half its members holds the brake on until it is restored
adblock.nix     the household switching Pi-hole through the gate
games.nix       the gate answers who is whose friend for invites
forge.nix       the full gate, for the Git pages' live changes
nginx.nix       every box's nginx config through `nginx -t`
```

## Changing it

- **Who may call a handler is decided in the handler.** nginx only sends
  `/_dd/` to the gate; it does not check anything there. A new route
  starts from the nearest existing one and its check (`signed_in`,
  `identify`, `member`, `role`, `guest`), and the route table above gets
  a line.
- **The demo and guests are not members.** `member("demo")` is true when
  the demo is on, so a members-only handler says `user != DEMO_USER`
  as well; guests need `role()` or `guest()`. Look at how a neighbour
  does it before writing a new test of who.
- **A change that writes from a browser** uses JSON (or another type a
  cross-site form cannot send) so the browser's own preflight protects it
  (`from_our_page` in friends.rs and adblock.rs).
- **Anything that ends access must reach the other services**: write the
  name to `VERIFY_ENDED` as `end_elsewhere` does.
- **The directory's rules live in one place.** Replay, `admit` and the
  peer pull all call the same checks; a new rule goes in there, with a
  test that every box replaying one log ends at the same directory.
- **A page or script** is a file under `servers/<service>/web/`, listed in
  `STATIC` or `PAGES` in pages.rs, and in the flake's pages directory. A
  missing file stops the gate at start (and the unit tests, which read
  the tree), so a typo fails a test rather than a page.
- **A new host** the gate stands in front of goes in `dd.verify.hosts`,
  or its pages cannot run a passkey ceremony (`own_hosts`) and it gets no
  `/_dd/` location or `@login`.
- **A new public path** goes through public.nix: a name not listed there
  is closed to outsiders, whatever the gate would answer.
- **Rename debt.** The crate, unit, user and variables are still `verify`.
  Renaming the unit or user moves `/var/lib/dd-verify`, which is in the
  backup; do it as its own change.
- Test with the unit tests and `client/cli/tests/e2e.rs` first; they start
  gates in seconds. Then the VM test for the part you touched.
