# Releasing the app

What a stranger downloads from `commonty.org/download`: the app for Linux
(`.deb`, AppImage), Windows (installer, and `dd.exe`) and Android (apk).
Every file is built by nix from the commit a `v*` tag points at, by two
boxes that must make the same bytes, chosen as far apart as the fleet
allows, and signed on the laptop. Nothing is built by anyone else's
machines.

## How a release goes

```
dd release publish (the boxes' release) ─► returns; the boxes never wait
  └─ dd release app-next, in the background
       the app's builds at the last v* tag == at main? (same version for both)
       ├─ yes: nothing to do
       └─ no:  tag the next v* ─► the forge
               dd release app --tag vX.Y.Z
                 laptop: evaluate each download's store path and needs
                         pick 2 builders per download (below)
                         sign a build order ─► releases branch: build-order.json
                 each box, every 2 min (dd-build: dd-attest --order):
                         the order verifies against the release key and names me?
                         build it, rebuild what I did not build myself, sign
                         path + closure + bytes with my host key
                         ─► dd-control ─► the agreed log ("built")
                 laptop: read the log through one of its owner's boxes
                         every builder answered, signed, and all agree?
                         fetch the bytes through that box, check the hash
                         sign the apk with the Android key
                         upload to the forge's release of the tag
                         sign app.json with the release key ─► releases branch
  commonty.org/download/<tag>/<file> ─► the forge's release file, as nobody
  home.<domain>/_dd/download          ─► offers what app.json names
```

## Who builds

Each download goes to two boxes, picked by the laptop and named in the
signed order ([`client/cli/src/build_order.rs`](../../client/cli/src/build_order.rs)):

- **Only boxes that can.** A build says what it needs (`needs` on each
  build in the flake: KVM, memory); a box's heartbeat says what it has. A
  box that is not alive, or lacks either, is passed over.
- **Spread, and steady.** Boxes are ranked per download by a hash of the
  download's store path and the box's name (rendezvous hashing): work
  spreads across the fleet as it grows, and the same download lands on the
  same boxes, whose stores already hold most of it.
- **Apart.** The second builder is the one furthest from the first: a
  different owner, house (`siteId`) and region (`regionId`), each counting
  one. Where the fleet cannot give all three - today, two boxes of one
  owner in one house - the most separated pair there is is used, and the
  manifest records how far apart they were (`built.<download>.apart`, 3
  being the rule met).
- **A box that never answers** is passed over and the order made again,
  once, after four hours. A box that answers that it could not build stops
  the release, with its reason.

- **It follows releases on its own.** `app-next` builds nothing to decide:
  it evaluates the four builds at a fixed version, at the last tag and at
  `main`, and a new app is tagged only when they differ. A release that
  changed nothing the app is made of makes no app. Its log is
  `~/.cache/dd/app-release.log` on the machine that published.
- **It never holds the boxes up, and is never a pull request's wait.** The
  builds take the better part of an hour the first time; CI never runs
  them. A pull request runs the app's tests and lint only
  (`checks.tests-app`, `checks.net-check`).
- **By hand:** `dd release app --tag vX.Y.Z` for a tag already on the forge
  (`--dry-run` prints the manifest and uploads nothing), and
  `dd release app-next` for the whole round from wherever `main` is.

## The builds

`legacyPackages.x86_64-linux.appRelease "<version>"` in the flake, one
file per platform under [`nix/app-release/`](../../nix/app-release):

```
linux       linux.nix     .deb + AppImage, built inside Ubuntu 22.04 (a
                          dated package snapshot, in a VM) so they run on
                          any desktop from that year on; nix's own glibc is
                          newer than every distribution's
windows     windows.nix   the installer: Rust for msvc with clang and lld
                          (cargo-xwin) against Microsoft's runtime and SDK,
                          the Go engine as a mingw dll beside the exe, NSIS
dd-windows  windows.nix   dd.exe, the same way
android     android.nix   the apk, both abis, unsigned; gradle offline
                          against android-deps.json
box-installer nix/installer/image.nix  the box installer's drive, which
                          the app's installer window writes
                          (docs/dev/joining.md)
tests       test-linux.nix  the .deb installed on stock Ubuntu 22.04 and the
                          AppImage run on 24.04 without WebKit; each must
                          open its window
```

To build one by hand:

    d=$(nix eval --raw .#legacyPackages.x86_64-linux.appRelease \
          --apply 'r: (r "0.0.1").linux.drvPath')
    nix build "$d^*"

The Linux build and its tests need KVM; every box has it.

## What the boxes are trusted with

Nothing alone. A box builds only what a signed order names it for, from the
commit the order names, and evaluates it itself: it refuses a job whose
path its own evaluation does not reach. `dd-attest` vouches only if every
path under the download was signed by cache.nixos.org, is
content-addressed, was built by that box, or was rebuilt there and came
out the same. Its word is signed by its ssh host key and carried in the
agreed log as that box's request (signed by its TPM). The laptop needs both
builders to agree on the path, the closure and the bytes; one that differs
stops the release. The box the laptop reads the log and fetches the bytes
through is trusted with neither: the results carry their builders'
signatures, and the bytes are held to the hash they agreed on.

The Android key never leaves the laptop: no box could sign an update to
the app on anyone's phone.

## Reproducible, or it does not ship

Two boxes agreeing is only possible because the builds make the same
bytes every time:

- Every tool and download is pinned by hash: the Ubuntu packages (a dated
  snapshot), the AppImage tools and runtime, Microsoft's runtime and SDK
  (CRT 14.44.17.14, SDK 10.0.26100), the NSIS plugin, every maven file
  gradle fetches (`android-deps.json`), every crate and Go module.
- Every file is stamped with one fixed time (`SOURCE_DATE_EPOCH`).
- A build that turns out not to reproduce shows up as the boxes
  disagreeing, and nothing is signed. Check one with
  `nix build --rebuild "$d^*"`.
- **One machine agreeing with itself is not enough.** A rebuild on the same
  box shares its disk, its directory order and its core count; only a
  second box shows what depends on those. v0.1.25 stopped on exactly that:
  the `.deb`'s checksum list came in each disk's directory order, the same
  on every rebuild of one box and different between node1 and node2. To
  check a fix, build it on two boxes and compare (below).

## The record

- **v0.1.25** (release 214, 2026-10-05): the first build order. node1 and
  node2 agreed on the apk, `dd.exe` and the AppImage, and not on the
  `.deb` (its checksum list in each disk's order). Nothing was signed;
  the tag stays unsigned.
- **v0.1.26** (release 216, 2026-10-05): the first app released by the
  fleet. All four builds agreed bit for bit on node1 and node2 (apart 0
  of 3: one owner, one house); the apk signed on the laptop; every file
  served at `commonty.org/download/v0.1.26/` with the hash the release key
  signed.

## Following a release

```
~/.cache/dd/app-release.log            the laptop: tag, order, each answer, upload
releases branch: build-order.json      the order the boxes are working on
journalctl -u dd-build    (on a box)   that box's builds, rebuilds and its answer
curl 127.0.0.1:4195/built (on a box)   the agreed log of answers
```

A box takes up an order within two minutes; each download takes it the
better part of an hour the first time (most of it rebuilding what it
fetched, to compare), much less once it has checked those. The laptop
stops at the first disagreement or failure with the download's name, and
nothing is signed; the tag stays unsigned and the next change makes the
next tag. Nothing is ever re-tagged.

To compare a download on two boxes by hand, from a pushed branch:

    d=$(nix eval --raw 'git+https://git.<domain>/<repo>?ref=<branch>&rev=<sha>#legacyPackages.x86_64-linux.appRelease' \
          --apply 'r: (r "0.0.1").linux.drvPath')
    nix build --no-link --print-out-paths "$d^*" | xargs -I{} sh -c 'sha256sum {}/*'

on each box (with `git` on the path: `nix shell nixpkgs#git -c ...`), and
compare the sums.

## The nightly check

Reproducibility can break without anyone touching the app: a nixpkgs
bump, a new tauri or Android plugin, a crate whose build script stamps the
time. A release would stop on it (the builders disagree, nothing is
signed), but an hour into the builds and with the release held up. So the
laptop checks every night:

```
dd release app-check            the app at main, at version 0.0.1
  the same paths as the last check that passed?  ─► nothing to do (seconds)
  otherwise: a check order (build-check.json, never in the way of a
  release's build-order.json), two boxes build it and vouch, as a release
  agreed ─► logged; the next release will sign
  not    ─► a desktop notification, the timer's unit fails, and
            ~/.cache/dd/app-check.json says which build and why
```

`dd release app-check --install` sets up the timer (03:00, or on waking
if the laptop was asleep then). It runs on the laptop because the check
order is signed with the release key, which lives there.

## Android's key

A phone installs only a signed apk, and takes an update only when it is
signed by the same key as the version it has. The build makes an unsigned
apk; `dd release app` signs it with an Android key kept beside the release
key in the laptop's keystore and publishes it as `commonty.apk`. Without
the key the apk is left out of the manifest, and the page shows Android as
not ready rather than a file no phone can install.

- `dd release android-init` makes the key, once. It prints the
  certificate's SHA-256, which is how anyone recognises the app's signer.
- Losing it means every phone must uninstall and reinstall to take the
  next version, and making a new one has the same effect, so the command
  refuses while one exists. Keep it as safe as the release key.

## The version

Each download is built with its tag's version (`COMMONTY_VERSION`): the app
says it in the Devices tab, the installers carry it, and Android's version
code rises with it, so each release installs as an update.

## Known gaps

- **Separation.** Every box today has one owner, in one house and region,
  so builders are "apart 0 of 3" and the manifest says so. The rule is met
  once a second household's box joins.
- **The Linux download's closure** names several hundred store paths
  (build paths left in the binary's strings), so a box accounts for far
  more than two files. Harmless to the files themselves; trimming it makes
  a release faster.

## Changing it

- **Rust** follows the flake's: Windows and Android take the same version
  from rust-overlay with their targets added.
- **Tauri:** the CLI is nixpkgs' `cargo-tauri`; the crates are in
  `Cargo.lock`. After a tauri, plugin or gradle change, refresh the maven
  lock (it runs the real build behind a recording proxy, about twenty
  minutes):

      nix build --impure --expr '((builtins.getFlake "git+file://'$PWD'").legacyPackages.x86_64-linux.appRelease "0.0.1").android.mitmCache.updateScript'
      ./result

- **A pinned download** (AppImage tools, runtime, NSIS plugin, Microsoft's
  SDK): change the address and the hash together. The SDK's hash comes
  from building it once with a placeholder.
- **The Ubuntu snapshot** comes with nixpkgs' `vmTools`; moving it moves
  every Linux file.
- **A new file:** add its build to `appRelease`, its name to `APP_BUILDS`
  and `app_files` in
  [`client/cli/src/release_cmd.rs`](../../client/cli/src/release_cmd.rs), and
  to the downloads page (`servers/gate/src/pages.rs`).
