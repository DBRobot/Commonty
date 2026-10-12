# Packaged upstream programs

Some of what Commonty offers is someone else's program: Ente for Photos,
Vaultwarden for Passwords, Pi-hole for ad blocking, llama.cpp for Chat,
Thanos for Metrics, Forgejo for Git. This page is what each one is, which
version runs, what we changed in it and why, and how to move it to a new
upstream version without breaking it.

## The approach

Three rules, held for every program here:

- **Patch as little as possible.** A setting, an nginx rule or a page of our
  own beside the program comes first. A patch is for what nothing else can
  reach: how a session token is stored, a sign-in screen that must never
  show. Every patch is a cost at every upgrade, so each one says in its
  module why it exists.
- **Patches live with the service**, in `servers/<service>/` beside its
  pages: [servers/photos](../../servers/photos),
  [servers/vault](../../servers/vault). The nix module that applies a patch
  names it by that path. Small edits a build can make with
  `substituteInPlace --replace-fail` or `sed` stay in the module; they fail
  the build when upstream moves the text they match, which is the point.
- **Sign-in is always the gate's.** No program here has a password form a
  member uses to get in. Either nginx asks the gate who is calling
  (`auth_request`), or the program trusts the gate as its identity provider
  (OIDC), or our own page signs the member in to the program with a key
  their app keeps (never one made from a passkey). Nobody has an account
  password to know.

How a request reaches a program, in the common case:

```
browser ── https://<service>.<domain>/... ──▶ nginx
                                              │ auth_request /_dd/verify ──▶ gate
                                              │   200 + X-Auth-Request-User, X-DD-Role
                                              │   401 → /_dd/login (passkey), 403 → home
                                              │ dd_session cookie taken out
                                              ▼
                                           the program, on 127.0.0.1 or a socket
```

- `/_dd/verify` is the gate's answer for nginx
  ([servers/gate/src/lib.rs](../../servers/gate/src/lib.rs), `verify`). It
  takes a passkey session cookie or a device's bearer token, and returns
  who it is and their role: member, demo or guest. The demo gets only what
  its tile allows; a guest gets only the games pages.
- The gate's session cookie never reaches a program:
  [strip-cookie.nix](../../nix/modules/gate/strip-cookie.nix) removes it from
  every location that proxies anywhere but the gate.
- Every vhost the gate covers also gets `/_dd/` (the gate's own pages),
  `@login` and `@waiting`
  ([nix/modules/gate/verify.nix](../../nix/modules/gate/verify.nix)).
- Only `home`, `accounts`, `headscale`, the bare domain and the demo doors
  are reachable from outside the fleet's network
  ([nix/roles/gateway.nix](../../nix/roles/gateway.nix),
  [public.nix](../../nix/modules/gate/public.nix)). Passwords adds `vault`
  for its Send links alone. Everything else on this page is reached on the
  fleet's network only.

The app carries the code for Photos and Passwords
([app/src/local.rs](../../app/src/local.rs)): Ente's photos client and
the web vault come in its signed release, never from a box, so root on a
box cannot hand the page that holds a member's keys a copy that sends
them away.

- It serves them on local servers, Photos on port 41661 and Passwords on
  41662: to its own window at `127.0.0.1`, and to the member's own browser
  at a name made for that browser alone (`p-<session>.localhost`,
  `v-<session>.localhost`).
- A door sets the cookie every other request there needs. The window's
  door takes the launch secret. A browser gets a door only after the
  member allows it in the app's window, by a code both show, and only
  that browser can then use the door.
- A window or a browser is only sent to a port the app holds.
- Code-type answers from a box (html, js, css, svg, xml) are refused.
- Both clients are built into the app on every platform (`appWeb` in
  flake.nix bundles them under `app/web/_clients/`, source maps left out),
  and the app serves them from its own assets. The boxes still build them
  from the box config, but no box serves either one to a browser.
- In a browser, `photos.<domain>` and `vault.<domain>` show a handoff page
  (the gate's `/_dd/use-app`,
  [use-app.html](../../servers/home/web/pages/use-app.html)). Its button
  opens `commonty://open?service=photos|vault&path=…`, and the app opens
  this browser at its own copy once the member allows it in the app. If
  nothing opens, the page says the app must be installed and signed in,
  with the download link and "Try again".
  How the handoff is kept safe is in
  [app.md](app.md#the-sites-key-pages-photos-and-passwords-in-a-browser). nginx keeps Photos'
  `/_dd/` and `/_dd/museum/`, and the vault's `/api`, `/identity`,
  `/notifications`, `/events`, `/alive` and Send paths.
- Bitwarden's desktop app and browser extension still sign in through
  `vault.<domain>/#/sso`: [use-app.js](../../servers/home/web/use-app.js)
  makes the same hops the web vault did (`identity/sso/prevalidate`, then
  `identity/connect/authorize`), and `/sso-connector.html` says "Signed
  in" for the extension. A Send link goes to the Send page.

Each program also has a `needs.nix` beside its module: what it needs from a
box, what it reaches, and how much of members' data it can read. That is
the input to placement ([hardware.md](hardware.md)).

## Photos (Ente)

### What it is

[Ente](https://github.com/ente-io/ente) is an end-to-end encrypted photo
library. Two parts run here:

- **museum**, Ente's server (Go): accounts, albums, and where each encrypted
  file is. The files themselves are in the garage bucket `ente`.
- **ente-web**, Ente's web apps (Next.js): photos, accounts, albums, cast,
  and locker. They are built to static files and served by nginx.

Both are pinned to the tag `photos-v1.3.36`, by an overlay in
[nix/modules/photos/ente.nix](../../nix/modules/photos/ente.nix), whatever
nixpkgs has. Our Rust that speaks Ente's account protocol (in the browser
and in the gate) pins Ente's `ente-accounts` and `ente-core` crates to
commit `d28c3462` ([client/ente/Cargo.toml](../../client/ente/Cargo.toml),
[client/web/Cargo.toml](../../client/web/Cargo.toml)). The NixOS module is
nixpkgs' own `services.ente`; locker, which that module does not know,
is built and served beside it with the same settings.

### The patches

Applied in this order (the menu patch builds on the passkey patch):

- [museum-hash-tokens.patch](../../servers/photos/museum-hash-tokens.patch),
  on museum. The `tokens` table keeps `sha256` of each session token
  instead of the token, so a copy of the database signs nobody in. Every
  query by token hashes first; the session list hands each app its own
  token back, so "this device" still matches; ending a session takes the
  token or the hash the list gave out. Sessions made before the patch are
  hashed once by `ente.service`'s `preStart` (a token is never 64 hex
  characters, a hash always is).
- [museum-delete-now.patch](../../servers/photos/museum-delete-now.patch),
  on museum. Deleted means deleted: once a file's trash is emptied its
  objects go at museum's next run, not 45 days on (`DeleteObjectQueue` in
  `pkg/repo/queue.go`), and a deleted account's data is scheduled for the
  next run, not 7 days on (the insert in `pkg/repo/datacleanup`, over
  migration 59's default). `checks.retention` reads both from the patched
  source, so a bump that drops either fails it.
- [ente-web-theme.patch](../../servers/photos/ente-web-theme.patch), on every
  app: the site's colours, Public Sans, 4px corners.
- [ente-web-passkey.patch](../../servers/photos/ente-web-passkey.patch), on
  photos: every way out of the app that would show Ente's own sign-in or
  sign-up (no key in the session, the front page, sign-out) goes to
  `/_dd/photos` instead. The app is reached only through our page.
- [ente-web-commonty.patch](../../servers/photos/ente-web-commonty.patch), on
  photos: the site's bar across the top. Its stylesheet is the site's own
  [bar.css](../../servers/web-shared/bar.css), copied in at build; the bar
  loads `/_dd/static/shell.js` to fill in who is signed in.
- [ente-web-commonty-menu.patch](../../servers/photos/ente-web-commonty-menu.patch),
  on photos. The side menu without what needs the account password, which
  nobody knows: no Account, Help, Free up space, plans or Sign out (signing
  out is the site's). "Confirm it's you" asks the app for the
  password (photos-confirm.js) and checks it opens the master key, as a typed
  one would be checked. The recovery key is in the menu, behind that
  check, and so is the hidden section, as in Ente: hiding photos and
  albums works.
- [ente-web-commonty-thumbs.patch](../../servers/photos/ente-web-commonty-thumbs.patch),
  on photos. A thumbnail is fetched only once it has stayed in view a
  moment, so pictures flung past never start a fetch; and every thumbnail
  in the library is warmed into the disk cache in the background, three at
  a time, nearest first, pausing while anything on screen waits or the
  person scrolls.

Edits made in the module, not as patches:

- the photos grid draws 20 rows past the screen, not 3;
- Next's build id is fixed and `_buildManifest.js` is written back in a
  stable form, so two builds of the same source are the same bytes (the
  boxes vouch for a release by rebuilding it);
- museum listens on `127.0.0.1` for its API (8080) and metrics (2112),
  not every address.

`ente-health` restarts museum every few minutes if its login route answers
500, a known panic in its rate limiter
([ente-health.sh](../../nix/modules/photos/ente-health.sh)).

### Sign-in

The member's photo account is `<name>@users.<domain>`. Its password is
made by the member's app, at random, and kept with the device's keys
([app/src/photos.rs](../../app/src/photos.rs)). Nobody types it and no
passkey makes it: a passkey answers its PRF to any page of the domain, so
anything made from one is open to every box. The member's other devices
get it from a copy beside their disk images (`dd-photos.json`), sealed to
each device, the root and the paper key of their root-signed entry and
signed by the device that made it; a copy no device of theirs signed is
not taken.

```
the app's Photos page (its window, or a      the app                    gate / museum
 browser it let in, port 41661)
 POST /_dd/photos/config ─────────────────────────────────────────────▶ api, accountApi, email
 POST /_dd/app/photos-key ──────────────────▶ once per door:
                                              {pw}            the account's
                                              {pw, fresh}     none yet: make it
                                              {pw, next}      a passkey's: move it
 ente_login (SRP, our wasm) ──────────────────────────────────────────▶ api.<domain>
   fresh, no account: ente_create ─▶ /_dd/museum ─▶ the app, with its token ─▶ photos.<domain>
     users/verify-email ─▶ gate adds the code ─────────────────────────▶ museum
   next: ente_rekey(pw → next) ────────────────────────────────────────▶ api.<domain>
 POST /_dd/app/photos-keyed {pw} ───────────▶ that password is the account's:
                                              keyring, dd-photos.json
 session written where Ente's app reads it, then /gallery
```

- [app/web/photos-open.js](../../app/web/photos-open.js) runs it, with our
  Rust in the browser ([client/web/src/lib.rs](../../client/web/src/lib.rs):
  `ente_login`, `ente_create`, `ente_rekey`). The master key never leaves
  the page.
- Museum makes accounts under `@users.<domain>` with a fixed verification
  code instead of a mail (`internal.hardcoded-ott`), which it honours
  because the module runs it as `ENVIRONMENT=local`. That code would let
  anyone claim any address there, so it never reaches a page: the two
  calls that need it go to the gate (`museum_verify` in
  [photos.rs](../../servers/gate/src/photos.rs)), which checks the address
  is the caller's own (session or device token) and fills it in.
- **Moving an account a passkey made.** Before the app held the password,
  a passkey's PRF under `sha256("dd-photos")` made it. A device that held
  that password (from the old handoff) re-keys the account the next time
  Photos opens (`{pw, next}`). A member whose devices never held it moves
  it once: the page offers it, the app opens a slot at the gate
  (`/_dd/photos/handoff`) and the browser at `home.<domain>/_dd/photos/move`,
  whose page ([photos-move.js](../../servers/photos/web/photos-move.js),
  the one page that still asks a PRF) seals that old password to a key only
  the app holds; then the page re-keys. After the move the old password
  opens nothing at museum.
- Ente's own "confirm it's you" imports `/_dd/static/photos-passkey.js`
  (the menu patch); the app answers it with its own
  [photos-confirm.js](../../app/web/photos-confirm.js), which asks the app.
- Before seeding, the page clears whatever Ente's app kept for someone else
  on its origin. Signing out of the site goes through
  `/_dd/photos/forget`, which clears the photos host's storage with
  `Clear-Site-Data`.
- Ente's web client comes in the app's release, built into the app, on
  port 41661 ([app/src/local.rs](../../app/src/local.rs)), in its window or
  the member's browser. What it is handed is in no address, so no history
  keeps it. That copy asks the gate's `/_dd/` routes with the device's
  token, and talks to museum (`api.<domain>`) and S3 directly.

### Our pages

- `/_dd/photos`: the page that hands a browser to the app; `/_dd/photos/move`:
  the one-time move,
  [servers/photos/templates/photos.html](../../servers/photos/templates/photos.html)
  rendered by the gate.
- `/_dd/photos/demo`: the demo's gallery,
  [demo-photos.html](../../servers/photos/web/pages/demo-photos.html) and
  [demo-photos.js](../../servers/photos/web/demo-photos.js). The gate signs
  in as the demo account itself and serves its thumbnails and pictures
  ([demo_photos.rs](../../servers/gate/src/demo_photos.rs)), so Ente's app
  and server are never opened to the demo. It signs in once at start, lists
  again every 10 minutes in the background (signing in again only when the
  session no longer lists) and opens every thumbnail ahead, so a visitor is
  always answered from what it holds and never waits on the sign-in's key
  derivation.

### Data and secrets

```
museum's records      postgres database "ente", local socket, no password;
                      dumped by storage/postgres-backup.nix
the photos            garage bucket "ente", ciphertext; browsers upload to it
                      directly with presigned URLs (CORS "*", see roles/photos.nix)
sops, read by ente    ente-ott, ente-key-encryption, ente-key-hash,
                      ente-jwt-secret, garage-key-id/secret
sops, read by gate    ente-ott (as ente-ott-verify), ente-demo-password
```

Museum's admin API is open to two accounts by id: the owner's
(`dd.photos.admin`) and the storage ledger's (`dd.photos.ledger`), which
sets each member's photo quota to what is left of their allowance (across
every box, from the fleet's count; only when that count is under an hour
old) ([nix/roles/photos.nix](../../nix/roles/photos.nix)).

### Public and members-only

All of Photos is members-only and on the fleet's network. The demo's
account has a zero quota (Ente's read-only), made once with
`dd photos-demo`; outsiders see it only as the gate's gallery on
`home.<domain>`. The gateway leaves Photos out of the demo doors on
purpose.

### Tests

[nix/tests/photos.nix](../../nix/tests/photos.nix) and
[photos.py](../../nix/tests/photos.py): museum as the module builds it, on
loopback alone, healthy by `ente-health`; the gate's page and settings for a
member, without the fleet's code; the code filled in for her own address
and refused for another's; her account made and its session kept as a
hash. Ente's web apps are left out (their build is most of an hour), so
what they do in a browser is checked by hand (below).
[pgrestore](../../nix/tests/pgrestore.nix) tests the dumps of an `ente`
database.

### Updating

1. Change `version`, `rev` and both `hash`es (museum and ente-web) in
   [ente.nix](../../nix/modules/photos/ente.nix).
2. Rebase each patch on the new tag, in order. A `--replace-fail` that no
   longer matches fails the build; find the new spot, don't drop it.
3. Newer museum hashes tokens itself (its migrations 134 and up, into a
   `token_hash` column). When that lands, drop
   `museum-hash-tokens.patch` and the `preStart` hashing, and carry the
   hashes we already have into `token_hash` by hand. Do not hash them
   again.
4. Move `ente-accounts` and `ente-core` in both Cargo files to the matching
   commit, and check `seed()` in [photos-open.js](../../app/web/photos-open.js)
   still writes the session in the shape the new app reads (`user`,
   `keyAttributes`, `encryptionKey`, the `kv` IndexedDB token).
5. Check by hand: a new account made by the app, an existing one opening,
   the move off a passkey's password, an upload, "Confirm it's you", the recovery key,
   hiding a photo and opening the hidden section,
   sign-out clearing the app, the app's handoff, the app's own copy of
   Photos opening (it is built from the same ente-web), and the demo
   gallery.

## Passwords (Vaultwarden)

### What it is

[Vaultwarden](https://github.com/dani-garcia/vaultwarden) is a Bitwarden
server in Rust; Bitwarden's own apps and its web vault talk to it. Version
`1.37.1` is pinned in
[nix/modules/vault/vaultwarden.nix](../../nix/modules/vault/vaultwarden.nix)
by an assertion: a nixpkgs that moves Vaultwarden stops the build there
until someone reads what changed. It is built with the `postgresql` and
`s3` features. The web vault is nixpkgs' `vaultwarden.webvault`, edited at
build and held the same way, at `2026.6.4+0`.

### The patches

[vaultwarden-hash-refresh.patch](../../servers/vault/vaultwarden-hash-refresh.patch):
the `devices` table keeps `sha256` of each refresh token, so a copy of the
database renews nobody. The token is hashed on save and on lookup and held
in the clear only in memory. A new sign-in on a known device gets a new
token, since the old one cannot be read back. Tokens from before the patch
are hashed once by `vaultwarden.service`'s `preStart`.

[vaultwarden-app-loopback.patch](../../servers/vault/vaultwarden-app-loopback.patch),
in `sso.rs`: the web client's SSO may return to where the Commonty app
serves its own copy of the web vault. That is exactly
`http://127.0.0.1:41662/sso-connector.html` for the app's window, or
`http://v-<32 hex>.localhost:41662/sso-connector.html` for the member's
browser's session. No other loopback address. The box cannot know each
session's name, so the patch checks its shape. Either way the code goes
back to the member's own device, and only the page that started the
sign-in holds its PKCE verifier.

Edits to the web vault, made in the module:

- breach checks ask `/pwned/range/` on this box instead of
  `api.pwnedpasswords.com` (the build fails if that URL is gone);
- its name, colour and icons are Commonty's: Bitwarden's name and mark on
  a domain that is not Bitwarden's got the site flagged as phishing;
- master passwords from 8 characters, not 12;
- our colours and type through Vaultwarden's own stylesheet hook
  ([user.vaultwarden.scss.hbs](../../servers/vault/web/user.vaultwarden.scss.hbs)).

### Sign-in

The gate is an OpenID Connect issuer for Vaultwarden alone
([servers/gate/src/oidc.rs](../../servers/gate/src/oidc.rs)), and
Vaultwarden takes no other sign-in (`SSO_ONLY`).

```
Bitwarden app or web vault
 └▶ vault.<domain>/identity/connect/authorize
     └▶ 302 home.<domain>/_dd/oidc/authorize ──▶ gate: a member with a session?
                                                 none: /_dd/login (passkey), then back
     ◀─ 302 vault.<domain>/identity/connect/oidc-signin?code=...
 Vaultwarden ── POST /_dd/oidc/token (client secret, PKCE S256) ──▶ gate
             ◀─ ES256 id token: <name>@<domain>, email_verified
```

- The issuer is the gate on Passwords' own box, reached at
  `home.<domain>`: sessions are per box, and the fleet's names send `home`
  to the gateway. So Passwords runs on the gateway (`with` in
  [fleet/roles.json](../../fleet/roles.json), and an assertion in the
  module).
- One client (`vaultwarden`), one redirect, codes good once for five
  minutes, PKCE checked. The signing key is made on first start
  (`/var/lib/dd-verify/oidc.key`). The demo and guests get no code.
- An account is made at a member's first sign-in (`SIGNUPS_DOMAINS_WHITELIST`
  is the domain). The OIDC sign-in proves who they are and nothing else: each
  vault is still opened by the master password on their own device
  (`SSO_AUTH_ONLY_NOT_SESSION`).
- Signing a member out everywhere: when one of their devices or passkeys
  is removed, the gate drops their name in `/run/dd-ended`.
  `dd-vault-end` then deletes their devices and changes their security
  stamp, so every app has to sign in again through the gate.
- The web vault is served from the app's release on port 41662, to the
  app's window and to the member's browser. Its `/api`, `/identity`,
  `/icons`, `/events`, `/notifications`, `/alive` and `/pwned` calls go
  through the app to `vault.<domain>`. The window first signs in at the
  gate (`POST /_dd/app/signin` with `then=vault`), which sends it back to
  the app's `/_dd/app/vault`. A browser signs in at the gate with its own
  passkey on `home.<domain>`, the way any sign-in does. Vaultwarden then
  sends it back to `v-<session>.localhost:41662/sso-connector.html`.

### Our pages

- `/_dd/send`: a Send link opened by anyone
  ([send.html](../../servers/vault/web/pages/send.html),
  [send.js](../../servers/vault/web/send.js)). It decrypts in the browser
  with the key after the `#`, using Bitwarden's Send scheme.
- The bar above the web vault, added by nginx with `sub_filter`.

### Data and secrets

```
records            postgres database "vaultwarden"; dumped like the others
attachments, Sends garage bucket "vaultwarden", under data/ (DATA_FOLDER s3://...)
breach list        /srv/pwned, its own dataset, ~80G, fetched weekly (pwned.nix)
sops               vault-key-id, vault-key-secret (garage), vault-oidc-secret
                   (shared by Vaultwarden and the gate)
```

Site icons are never fetched (`/icons/` is a 404): a request for one would
tell the box which sites are in a vault. Mail to members goes out by
`dd.mail.sendmail` to `<name>@<domain>`, and the mail Worker sends it to
the address that member gave it ([mail.nix](../../nix/modules/mail.nix)).

### Public and members-only

`vault.<domain>` is behind the public door, but an outsider gets the Send
page at any address, its files under `/_dd/static/`, and the three Send
API calls it makes. The rest of the API, `/identity/` and
`/notifications/` close the connection (444). Inside the fleet's network it is the whole web vault. The
breach list is inside only. There is no demo.

### Tests

[nix/tests/vault.nix](../../nix/tests/vault.nix) and
[vault.py](../../nix/tests/vault.py): Vaultwarden on postgres with its files
in garage; a password login refused; the issuer's discovery, keys and
refusals; the whole sign-in from a device token to a Vaultwarden session;
refresh tokens kept as hashes; `/run/dd-ended` signing her out; the Send
page served to anyone.

### Updating

1. Bump nixpkgs. The assertions stop the build if Vaultwarden or its web
   vault moved.
2. Read Vaultwarden's release notes, especially SSO and the `devices`
   table, then set `version` and `webvaultVersion`.
3. Rebase both patches: hash-refresh on `device.rs` and `identity.rs`;
   app-loopback on `sso.rs` (the web/browser redirect match).
4. Build the web vault. The breach-list, title and password-length edits
   fail loudly if their text moved. Check the name in the JS and JSON
   files by hand.
5. Check the Send API paths in nginx's rules still match what
   [send.js](../../servers/vault/web/send.js) calls.
6. Run the `vault` test.
7. Check that Passwords still signs in and returns, in the app's window
   (127.0.0.1:41662) and in a browser opened from `vault.<domain>`'s
   button (`v-<session>.localhost:41662`). The vault test checks only that
   Vaultwarden accepts these return addresses and refuses others.

## Ad blocking (Pi-hole)

### What it is

[Pi-hole](https://pi-hole.net) (its FTL daemon, version 6 API) filters
lookups for a household's devices. It runs on the house's main box with
Unbound as its only upstream, which forwards over TLS to Quad9 and
Cloudflare. Both come from nixpkgs (`services.pihole-ftl`,
`services.unbound`) at whatever version the flake's nixpkgs has; nothing is
pinned or patched. The blocklist is StevenBlack's hosts file, refreshed
weekly. Module:
[nix/modules/adblock/pihole.nix](../../nix/modules/adblock/pihole.nix);
role: [nix/roles/adblock.nix](../../nix/roles/adblock.nix).

What we set instead of patching:

- privacy level 3: totals only, nothing about which device asked what;
- no DHCP, no NTP; listens on the house address, the box's address on the
  fleet's network (for the owner's own devices, when set to ask it: the
  network hands every device Quad9, unfiltered), and loopback; the
  firewall lets port 53 in from the house network only, and from the
  owner's devices on `commonty0`
  ([nix/modules/net/owner.nix](../../nix/modules/net/owner.nix));
- the lists are written into Pi-hole's `adlist` table by
  `dd-pihole-lists` and not by the module, whose own setup fails the
  second time it runs.

### Sign-in

Pi-hole's API listens on `127.0.0.1:8053` and wants a password only the
gate holds. Pi-hole's own web pages are not used. The gate is its only
door ([servers/gate/src/adblock.rs](../../servers/gate/src/adblock.rs)):
it signs in to Pi-hole, keeps the session, and lets only members listed in
the house's `household` use it. Changes must come as JSON, which a form on
another site cannot send.

### Our pages

- `/_dd/adblock`: totals, the lists, a pause, and sites let through
  ([adblock.js](../../servers/adblock/web/adblock.js)).
- "Ad blocking at home" in the bar's menu, with its switch, for the
  household only (from `/_dd/me`).

The household's on/off is kept by the gate in
`/var/lib/dd-verify/adblock` and put back after a release, since Pi-hole
comes up blocking.

### Data and secrets

Pi-hole's own state under `/var/lib/pihole` (counts and the gravity
database). Secret: `pihole-api-password` in sops, as Pi-hole's
`FTLCONF_webserver_api_password` and to the gate as a systemd credential.

### Tests

[nix/tests/adblock.nix](../../nix/tests/adblock.nix) and
[adblock.py](../../nix/tests/adblock.py): the API refuses without the
password; Unbound over TLS; the list loads, survives a restart and a
second load; the house is filtered, and so is a device away asking the
box's address on the Commonty network; the fleet's names are not answered
with the house address; the switch, pause, restore after restart and allow list
for the household, and not for a member who lives elsewhere.

### Updating

Pi-hole moves with nixpkgs. After a bump, run the `adblock` test. What we
rely on: the v6 API (`/api/auth`, `X-FTL-SID`, `/dns/blocking`,
`/stats/summary`, `/history`, `/lists`, `/domains/allow/exact`) and the
`adlist` table's columns.

## Chat (llama.cpp)

### What it is

[llama.cpp](https://github.com/ggml-org/llama.cpp)'s `llama-server` runs
the model; [llama-swap](https://github.com/mostlygeek/llama-swap) starts it
when first asked and unloads it after an idle hour. Both are nixpkgs'.
`llama-cpp` is rebuilt with Vulkan and every CPU variant as a loadable
backend (the nixpkgs build has no SIMD at all), so one build runs on any
box and picks the best path there
([nix/modules/llm/llama-cpp.nix](../../nix/modules/llm/llama-cpp.nix)). The
model is pinned by Hugging Face commit and `sha256`: Qwen3.6 35B A3B, Q4.

Web search is [SearXNG](https://github.com/searxng/searxng), nixpkgs'
`services.searx`, on loopback, kept from private addresses
([search.nix](../../nix/modules/llm/search.nix)).

Nothing is patched. Flags do the work: no web UI, no `/slots` (it would
show one person's requests to another), nothing captured or logged,
thinking off unless a request asks.

### Sign-in

```
llm.<domain>/          → gate's Chat page (members and the demo)
llm.<domain>/v1/...    → auth_request, then a socket only nginx may open,
                         into the model servers' own network namespace
llm.<domain>/search    → gate, members only → SearXNG on 127.0.0.1:4196
```

nginx replaces the caller's credential with a key made at boot, which
llama-swap checks. A client with a device token can use `/v1/` too: it is
the OpenAI-shaped API. The demo's prompts are counted at the gate
(`rate:10/60` on its tile: ten an hour each, sixty for every demo
together); everyone's are paced by nginx's `dd_model` zone.
[llm.nix](../../nix/modules/llm/llm.nix) has the vhost.

### Our pages

`/_dd/chat` ([servers/chat/web/chat.js](../../servers/chat/web/chat.js)).
A member's chats are sealed into their own library on their device; the
demo's stay in the tab.

### Data and secrets

Model files in `/tank/models`, named by revision. The key is made at boot
in `/run/dd-llm`; SearXNG's own in `/run/dd-search`. Nothing of what is
asked is kept. Prompts and answers are plaintext to the box while it
answers, and the module labels the box so (`dd.box.plaintext`).

### Tests

[nix/tests/llm.nix](../../nix/tests/llm.nix) and
[llm.py](../../nix/tests/llm.py): a one-megabyte model woken by a real
request; the key required; nothing else on the box reaching llama-swap or
a model server; nothing kept; SearXNG on loopback only, kept from private
addresses.

### Updating

- **The programs** move with nixpkgs. Check the `cmakeFlags` still apply,
  that every `llama-server` flag in the model's `cmd` still exists, and run
  the `llm` test. Then measure on a real box: speed is the reason for the
  build flags and each model's `split`.
- **A model**: change `repo`, `rev`, `file` and `sha256` in
  `dd.llm.models`. It is fetched on the next boot or switch, and the old
  file is deleted a minute after the switch (`dd-models-prune`: only files
  a fetch placed, listed in `/tank/models/.dd-placed`, that no model names
  now).

## Metrics (Thanos)

### What it is

Every box runs Prometheus and node_exporter; a Thanos sidecar ships its
blocks to a garage bucket, and the observe box runs Thanos query and store
over all of them ([thanos.nix](../../nix/modules/observe/thanos.nix),
[metrics.nix](../../nix/modules/observe/metrics.nix)). All from nixpkgs,
unpatched. Grafana is gone; `grafana.<domain>` redirects.

- Prometheus keeps 30 days on each box; the history is in the bucket
  through Thanos.
- Every box runs smartd on its reported disks and writes
  `dd_disk_warnings` and `dd_pool_unhealthy`
  ([disks.nix](../../nix/modules/observe/disks.nix)).
- The observe box's `dd-fleet-alerts` mails any box's disk warnings, an
  unhealthy pool, or a backup older than 26 hours, read from the Thanos
  fleet view, so up to a block's length late
  ([alerting.nix](../../nix/modules/observe/alerting.nix)).

### Sign-in and our page

`metrics.<domain>/` is our page
([servers/observe/web/metrics.js](../../servers/observe/web/metrics.js)),
served by the gate. nginx lets through to Thanos only the read-only calls
the page makes (`query`, `query_range`, `series`, `labels`, a label's
values), behind `auth_request`, with any credential removed
([metrics-ui.nix](../../nix/modules/observe/metrics-ui.nix)). No admin
API, nothing that writes. The demo sees it as a member does, through its
demo door.

### Tests

[metrics](../../nix/tests/metrics.nix) (a box measures itself),
[thanos](../../nix/tests/thanos.nix) (two boxes, one query over both, and
history kept when one is gone) and
[metrics-ui](../../nix/tests/metrics-ui.nix) (the page for whoever is
signed in; only the read-only calls reach Thanos, from someone signed in,
with no session cookie or `Authorization`, against a stand-in that writes
down what it is asked).

### Updating

Moves with nixpkgs. Run the three tests; if Thanos changes its HTTP API, check
the location regex in `metrics-ui.nix` and the page's queries.

## Git (Forgejo)

### What it is

[Forgejo](https://forgejo.org), nixpkgs' `forgejo-lts`, unpatched
([nix/modules/forge/forgejo.nix](../../nix/modules/forge/forgejo.nix)). It
holds the repositories and runs CI.

### Sign-in

Forgejo's reverse-proxy login: nginx asks the gate, and passes the name in
`X-WEBAUTH-USER`, replacing any the client sent. Forgejo makes the account
on first sight; registration and the password form are off. It listens on
a unix socket that only its group (nginx, and our own units) may open,
since CI jobs on the same box could otherwise name themselves. Someone not
signed in, or the demo, comes in as nobody and sees public repositories.
`/user/login` goes to the passkey page.

### Our pages

Every address that is not Forgejo's own (its API, git over https, raw
files, archives, job logs, avatars, account pages) gets our Git page,
`/_dd/git` ([servers/forge/web/git.js](../../servers/forge/web/git.js)),
which draws itself from Forgejo's API.

### Data, tests, updating

Repositories and LFS on `<bulk pool>/enc/forgejo` (`/vault/forgejo`), the
database in postgres. [nix/tests/forge.nix](../../nix/tests/forge.nix)
boots it and checks the admin account and the branch rule on `main`. An
LTS bump comes with nixpkgs; after one, run the `forge` test and look at
the Git pages, since they depend on the API's shapes.

## Smart home (Home Assistant)

### What it is

[Home Assistant](https://www.home-assistant.io), from nixpkgs
(`services.home-assistant`), on a household's home controller: a
Raspberry Pi of theirs, not a box ([home-controller.md](home-controller.md)).
Module: [nix/modules/smarthome/home-assistant.nix](../../nix/modules/smarthome/home-assistant.nix);
the system: [nix/controller](../../nix/controller). Nothing is patched.

What we set instead of patching:

- it listens on 127.0.0.1:8123 alone; the door is the only thing on the
  network;
- no cloud, analytics or mobile app integration; the household's own
  devices' integrations (Zigbee, Matter, Thread, ESPHome, Shelly, Hue,
  HomeKit) are in;
- history kept 30 days (`purge_keep_days`, held to 90 by
  `checks.retention`), written every 30 s so the card wears slower.

### Sign-in

The trusted networks provider, and nothing a person types: the door
connects from each person's own loopback address, and Home Assistant signs
that address in as their user (`trusted_users`, from a file the door
writes). The door's own admin login, from 127.0.0.1, is a password only it
holds.

### Our pages

Home Assistant's own, with the Commonty theme
([servers/smarthome/theme/commonty.yaml](../../servers/smarthome/theme/commonty.yaml))
and the Commonty bar added by the door. The door's own pages (sign-in,
people, pairing) are in `servers/smarthome/web`.

### Data and secrets

Everything is on the controller's locked partition: Home Assistant's
state and history, and the door's (the controller's key, who is in).
No box holds any of it; backups go out locked.

### Tests

`home-controller` (the card locked to the house's Tang, the door, sign-in,
users) and the door's crate tests.

### Updating

It moves with nixpkgs. After a bump, run `home-controller`. What we rely
on: the trusted networks provider (`trusted_users`, `allow_bypass_login`),
the onboarding api and the websocket `config/auth/create`, and the theme
variables in the theme file (unknown ones are ignored, so a rename shows
as Home Assistant's default, not a break).

## Before merging an upgrade

- The pinned ones (Ente, Vaultwarden, a model) change only in their
  module. Every other program here moves when nixpkgs does, so a nixpkgs
  bump is an upgrade of all of them at once: run their tests.
- Rebase patches; never drop a hunk to make one apply.
- Read upstream's notes for anything about sessions, tokens, sign-in or
  migrations. Those are what our patches touch.
- Say in the pull request what was checked by hand. Photos' VM test does
  not build or open Ente's web apps.
