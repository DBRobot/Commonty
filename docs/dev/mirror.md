# The GitHub mirror

The forge is where everything happens: pull requests, CI, merges, the
fleet's releases and the app's. GitHub only ever held a mirror of the
repository, and built the app until 2026-10-04, when GitHub suspended the
account; the app is now built by the boxes ([its page](app-release.md)).
Nothing depends on GitHub any more.

## The mirror

`dd release publish` pushes the released commit to each copy in
`forgeMirrors` ([`fleet/policy.json`](../../fleet/policy.json)) as the last
thing it does, from the machine that publishes, over ssh with the owner's
own GitHub key ([`client/cli/src/mirror.rs`](../../client/cli/src/mirror.rs)).
A GitHub address is turned into its ssh one. Only the released branch goes,
never forced: a copy that has gone its own way is reported, not overwritten.
A push that fails (no key on this machine, the account suspended) is
reported and never holds the release up; `dd release mirror` pushes again.

No box holds a GitHub credential. The forge used to push the copy itself
with a token kept in `secrets/`, which any box running the forge could have
used to write to GitHub as the owner. Now the forge keeps no push mirror:
[`forgejo-mirror.nix`](../../nix/modules/forge/forgejo-mirror.nix) removes
any to a listed copy, and the token Forgejo held inside it with it.
Nothing is ever changed on GitHub directly: the next release overwrites it.

The repository was called `DBRobot/Home-Server` until 2026-10-04; GitHub
redirects the old name, and the mirror still pushes to it. While the
account is suspended the mirror's pushes fail, and nothing else notices.
