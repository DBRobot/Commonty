# The mirror

The forge is where everything happens: pull requests, CI, merges, the
fleet's releases and the app's. GitHub only ever held a mirror of the
repository, and built the app until 2026-10-04, when GitHub suspended the
account; the app is now built by the boxes ([its page](app-release.md)).
Nothing depends on GitHub any more.

## The mirror

The forge pushes every change to `DBRobot/Commonty` on GitHub as it
happens ([`nix/modules/forge/forgejo-mirror.nix`](../../nix/modules/forge/forgejo-mirror.nix)),
with a GitHub token kept in `secrets/`. Nothing is ever changed on GitHub
directly: the mirror would overwrite it.

The token must be allowed to push workflows (its **Workflows** permission).
Without it, GitHub refuses any commit that changes a file in
`.github/workflows/`, and the mirror stops at that commit until someone
pushes it over ssh: `git push git@github.com:DBRobot/Commonty.git main`.

The repository was called `DBRobot/Home-Server` until 2026-10-04; GitHub
redirects the old name, and the mirror still pushes to it. While the
account is suspended the mirror's pushes fail, and nothing else notices.
