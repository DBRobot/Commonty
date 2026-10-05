# Agents

This repository is Commonty: NixOS boxes that run a household's services,
the `dd` client, and the identity that no server issues. The boxes are
rebuilt from it, so a change here reaches real machines.

- **How things work: [`docs/`](docs/).** Each page covers one part of the
  system; read the one for the area you are changing before you change it.
  When you change how something works, change its page in the same pull
  request. The same pages are at `<domain>/docs`.
- **Any page, screen or style: [`docs/ui.md`](docs/ui.md).** Read it before
  changing anything a person sees - the site, the app's own screens, Git,
  Docs, Games. Fast first, then cohesive, then quality; the shared pieces
  are in `box/web/kit.js` and `box/web/home.css`.
- **Git, pull requests, CI and releases: [`docs/ci.md`](docs/ci.md),**
  loaded below. Follow it for every branch, commit, pull request, merge and
  release.
- **The app (`app/`): [`docs/app.md`](docs/app.md).** Read it before
  changing the app, the pages it carries, or anything that touches its keys.
- **The app's downloads, how they are built and released:
  [`docs/app-release.md`](docs/app-release.md).** Read it before changing
  `nix/app-release/`, `dd release app`, or anything they pin. The GitHub
  mirror is [`docs/github.md`](docs/github.md).

@docs/ci.md
