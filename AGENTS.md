# Agents

This repository is Commonty: NixOS boxes that run a household's services,
the `dd` client, and the identity that no server issues. The boxes are
rebuilt from it, so a change here reaches real machines.

- **How things work: [`docs/`](docs/).** Each page covers one part of the
  system; read the one for the area you are changing before you change it.
  When you change how something works, change its page in the same pull
  request. The same pages are at `<domain>/docs`.
- **Git, pull requests, CI and releases: [`docs/ci.md`](docs/ci.md),**
  loaded below. Follow it for every branch, commit, pull request, merge and
  release.
- **The app's build and release on GitHub, and the mirror:
  [`docs/github.md`](docs/github.md).** Read it before changing
  `.github/`, the app's release, or anything it pins.

@docs/ci.md
