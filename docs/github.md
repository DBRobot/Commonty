# GitHub

The forge is where everything happens: pull requests, CI, merges, the
fleet's releases. GitHub holds a mirror of the repository and does one job
the boxes cannot: it builds the app people download, for Linux, Android and
Windows, on machines that have those systems.

## The mirror

The forge pushes every change to `DBRobot/Commonty` on GitHub as it
happens ([`nix/modules/forge/forgejo-mirror.nix`](../nix/modules/forge/forgejo-mirror.nix)),
with a GitHub token kept in `secrets/`. Nothing is ever changed on GitHub
directly: the mirror would overwrite it.

The token must be allowed to push workflows (its **Workflows** permission).
Without it, GitHub refuses any commit that changes a file in
`.github/workflows/`, and the mirror stops at that commit until someone
pushes it over ssh: `git push git@github.com:DBRobot/Commonty.git main`.

The repository was called `DBRobot/Home-Server` until 2026-10-04; GitHub
redirects the old name, and the mirror still pushes to it. GitHub attests
new builds under the new name, which is the one `dd release app` checks.

## The app's release

```
dd release publish (the boxes' release) ─► returns; the boxes never wait
  └─ dd release app-next, in the background
       app/, client/, Cargo.toml/lock or release.yml changed since the last v* tag?
       ├─ no:  nothing to do
       └─ yes: tag the next v* ─► forge, and GitHub over ssh
                 └─ GitHub: release.yml
                      linux (deb, AppImage) · android (apk) · windows (installer, dd.exe)
                      └─ publish: GitHub attests every file, attaches them to a release
               dd release app: checks each file's attestation and commit,
               signs the apk with the Android key, uploads it as
               commonty-signed.apk, signs app.json ─► the downloads page
```

- **It follows every release on its own.** A release whose merges touched
  what the app is built from gets a new app; one that did not, does not.
  The log is `~/.cache/dd/app-release.log` on the machine that published.
- **It never holds the boxes up.** The app takes the better part of half an
  hour to build; the fleet's release is done before it starts. If GitHub's
  build fails, `app-next` stops with a link to the run, and the downloads
  page keeps offering the last app that was signed.
- **By hand:** `dd release app --tag vX.Y.Z` signs a tag GitHub has already
  built, and `dd release app-next` does the whole round from wherever
  `main` is.

## Android's key

A phone installs only a signed apk, and takes an update only when it is
signed by the same key as the version it has. GitHub's build is unsigned:
`dd release app` signs it, on the machine that holds the release key, with
an Android key kept beside it in that machine's keystore, and uploads the
result to the same GitHub release as `commonty-signed.apk`. The downloads
page offers that file. Without the key, the apk is left out of the
manifest and the page shows Android as not ready, rather than offering a
file no phone can install.

- `dd release android-init` makes the key, once. It prints the
  certificate's SHA-256, which is how a phone, or anyone, recognises the
  app's signer.
- Losing it means every phone must uninstall and reinstall to take the
  next version, and making a new one has the same effect, so the command
  refuses while one exists. Keep it as safe as the release key.

## The version

Each bundle is built with its tag's version (`COMMONTY_VERSION`, set by
`release.yml`): the app says it in the Devices tab, the installers carry
it, and Android's version code rises with it, so each release installs as
an update.

## What GitHub is trusted with

The boxes trust no single builder: every box rebuilds a release and checks
it matches before vouching for it. The app is different. Its files are
built once, on GitHub's machines, and trusted because GitHub attests that
they came out of
[`.github/workflows/release.yml`](../.github/workflows/release.yml) at that
tag's commit; `dd release app` checks that statement before the release key
signs anything. The Android key never leaves the laptop: GitHub could not
sign an update to the app on anyone's phone. A Tauri bundle is not reproducible enough to rebuild and
compare, so this is the trust the app runs on, and it is named here so it is
a choice and not an accident.

## How the build stays honest and fast

The same values as [CI](ci.md): what it checks is complete, and it does not
take longer than it must.

- **Pinned.** It builds with the Rust the flake compiles with
  ([`nix/tests/ci.nix`](../nix/tests/ci.nix) fails the build if the two
  differ), one exact `tauri-cli`, actions pinned to commit hashes, and every
  file it downloads (rclone, WinFsp) checked against its hash. A tag rebuilt
  next month makes what it made today.
- **Cached the way the fleet's builds are.** The dependencies are built
  once per `Cargo.lock` and compiler and kept, so a release compiles only
  what changed; `tauri-cli` is built once per version.
- **Whole or not at all.** The release goes out with all five files or
  fails: a missing one is a failed build someone sees, not a greyed-out
  download.
- **Checked before it runs.** The forge's lint runs actionlint and the
  pinned-actions rule on `.github/workflows` too, and a change there runs
  the flake checks.

## Changing it

- **A newer Rust** comes with the flake: when `flake.lock` moves nixpkgs to
  a new `rustc`, `ci.nix` fails until `toolchain:` in `release.yml` says the
  same version. Change all three jobs.
- **A newer tauri-cli:** change `TAURI_CLI` at the top of `release.yml`;
  the cache key follows it.
- **A newer rclone or WinFsp:** change the version and the hash together,
  the hash from the project's own published sums, never one worked out from
  a download alone.
- **A new platform or file:** add its job, add it to `publish`'s list of
  files that must be there, and to `app_files` in
  [`client/cli/src/release_cmd.rs`](../client/cli/src/release_cmd.rs), which
  is what `app-next` waits for.
- Time a change to the build the way [CI](ci.md#adding-to-ci) asks: say in
  the pull request what it costs.
