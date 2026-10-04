# CI

Every change to `main` arrives as a pull request, and every pull request is
checked by the forge's CI before it can merge. This page is what CI is for,
how it is put together, and how to change it without making it worse.

## What CI is for

Two things, held together:

- **Complete.** Anything that can break a box is checked before it merges:
  the flake evaluates, every box's system builds, the boxes boot as VMs and
  are driven through their failure cases, the Rust is formatted, lint-clean
  and tested. A green run means the release is safe to publish. If
  something broke on a box and CI was green, CI is missing a check, and the
  fix includes one.
- **Fast.** A run is something people wait on, many times a day. It should
  never take a long time, and it should get its answer as early as it can.
  A slow check is not free because it is thorough: slow CI gets batched,
  skipped, or worked around, and then it checks nothing.

When the two pull against each other, the answer is almost never to drop a
check. It is to make the check cheaper: run it only when its inputs changed,
cache it by content, split it so it runs alongside the others, or move the
slow part somewhere it is done once.

## How a run fits together

Everything is one workflow,
[`.forgejo/workflows/entry_point.yml`](../.forgejo/workflows/entry_point.yml),
so one job can wait on every other.

```
pull request opened, pushed, or edited
│
├─ files_modified ── what changed: nix, rust, vm, app (and: a draft? already passed?)
│   ├─ flake_check ─── the flake evaluates; placement, box list, this file
│   ├─ mail_worker ─── the mail Worker and the control plane, end to end
│   ├─ build_part ──┬─ build_host (every box's system, built on every box)
│   │               │    └─ same_build (both builds of a system are one path)
│   │               └─ vm_tests (one job per test)
│   ├─ rust_lint · test · app_lint
│   └─ lint ────────── real faults block; the rest is a yellow warning
├─ style ───────────── the title, the body, the commits, the files touched
│
└─ ci_ok ───────────── every job passed or was rightly skipped
```

- **files_modified** decides what the change can affect, from the paths it
  touches, and every other job skips itself when its part did not change. A
  skipped job still reports, so the run always has an answer.
- **The jobs run side by side.** Nothing waits on another job unless it
  needs that job's output (the VM tests need the programs `build_part`
  built). A failure anywhere stops the whole run at once
  ([stop-run](../.forgejo/actions/stop-run/action.yml)), so running things
  in parallel costs nothing when something is wrong.
- **ci_ok** is the one check `main`'s rule asks for. It runs whatever the
  other jobs did and passes only if each one passed or was skipped because
  its part did not change. [`nix/tests/ci.nix`](../nix/tests/ci.nix) fails
  the build if a job is missing from its list, so a new job always counts.
- **Jobs run on the boxes**, with nix already there, and build into each
  box's own store. What a run builds is already on the boxes when the
  release goes out, and `dd release publish` signs the paths CI recorded
  instead of building them again.
- **A run killed from outside** (the forge or a runner restarting under it,
  as a release can) is run again once, by
  [`ci-resume.py`](../nix/modules/forge/ci-resume.py) on the forge box.

## Making a change

The same steps for everyone, a person or an agent:

1. **Work in your own worktree**, on a new branch from the forge's `main`:
   `git worktree add ../Home-Server-<task> -b <branch> forge/main`. Never
   switch branches in someone else's checkout: an editor watching it shows
   a checkout as edits.
2. **Commit as the repository's author**
   (`git -c user.email=davidsprojects7@gmail.com commit ...`), and never
   name a tool as an author: no `Co-Authored-By`, "Generated with" or
   session links, in commits or in pull requests.
3. **Open the pull request as a draft** (`WIP:` in the title) while it is
   being worked on, with a title and body the style check accepts.
4. **One release at a time.** Everything from taking the draft out to the
   release landing on the boxes happens inside the release lock:
   `exec 9>~/.cache/dd-release.lock; flock 9` ... `exec 9>&-`. That covers
   the full CI run, rebasing onto `main` right before merging (the rule
   refuses an outdated branch anyway), the merge, `dd release publish`,
   and waiting until every box runs the new release.
5. **Publish with the dd built from what merged**, not an older build, and
   read the release's number from what `dd release publish` prints.
6. **Wait on `ci_ok`**, not on every status being green: a failure anywhere
   is final, since the run stops at the first one.

## What blocks a merge

`main` takes a pull request when:

- `ci_ok` passed on its latest commit, and
- the branch is up to date with `main`, so what merges is the tree CI ran
  on, not a green branch on an old base.

The **style** check ([`.forgejo/checks/style.py`](../.forgejo/checks/style.py))
blocks on anything it finds:

- the title is `<type>: <what it does>`, where type is one of `feature`,
  `service`, `fix`, `security`, `ci`, `docs`, `refactor`, `deps`, `chore`.
  A ship script that merges and publishes puts `release:` in front; `WIP:`
  in front makes it a draft.
- the body fills in the sections of
  [the template](../.forgejo/pull_request_template.md) its type needs.
- no fixup or merge commits inside the branch, the right author, and no
  tool named as an author.
- in every file the pull request touches: no conflict markers, no trailing
  spaces, a final newline, nothing over 1 MiB, no build leftovers, JSON,
  YAML and TOML that parse, secrets still encrypted, Python and Nix
  formatted. A file you touch is checked whole, so an old file is cleaned
  up the first time someone edits it, and never holds up anyone else.

The **lint** check ([`.forgejo/checks/lint.py`](../.forgejo/checks/lint.py))
blocks only on real faults: a workflow error, a shell script that cannot
run, an undefined name, a JavaScript error, a secret in a new commit, an
action not pinned to a commit hash, a vulnerable crate the pull request
brings in, a docs link that goes nowhere. Everything else (style notes,
spelling, advisories already in the lockfile, old flake inputs) is a yellow
`lint/warnings` status that blocks nothing.

## Drafts

A pull request whose title starts with `WIP:` is a draft. It runs `style`
and `lint` only, builds nothing, and cannot merge. Push to it as often as
you like. Taking `WIP:` out of the title is what starts the full run. An
edit to the title or body of a pull request whose commit already passed
reruns `style` alone.

## Adding to CI

Every check added has to earn its place on both counts: it catches
something real, and it does not make the run slow. Before it merges:

- **It runs only when it can matter.** Gate it on what `files_modified`
  says changed. A check that runs on every pull request must be one that
  every pull request can break.
- **It is cached by content.** Prefer a nix derivation (a check in the
  flake) over a script: a run whose inputs did not change gets the last
  answer from the store instead of working it out again.
- **It runs alongside the others.** Add `needs:` only for an output it
  really uses. A new VM test is its own job in the `vm_tests` matrix, never
  a step in someone else's.
- **It stops the run when it fails.** End the job with the stop-run step,
  like the others.
- **It is in `ci_ok`'s list.** `nix/tests/ci.nix` will refuse the change
  otherwise.
- **It has been timed.** Run it on a warm cache and a cold one and say what
  it costs in the pull request's "How it was tested". A check that makes
  the run noticeably longer needs a reason it cannot be cheaper, and the
  pull request that adds it is where that is argued.
- **It blocks only on real faults.** Anything that is a matter of taste is
  a warning, not a failure.

Taking a check out needs the same care in reverse: say in the pull request
what it was catching and why that is no longer needed, or what catches it
now.

## When a run is red

- **Read the first failure.** The run stops at the first failed job, so the
  rest show "Has been cancelled"; only the one that failed has the reason.
- **style or lint** list what to fix, by file and line. Run the same script
  locally with the pull request's base:
  `BASE=$(git rev-parse forge/main) HEAD=$(git rev-parse HEAD) TITLE='…' BODY='…' nix shell .#ci-tools -c python3 .forgejo/checks/style.py`.
- **same_build** means the two boxes built a system differently: a build
  that is not reproducible. The boxes would refuse to vouch for it after
  the merge, so it is fixed before.
- **"context canceled"** in a log means the run was killed from outside,
  not that the code failed. ci-resume runs it again within a few minutes.
- **Never merge around a red run.** If a check is wrong, fix the check in
  its own pull request.
