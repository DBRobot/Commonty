# Agents

This repository is Commonty: NixOS boxes that run a household's services,
the `dd` client, and the identity that no server issues. The boxes are
rebuilt from it, so a change here reaches real machines.

- **How things work: [`docs/`](docs/).** Each page covers one part of the
  system; read the one for the area you are changing before you change it.
  When you change how something works, change its page in the same pull
  request. The same pages are at `<domain>/docs`.
- **Any page, screen or style: [`docs/dev/ui.md`](docs/dev/ui.md).** Read it before
  changing anything a person sees - the site, the app's own screens, Git,
  Docs, Games. Fast first, then cohesive, then quality; the shared pieces
  are in `servers/web-shared/kit.js` and `servers/web-shared/home.css`.
- **Git, pull requests, CI and releases: [`docs/dev/ci.md`](docs/dev/ci.md),**
  loaded below. Follow it for every branch, commit, pull request, merge and
  release.
- **The app (`app/`): [`docs/dev/app.md`](docs/dev/app.md).** Read it before
  changing the app, the pages it carries, or anything that touches its keys.
- **The app's downloads, how they are built and released:
  [`docs/dev/app-release.md`](docs/dev/app-release.md).** Read it before changing
  `nix/app-release/`, `dd release app`, or anything they pin. A change to
  how any download is built is checked on two boxes, not one (it says
  how).
- **Other sessions working here too: the `sessions` skill**
  ([`.claude/skills/sessions/SKILL.md`](.claude/skills/sessions/SKILL.md)).
  Look at the board and claim your paths before you start; queue for the
  release lock; hand off by short messages; no secret ever crosses.
- **A unit on a box that runs on a timer** must never fail a switch:
  [`docs/dev/ci.md`](docs/dev/ci.md#when-a-release-does-not-land).

@docs/dev/ci.md
