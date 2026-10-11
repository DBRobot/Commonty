# The gate

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
Member  on the member list by member_id(root) and not revoked, or the
        entry carries a grant (an invite) the release key signed.
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
list) carries no names.

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

sign up      POST /_dd/join/start {username}       name free, not "demo",
                                                    not guest* (unless x-dd-probe)
             POST /_dd/join/finish (new credential [+ library_key])
               → an entry with that passkey as root (and a grant, if the
                 browser sent an invite claim in x-dd-grant); the browser is
                 asked to sign the entry's hash with the same passkey
             POST /_dd/join/sign (assertion)
               → with a grant or already on the member list: admitted to
                 the directory. Otherwise HELD on this box (below).
                 Either way: a session cookie.

redeem       POST /_dd/redeem/start (x-dd-grant)   signed in, passkey root
             POST /_dd/redeem/sign                 same as join/sign: the
                                                    entry with the grant, v+1

enrol        dd enrol prints a link with a token minted for purpose "enrol"
             POST /_dd/enrol/start   (Bearer enrol token)
             POST /_dd/enrol/finish  the new passkey is kept in memory for
                                     10 minutes under sha256(token); never stored
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
                                                    from), else /_dd/photos
```

The signature counter is not written back after a sign-in: the
credential lives in the signed entry, which the gate cannot change. Clone
detection by counter is given up for that (comment in `login_finish`).

**Held sign-ups.** A browser sign-up with no code and not on the member
list is kept on this box only, in `held/`, never served or pulled. The
name counts as taken here and the person can sign in; they land on the
waiting page. It becomes an entry when a code is redeemed, when a friend
link is accepted (as a guest), or when the member list names it: a task
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
everyone else               401, whether or not the name exists
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
                   by the recovery key on file (client/identity)
check_grant        a grant: signed by the release key, proved by the root,
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
write (PUT /_dd/directory/x, a join, a redeem, PUT /_dd/invite/k)
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

A member's own signed statement that they leave, or come back, is a
directory item too (`Item::Leave`), replayed at its agreed time into
`leaving.json`; someone who left is no member from then on. What happens
next, and when, is [leaving.md](leaving.md).

### Who reads it on the box

- The gate, on every request (entries are cached by file time and size).

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
  and Photos' `photos.html` in `servers/photos/templates`
  ([askama.toml](../../servers/gate/askama.toml)). Their scripts are in
  [servers/gate/web](../../servers/gate/web).
- **The wasm bundle** (client/web, built by the flake) is served from
  `VERIFY_WEB_DIR` at `/_dd/web/{file}`, `.js` and `.wasm` only.
- **Every static page fills itself from `/_dd/me`**: the name, role, the
  bar's menu and the tiles (`pages::me_json`), plus `tmdb` for members and
  the demo, `guestOf` for a guest, and the ad-blocking switch for the
  household. `no-store`. The app carries the same pages.
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
location: HSTS, `nosniff`, `Referrer-Policy: strict-origin-when-cross-origin`.
Locations that proxy to the gate (or the games manager) get the content
policy:

```
default-src 'self'
script-src  'self' 'wasm-unsafe-eval'
style-src   'self' 'unsafe-inline'
img-src     'self' data: blob: https:
media-src   'self' blob: https://*.<domain>
connect-src 'self' https://*.<domain> wss://*.<domain>
            https://api.themoviedb.org https://image.tmdb.org
font-src    'self' data:
frame-src   'self' blob: https://*.<domain>
worker-src  'self' blob:
object-src 'none'; base-uri 'self'; form-action 'self' https://*.<domain>
frame-ancestors 'self'; report-uri /_dd/csp
```

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

The step between the site and Ente's own app. The passkey (through the
wasm in `photos.js`) makes the photo account's password in the browser.
`/_dd/photos/config` (a session, or a device token: the app's own copy
of Photos asks with the device's) gives the page museum's address, the
person's ente address (`<user><VERIFY_PHOTOS_SUFFIX>`) and the rpId; the demo also gets
its password. The two museum calls that need the fleet's verification
code (`verify-email`, `change-email`) go through
`/_dd/photos/museum/{op}`: the gate checks the address is the caller's
own and adds the code, which never reaches a browser.

The app cannot use a passkey, so it borrows a browser's: the app opens a
**handoff** slot (device token), a signed-in browser fills it with the
password sealed to a key only the app holds, and the app takes it once.
At most 256 slots, 10 minutes each, 1 KiB each.

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
claim that one. A daily task empties what has been in the trash for three
months (an hour later if the bucket is not up).

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
  everyone revoked).

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
(except Photos). Every other name closes the connection (444) to anyone
not on the fleet network.

On a public gated host, the ceremonies a stranger may start
(`/_dd/(join|login|enrol|redeem|network|add)/`) are limited to one a
second per address, ten in hand; so are `/_dd/directory` and
`/_dd/invite` for a request that carries no credential at all (no
session, no token, no read proof).

**The bare domain** sends `/` to `https://home.<domain>`, `/download` to
the downloads page, serves release files from the forge as nobody under
`/download/vX.Y.Z/<file>`, and serves the docs (`/docs`, `/docs/...`)
with only the gate paths their page needs: `/_dd/static/`, `/_dd/web/`,
`/_dd/docs/`, `/_dd/me`, `/_dd/config`, `/_dd/csp`.

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
/_dd/use-app                                GET      "open it in the app", photos. and vault.  anyone
/_dd/logout                                 POST     end session, clear photo storage          our own pages
/_dd/join                                   GET      sign-up page                              anyone
/_dd/join/start, /finish, /sign             POST     account from a passkey                    anyone
/_dd/redeem/start, /sign                    POST     add an invite to your entry               session, passkey root
/_dd/enrol                                  GET      enrol page                                anyone
/_dd/enrol/start, /finish                   POST     make a passkey for dd                     token with purpose "enrol"
/_dd/enrol/result                           GET      dd collects the passkey                   the same token
/_dd/app/signin                             POST     device token -> session                   the app's page; member
/_dd/demo                                   GET      a demo session; signed in: the button     anyone, if the demo is on
/_dd/demo                                   POST     a demo session                            our own pages, if the demo is on
-- pages and what every page needs
/_dd/home                                   GET      home, or waiting page                     session (else -> login)
/_dd/me                                     GET      who, role, menu, tiles                    session or token; 403 without a role
/_dd/config                                 GET      rpId (+ demo library)                     anyone
/_dd/static/{file}                          GET      css, js, fonts, icons                     anyone
/_dd/web/{file}                             GET      wasm bundle                               anyone
/_dd/files, /_dd/media                      GET      Files, Movies & TV                        member; demo if a demo library
/_dd/settings                               GET      Settings                                  member or guest
/_dd/boxes, /backups, /devices,             GET      redirect to /_dd/settings#<tab>           anyone
  /network, /storage
/_dd/chat                                   GET      Chat                                      member or demo
/_dd/git, /_dd/metrics                      GET      Git, Metrics                              any session
/_dd/friends                                GET      Friends                                   member or guest
/_dd/adblock                                GET      Ad blocking                               household member
/_dd/add                                     GET      add-device page (QR target)               anyone
/_dd/send                                   GET      Passwords' Send page                      anyone
/_dd/download                               GET      app downloads                             anyone
/_dd/docs/page, /_dd/docs/page/{*}          GET      docs page                                 anyone
/_dd/docs/index.json, /_dd/docs/raw/{*}     GET      docs index, a page's markdown             anyone
/_dd/csp                                    POST     CSP report, logged                        anyone
/_dd/player-error                           POST     Movies & TV's player error, logged        anyone
-- directory
/_dd/directory                              GET      names and versions                        member (cookie or token); else 401
/_dd/directory/{name}                       GET      an entry                                  member; the person; a key in it; a reader key; else 401 (anyone until a reader key is set)
/_dd/directory/{name}                       PUT      publish an entry                          anyone; the signature decides
/_dd/invite/{key}                           GET      an invite                                 anyone
/_dd/invite/{key}                           PUT      keep an invite                            anyone; release key must sign it
-- sessions and devices
/_dd/signins                                GET      sessions, key uses                        member or guest
/_dd/signins/end                            POST     end one, or all others                    member or guest, passkey fresh
/_dd/devices/names                          GET,PUT  sealed device names                       member or guest
/_dd/add/start, /status, /cancel, /theirs   *        the approving device's side               member or guest
/_dd/add/offer, /wait, /reveal              *        the new device's side                     anyone with the code
-- photos
/_dd/photos                                 GET      Photos' sign-in step                      member; demo with a password
/_dd/photos/config                          POST     museum, address, rpId                     member or demo; session or device token
/_dd/photos/museum/{op}                     POST     verify-email, change-email + code         member, own address
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
/_dd/storage/mine                           GET      allowance and use                         member
/internal/storage/members                   GET      library and disk-image bytes per member   local (the Photos box's dd storage-ledger)
-- network and house
/_dd/network/join                           POST     Headscale join key                        token; member or guest
/_dd/network/box                            POST     a box's key, sealed to it                 a listed box's TPM signature
/_dd/house                                  GET      every box in the house                    member
/_dd/house/here                             GET      this box's link                           local; a directory box: its house
/_dd/house/wifi                             POST     a signed Wi-Fi change                     member + passkey signature
/_dd/house/wifi/relay                       POST     a peer passing it on                      port 4181 only (nginx 404s it); signature
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
/_dd/leave                                  GET      where the signed-in person stands         session
/_dd/leave/start                            POST     a statement for the root passkey          session; member to leave,
                                                                                               someone who left to come back
/_dd/leave/sign                             POST     the passkey's answer, into the log        session + the ceremony
/_dd/leaving/{name}                         GET      the newest signed statement               as its entry: member; the person; a key in it; a reader key (the mail Worker)
/_dd/leaving/{name}                         PUT      a statement `dd leave` signed             the root's signature
/internal/leaving                           GET      who left, when, due, done by whom         local (the deleting units)
/internal/leaving/done                      POST     a service deleted its share               local
-- the rest
/_dd/fleet.json                             GET      boxes' release, agent, backup             member (session or token)
/_dd/git/events                             GET      forge changes (SSE)                       member (session or token)
/_dd/chat/search                            GET      web search                                member
/_dd/adblock/state                          GET      on/off, totals, lists                     household member
/_dd/adblock/switch, /allow                 POST     switch, pause, allow a site               household member
/_dd/adblock/allow/{domain}                 DELETE   stop allowing                             household member
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
VERIFY_THANOS                  Thanos query, when there is no control plane
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
network reap           every 300 s: remove non-members' devices from Headscale
library trash          daily (hourly after a failure): empty three-month-old trash
leaving                every 60 s: end the ways in of whoever left; past 90 days,
                           delete this gate's share of them (leaving.md)
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
network.nix     Headscale: a box joins as a box, a member's device gets a key
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
