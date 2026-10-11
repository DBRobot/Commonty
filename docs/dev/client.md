# The client

`client/` is the code that runs on people's devices, and the code that
holds their keys: the `dd` command, the encrypted git remote, the crates
the app is made of, our Rust compiled to wasm for the pages, and the mail
Worker's checks. A box never holds a key that can sign as a person; every
such key is made and kept by the code in this folder. This page is how
the folder is laid out, how each part works, what it keeps on a device,
and how to build, test and change it.

Every crate here is a member of the one cargo workspace
([`Cargo.toml`](../../Cargo.toml)). The app ([app.md](app.md)) uses the
same crates behind its window. On the boxes, the gate uses `identity`
and `ente`, the library gate and the release agent `identity`, and the
transcoder `library`.

## What runs where

```
a member's computer   dd and git-remote-dd (.#dd), on Linux; dd-windows for Windows
the app               auth, identity, directory, account, media, library
                      (app/Cargo.toml), and the pages with dd_web.wasm
a browser tab         dd_web.wasm (client/web), served by the gate at
                      /_dd/web/dd_web.js: Photos' sign-in, libraries, the
                      Devices tab, Backups
Cloudflare            dd_mail.wasm (client/mail) inside the mail Worker
a box                 dd storage-ledger and dd storage-view (timers), dd
                      photos-demo (once); the crates above inside the gate,
                      the library gate, the transcoder and the release agent
the owner's laptop    dd release, dd member, dd invite, dd mail, dd control,
                      dd secret: the commands that change the fleet
```

## Where things are

```
client/
  auth/          the device key and the keystore
    device.rs      the ed25519 device key; biscuit tokens it signs
    keystore.rs    KeyStore: the OS credential store, or a 0600 json file
  identity/      a person's signed entry: devices, passkeys, libraries, grants;
                 the rules a box applies to an update; invites; pairing sums
  directory/     fetching and publishing entries; resolve(), the one rule for
                 which answer to believe; the proxy and user agent all HTTP shares
  account/       a member's identity from any device: create, admit, remove,
                 recover, link a passkey, redeem an invite, add a library
  library/       a library's key and format
    crypt.rs       rclone's crypt format: names, blocks, block keys
    lib.rs         sealing a library key to every key in an entry, and opening it
  media/         a library from a device
    gate.rs        WebDAV to the library gate, one object at a time
    mount.rs       rclone mount, per library (not on Android)
  archive/       dd image: restic's format (rustic_core) over the gate's WebDAV
  ente/          Ente's own account crates, pinned by commit; gallery.rs reads
                 one account's photos (the demo's, in the gate)
  gitremote/     git-remote-dd: encrypted repositories on an ordinary forge
  web/           dd-web, compiled to wasm for the pages
    lib.rs         Photos' account for a passkey person (ente_login/create/adopt)
    library.rs     a passkey's library key, and a library's crypto in the tab
    entry.rs       a passkey root's entry edits (Devices tab), the QR code
    archive.rs     reading and pruning dd image's archives (Backups)
  mail/          dd-mail: the mail Worker's checks, compiled to wasm
    src/lib.rs     admit (whose entry, a member) and login (their passkey)
    worker/        the Worker itself: worker.js, unlock.js, its pages
    examples/fixture.rs, tests/tester.json   the made-up member its tests sign in as
  cli/           dd
    src/main.rs    the commands, and the smaller ones in place
    src/*.rs       one file per larger command (below)
    tests/         e2e.rs, git.rs, repo.rs, live.rs
```

The crate names are not always the folder's: `client/cli` is crate `dd`,
`client/gitremote` is `git-remote-dd`, `client/web` is `dd-web`,
`client/mail` is `dd-mail`. Cargo's `-p` takes the crate name.

## The device key and tokens

[`auth/src/device.rs`](../../client/auth/src/device.rs). A device is an
ed25519 key made on the device, kept in its keystore, and admitted into
its person's signed entry by their root key. From then on it signs its own
tokens, offline. Its fingerprint is the first six bytes of the public key,
in hex, as the entry shows it.

A token is a biscuit signed by the device key:

```
user("<name>")              who it speaks for
device("<fingerprint>")     which of their devices signed it
check if time($t), $t < exp the expiry is in the token, not the verifier's choice
audience("<host>") + check if here("<host>")
                            mint_at with an audience: good at that one name only
purpose("<op>") + check if operation("<op>")
                            mint_for with an operation: "enrol" links cannot be
                            used to read files, and the other way round
```

`dd token` prints an hour's token with neither audience nor purpose, for
curl and rclone. The gate checks the signature against the devices in the
entry it holds ([gate.md](gate.md#credentials)).

## The keystore

[`auth/src/keystore.rs`](../../client/auth/src/keystore.rs). `KeyStore`
is `get`, `set` and `clear` by account name. `auth::open(service)` picks:

```
DD_KEYRING_FILE set    FileStore: a json file, 0600 from creation, written to a
                       .tmp and renamed. Not encrypted at rest. Tests use it
otherwise              OsKeyring: Secret Service, macOS keychain, Windows
                       credential manager, under service "commonty"
Android                FileStore in the app's private storage (open_file)
```

`dd` reads `DD_KEYRING` for the service name, so one machine can hold a
second device under another name. An entry missing under "commonty" is
copied from the old service name, "distributed-datacenter", on first read.

## Keys on a device

What `dd` keeps in the keystore, by account:

```
device-key          the device's ed25519 key (auth::device::ACCOUNT)
user                the name the device signs for
identity-root       the root key, base64; on the machine that made the
                    identity, or one it was exported to. Signs entry updates
ente                Photos' session: user id, token, master key, secret and
                    public key (cli/src/vault.rs)
archive             dd image's restic password, derived from the photo password
age-key             the age key for the repo's sops files (dd secret)
release-key         the release key: signs releases, invites, build orders
                    and the app's manifest (owner's laptop only)
android-key, android-cert   the key the Android app is signed with
cloudflare-workers  the token that uploads the mail and control Workers
```

And on disk:

```
~/.config/dd/device_ed25519(.pub)   dd git setup: the device key as an OpenSSH
                                    key, 0600, for git's ssh signing
~/.config/dd/allowed_signers         dd git signers: every device in every entry
<repo>/.git/dd/<sha256(url)[..16]>/  git-remote-dd: its clone of the backing
                                     repository, and local.json (the last
                                     counter and commit it verified)
~/.cache/dd/repos/                   git-remote-dd's clones for share and readers,
                                     run outside any repository
~/.cache/dd/app-check.json, app-release.log   dd release's nightly app check,
                                     and the log of an app release it started
```

The paper recovery key is not kept anywhere: `dd identity new` prints it
once. It can sign one thing, an entry that installs a new root, and it is
also the age identity that opens a box's disk-key copy (`dd disk recover`).

## Identity and the directory

[`identity`](../../client/identity/src/lib.rs) is the entry format and its
rules. A person is their root key. Their entry, signed by that root, is
the whole of their account:

```
Entry
  name        lower-case letters, digits, - _ . (64 at most)
  root        base64 ed25519 key, or webauthn:<credential id>... for a
              person whose root is a passkey (an account made in a browser)
  recovery    base64 ed25519 key that may install a new root; empty for a
              passkey root
  devices[]   fingerprint, public_key, added
  passkeys[]  id, cred (webauthn-rs's), added, rp_id, library_key
  grant       an invite redeemed: the signed invite, when, and the code's proof
  libraries[] id, keys (sealed to device:<fp>, root, passkey:<id>, recovery),
              readers, created
  version     strictly increasing
  updated
SignedEntry   entry, signature, recovery_signature
```

`identity::accept(old, new)` is the rule every box applies before storing
an update: signed by the root it names; a newer version; the same root,
or a new one co-signed by the recovery key on file; a passkey root's own
credential never changed or dropped; only the recovery key may change the
recovery key. The gate, the mail Worker and `dd` all call it. The
directory side of it is [gate.md](gate.md#the-directory).

[`directory`](../../client/directory/src/lib.rs) fetches an entry from
every directory a client names and decides what to believe in one place,
`resolve(found, anchor)`: every answer must verify, all must agree on the
root, and then the anchor must hold:

```
Root(key)       the root this machine holds
Members(ids)    the member ids in the signed release
FirstSight      nothing known yet: every directory asked must agree
```

Publishing PUTs the signed entry to every directory; `account::publish`
fails unless every one took it. The default directory is the front door,
`https://home.commonty.org/_dd/directory`; `--directory` (repeatable)
names others. All HTTP from these crates goes through `directory::http()`
or `builder()`: no redirects, a 20 s timeout, the user agent
`dd/<version> (<os>)` that the Devices tab shows, and the app's SOCKS5
proxy into the fleet's network when one is set (`directory::via`).

[`account`](../../client/account/src/lib.rs) is what a member does to
their own entry, shared by `dd` and the app: `create` (root and recovery
made here, this device the first, an invite's grant if a code is given),
`admit` and `remove_device`, `admit_passkey` (which seals every library
to the passkey in the same signed step), `link_passkey`, `remove_passkey`,
`recover` (the paper key co-signs a new root; every other device is
dropped; libraries are opened with the paper key and sealed afresh),
`redeem` and `add_library`. Each fetches the current entry, checks it is
ours (`ours`: the root this machine holds), changes it, signs and
publishes.

## Libraries, media and archives

These have their own page: [Encrypted libraries](libraries.md). In short:

- [`library`](../../client/library) is rclone's crypt format
  ([crypt.rs](../../client/library/src/crypt.rs)) and the key's sealing to
  every key in an entry. A library's key exists only sealed in its
  owner's entry.
- [`media`](../../client/media) opens a library with this machine's
  device or root key (`Opener`), talks WebDAV to the library gate at
  `files.<domain>` with a device token (`Gate`), and mounts each library
  with `rclone mount` (`mount.rs`), with `dd media --token` as the command
  rclone runs for a fresh token. `DD_RCLONE` names the rclone binary; the
  flake's `.#dd` sets it.
- [`archive`](../../client/archive) is `dd image`: restic's repository
  format through `rustic_core`, over WebDAV under
  `https://files.commonty.org/images/`, with a token that renews itself
  on a 401 so an upload outlives the hour. Its password is derived from
  the photo password with HKDF (`cli/src/derive.rs`, label
  `archive password v1`, salt `distributed-datacenter`), so a new machine
  needs `dd unlock` and nothing more. `dd image passkey` seals it to each
  passkey's library key in `dd-passkeys.json` beside the archives, which
  is how Backups in Settings opens them in a browser
  ([libraries.md](libraries.md#disk-images)).

## The encrypted git remote

[`gitremote`](../../client/gitremote/src/main.rs) is a git remote helper.
`git remote add origin dd::<url>` makes git run `git-remote-dd` for that
remote; the url is an ordinary repository on the forge, which holds only
ciphertext on its `dd` branch:

```
keys.json      signers: device keys that may read and sign, each with the
               repository key sealed to it; repo: owner/name, so one
               repository's states cannot pass for another's
manifest.enc   {counter, refs, packs}, encrypted with the repository key
packs/<id>     one encrypted git pack per push: XChaCha20-Poly1305, random nonce
sig.json       {counter, by, keys_sha256, manifest_sha256, signature}
```

A fetch checks the signature against a signer in `keys.json`, refuses a
counter lower than the last one seen (kept in `local.json`), decrypts and
unpacks. A push packs the new objects, encrypts, bumps the counter, signs
with the device key and pushes the `dd` branch. Sharing seals the
repository key to another device's public key
(`dd repo share <url> <key>`, `dd repo readers <url>`). The owner is the
url's path segment before the name, or `DD_REPO_OWNER`. How the forge
sees these repositories is [forge.md](forge.md#encrypted-repositories).

## The pages' wasm

[`web`](../../client/web) is built for `wasm32-unknown-unknown` by crane
and wasm-bindgen into `.#web`, which the gate serves at `/_dd/web/` and
the app carries ([servers.md](servers.md#how-a-page-is-served)). Its
exports, and who imports them:

```
ente_login, ente_create, ente_adopt, seal_for_app
        servers/photos/web/photos.js: Photos for a passkey person, the
        passkey's PRF output as the password; the master key stays in the tab
library_device_key, library_open, path_*, file_*, Sealer, Opener, plain_size,
library_blocks_for_box
        servers/home/web/library.js (Files, Movies & TV), gate/web/enrol.js
entry_with_device, entry_without, entry_signed, qr_svg
        servers/fleet/web/devices.js: a passkey root's entry edits
Archive
        servers/fleet/web/backups.js: dd image's archives, read and pruned
```

It uses the same `identity` and `library` crates as `dd`, so a browser and
the cli cannot disagree on a format.

## The mail Worker

[`mail`](../../client/mail) is what runs at Cloudflare as `mail.<domain>`:
a member's email address (an Email Routing rule), shown and changed after
their passkey, and the unlock Worker's half of each box's disk key. No
box can reach either. `worker/worker.js` is the Worker; `dd_mail.wasm`
does its checks with the boxes' own Rust:

```
admit(name, fresh, pinned, members, release)
        the entry from the directory, held to the newest this Worker has
        pinned (KV) by identity::accept, and its root on the member list
login(entry, id, assertion, challenge, origin)
        a passkey assertion against that entry
```

`.#mail-worker` builds the bundle: the wasm, the worker files, the shared
CSS and fonts, and `fleet.js` with `MEMBERS` (fleet/members.json),
`RELEASE` (fleet/release.pub) and `BOXES` (each box's `unlockKey`), so
the Worker takes none of them from a box. `dd mail setup` makes it once
(its KV namespace, `fleet/mail.json`, the `SESSION_KEY` and `CF_TOKEN`
secrets, the name); `dd release publish` uploads it with every release.
`unlock.js` hands a box its half of the disk key only for a request
signed by its TPM key from its house, or after a member approves it; a
box marked stolen is never answered.

## The dd commands

`dd --help` lists them; each has `--help`. By who uses them:

```
a member, on their device
  signup --username --email        identity and the Photos account, from nothing
  identity new|show|publish|recover|export|import
  device show|admit <key>|remove <fp>
  enrol                            a link that puts a passkey in your entry
  passkey list|link|remove|add
  token                            an hour's device token, for scripts and rclone
  library new|list|key|ls|put|get|mkdir|trash      (librarycmd.rs)
  media [--at DIR]                 every library as folders (mediacmd.rs)
  image init|push|list|pull|delete|repair|passkey  (archive crate)
  unlock --email                   Photos' keys into the keystore
  git setup|signers                commits signed with the device key
  repo share|readers               encrypted remotes (git-remote-dd)
  status, lock [--forget-identity]

the fleet's owner, from a checkout (--repo, default .)
  release init|publish|show|status|sign        (release_cmd.rs, releases.md)
  release app|app-next|app-check|android-init  (app-release.md)
  member add|remove|list           fleet/members.json (member.rs)
  invite [--ttl 5m]                a one-person code, signed with the release key
  box list [--private] | public <box>
  secret init|recipient|run -- <sops args>
  mail setup|deploy                (mail_worker.rs)
  control setup|deploy             (control_worker.rs)
  disk recover <file> <out>        a box's disk key from /boot/dd, with the paper key

a box, from its units
  storage-ledger                   Photos and libraries counted, Photos' limits set
  storage-view                     the fleet's count, from the control plane
  photos-demo                      the demo's read-only Photos account
```

`dd secret run` runs `sops` from PATH with the age key in `SOPS_AGE_KEY`;
without sops on PATH it runs the repository's pinned one
(`nix run .#sops`), never an unpinned binary. `dd release` decrypts
`secrets/fleet.yaml` the same way for the cache writer's key.

## Environment

```
DD_KEYRING          the keystore service name (default commonty): a second device
DD_KEYRING_FILE     a json file instead of the OS store
DD_RCLONE           the rclone binary (set by .#dd's wrapper)
DD_DIRECTORIES      git-remote-dd's directories, comma separated
DD_REPO_OWNER       git-remote-dd's owner, for a backing repository that is a path
DD_CF_API           a stand-in for Cloudflare's API (tests)
CLOUDFLARE_API_TOKEN  the Workers token for one run, instead of the keystore's
DD_BUILD_STORE, DD_ATTEST, DD_RELEASE_FORM   dd release (releases.md)
DD_LIVE_BASE, DD_LIVE_CODE   the live test (below)
```

## Building and running

```
nix build .#dd                     dd and git-remote-dd, rclone wrapped in
nix run .#dd -- status
nix develop                        cargo, clippy, rustfmt, sops, age, fuse3
cargo build -p dd -p git-remote-dd
cargo run -p dd -- --help
nix build .#web                    the pages' wasm bundle
nix build .#mail-worker            the Worker's files, as a release uploads them
```

Against the real fleet a device needs nothing else: the defaults point at
`commonty.org`. To try a change without touching your own keys, give the
run a file keystore: `DD_KEYRING_FILE=$(mktemp -d)/keys.json cargo run -p
dd -- device show`.

On the boxes `dd` comes from `.#dd`, built per crate from `Cargo.nix`
like every box program ([servers.md](servers.md#how-it-is-built)); the
app builds the crates with crane.

## Tests

```
cargo test -p identity -p directory -p library -p dd-web -p dd-mail ...   unit tests
cargo test -p dd --test e2e        the identity model end to end
cargo test -p dd --test repo       git-remote-dd end to end
cargo test -p dd --test git        commits signed and verified through the directory
cargo test -p library --test rclone   the format against rclone (skipped without it)
```

- **Unit tests** in the crates: the entry rules (birth, update, rotation,
  passkey roots, library ids, invites, pairing sums against the
  browser's) in identity; resolve in directory; the format and sealing in
  library; the wasm's library and archive crypto and entry edits in web;
  admit and login against a made-up member in mail; build orders, the
  storage limit, release notes and the disk-key recovery in cli.
- **[e2e.rs](../../client/cli/tests/e2e.rs)** runs real gates in-process
  (`verify::start`) and the real `dd` binary against them, with a software
  passkey for the browser: tokens, devices and recovery, enrol links vs
  access tokens, membership, sign-up in a browser, invite codes, the demo,
  the download page, directories catching up and refusing squats, friend
  links, Photos' code, demo doors, image deletion, sessions, Wi-Fi changes
  and adding a device ([gate.md](gate.md#tests)).
- **[repo.rs](../../client/cli/tests/repo.rs)** pushes and fetches
  through `git-remote-dd` against a plain repository, with a hostile
  forge: an old state refused, a non-fast-forward refused, one repository
  passed off as another refused.
- **[live.rs](../../client/cli/tests/live.rs)** walks the real fleet as
  a guest, and only when pointed at it:
  `DD_LIVE_BASE=commonty.org DD_LIVE_CODE=<a dd invite code> cargo test -p
  dd --test live -- --nocapture`. Without both it passes doing nothing.
- **VM tests** drive `dd` on booted boxes: `directory`, `library`,
  `photos`, `vault`, `network`, `adblock`, `metrics-ui`.
- **`checks.mail-worker`** runs the built Worker under wrangler, against
  a fake Cloudflare and directory, through its pages in a browser.

In CI, `checks.tests` runs every crate's tests, each inside the first box
program whose dependency graph holds it (or as its own root, for a crate
with dev-dependencies or in no box program's graph), with rclone, git and
ssh on hand
and the pages for the gate's ([ci.md](ci.md#how-a-run-fits-together)).
`checks.fmt` and `checks.clippy` (`-D warnings`) cover the folder too. A
change under `client/` turns on the rust part; under `client/identity` or
`client/auth`, the VM tests as well.

## Changing it

- **The entry format.** A new field in `Entry` is
  `#[serde(default, skip_serializing_if = ...)]`, so entries signed before
  it still verify: the signature is over the serialised bytes, field
  order included. A new rule goes in `identity::accept`, with a case in
  its tests and in e2e.rs, and reaches the boxes, the mail Worker (the
  next release), the app and the wasm at once.
- **Which answer to believe** is `directory::resolve`. Do not add a
  second rule somewhere else; call it with the right anchor.
- **A new dd command**: a variant in `Command` in main.rs with its doc
  comment (that is its `--help`), its work in its own file when it is more
  than a screen, and a test in e2e.rs if it talks to a box. Use
  `auth::open(&service())` for keys and `directory::http()` for HTTP.
- **A new keystore account**: a `const` beside its users, and a line in
  the list above. `dd lock` clears `device-key`, `user`, `archive` and
  `ente`; decide whether yours belongs there.
- **A new wasm export**: `#[wasm_bindgen]` in client/web, then the page
  that imports it from `/_dd/web/dd_web.js`. The `wasm-bindgen` crate
  version is pinned to the one nixpkgs' `wasm-bindgen-cli` speaks; the two
  move together.
- **The mail Worker**: worker.js and its pages, and `checks.mail-worker`
  (nix/tests/mail-worker) for what it must refuse. The fixture member is
  regenerated with `cargo run -q -p dd-mail --example fixture >
  client/mail/tests/tester.json`.
- **A dependency** changes `Cargo.lock`; then `nix run nixpkgs#crate2nix
  -- generate` ([servers.md](servers.md#how-it-is-built)).

## Pitfalls

- **The domain is in the code.** `DEFAULT_ENTE`, `DEFAULT_IMAGES`,
  `DEFAULT_ENROL` (cli/src/main.rs), `directory::DEFAULT`, git-remote-dd's
  `DEFAULT_DIRECTORIES`, and `dd release`'s `DEFAULT_URL` and
  `DEFAULT_CACHE` (node1's fleet address) all say `commonty.org` or node1.
  `fleet/policy.json`'s `domain` does not reach them.
- **git-remote-dd always uses the service "commonty".** It honours
  `DD_KEYRING_FILE` but not `DD_KEYRING`, so a second device made with
  `DD_KEYRING` cannot push as itself.
- **The derived passwords depend on the old name.** `derive.rs` uses the
  salt `distributed-datacenter`; changing it changes every archive
  password.
- **A token without an audience works everywhere** the person is known.
  Code that makes a token for one call should use `mint_at` with the host.
- **The keystore is a file in tests.** e2e.rs and the VM tests set
  `DD_KEYRING_FILE`; a test that forgets it writes to the developer's own
  keyring.
- **A shared library is not shared yet.** `readers` exists in the format
  and the gate honours it; nothing writes one.
