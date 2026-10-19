# CI and pull requests

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
[`.forgejo/workflows/entry_point.yml`](../../.forgejo/workflows/entry_point.yml),
so one job can wait on every other.

```
pull request opened, pushed, or edited
│
├─ files_modified ── what changed: nix, rust, vm, app (and: a draft? already passed?)
│   ├─ flake_check ─── the flake evaluates; plaintext labels, box list, needs units, this file
│   ├─ mail_worker ─── the mail, DNS and id Workers and the control plane, end to end
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
  ([stop-run](../../.forgejo/actions/stop-run/action.yml)), so running things
  in parallel costs nothing when something is wrong.
- **ci_ok** is the one check `main`'s rule asks for. It runs whatever the
  other jobs did and passes only if each one passed or was skipped because
  its part did not change. [`nix/tests/ci.nix`](../../nix/tests/ci.nix) fails
  the build if a job is missing from its list, so a new job always counts.
- **Jobs run on the boxes**, with nix already there, and build into each
  box's own store. What a run builds is already on the boxes when the
  release goes out, and `dd release publish` signs the paths CI recorded
  instead of building them again. The newest build of each box's system
  is kept from each box's garbage collection (`~/ci-roots/<box>` of the
  runner), so a run after a collection does not build it all again.
- **Jobs run where they fit.** A job heavier than a runner's usual one is
  a variant in the runner's needs
  ([`nix/modules/forge/needs.nix`](../../nix/modules/forge/needs.nix)):
  today `flake-check`, which holds 4G: one evaluation worker at its peak.
  [`nix/lib/ci-runners.nix`](../../nix/lib/ci-runners.nix) fits each
  variant to the runner boxes whose reported memory holds the runner's own
  use and the variant's, and files_modified sends `flake_check` to the first
  of them (`flake_check_on`), or to any runner when none fits. No box is
  named in the workflow.
- **Everything is evaluated, a check at a time.** `flake_check` runs
  [nix-eval-jobs](https://github.com/nix-community/nix-eval-jobs) over
  `legacyPackages.x86_64-linux.ciEval` (every check, package, dev shell
  and box system), each in a worker that starts afresh past 1.5G, one
  worker for every 16G the box has (one to four). A single `nix flake
  check` evaluator grew to 10.5G and took node2 out of memory beside a VM
  test; one worker peaks near 3G, two near 5G. An attribute that fails to
  evaluate is named in the job's log.
- **The tests are cached crate by crate.** `checks.tests` is a linkFarm
  of every workspace crate's test run but the app's (its tests are
  `tests-app`). Each crate's tests are their own build from
  [`Cargo.nix`](../../Cargo.nix), made inside the first box program whose
  graph holds that crate (`testHosts` in [`flake.nix`](../../flake.nix)),
  against that program's own per-crate builds, so a crate whose code and
  dependencies did not change is a cache hit, its tests too. A crate with
  dev-dependencies is tested as its own root. A change to the cli alone
  reruns only dd's tests. A test build compiles unoptimised, as `cargo
  test` does; the box programs' own builds it shares stay optimised.
- **Lint is cached the same way.** `checks.clippy` is every crate's lint:
  its test build is compiled with clippy standing in for rustc (`-D
  warnings`, the crate's own code; its dependencies are the shared builds),
  so lint and tests are one build, and its plain build is linted once more
  for what only exists outside tests (`cargo clippy --all-targets`, a crate
  at a time). The app's checks keep no target dir
  (`doInstallCargoArtifacts = false` on clippy-app and tests-app): keeping
  it made CI upload a 198M target to the cache after every run.
- **A run killed from outside** (the forge or a runner restarting under it,
  as a release can) is run again once, by
  [`ci-resume.py`](../../nix/modules/forge/ci-resume.py) on the forge box.

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
   read the release's number from what `dd release publish` prints. When
   the app would come out different from its last tag, publish also
   starts `dd release app-next` in the background: it tags the next `v*`,
   has two boxes build the downloads and vouch for them, and signs them
   for the downloads page. The boxes' release never waits for it; its log
   is `~/.cache/dd/app-release.log`. The whole of it is
   [its own page](app-release.md).
6. **Wait on `ci_ok`**, not on every status being green: a failure anywhere
   is final, since the run stops at the first one.

## What blocks a merge

`main` takes a pull request when:

- `ci_ok` passed on its latest commit, and
- the branch is up to date with `main`, so what merges is the tree CI ran
  on, not a green branch on an old base.

The **style** check ([`.forgejo/checks/style.py`](../../.forgejo/checks/style.py))
blocks on anything it finds:

- the title is `<type>: <what it does>`, where type is one of `feature`,
  `service`, `fix`, `security`, `ci`, `docs`, `refactor`, `deps`, `chore`.
  A ship script that merges and publishes puts `release:` in front; `WIP:`
  in front makes it a draft.
- the body fills in the sections of
  [the template](../../.forgejo/pull_request_template.md) its type needs.
- no fixup or merge commits inside the branch, the right author, and no
  tool named as an author.
- in every file the pull request touches: no conflict markers, no trailing
  spaces, a final newline, nothing over 1 MiB, no build leftovers, JSON,
  YAML and TOML that parse, secrets still encrypted, Python and Nix
  formatted. A file you touch is checked whole, so an old file is cleaned
  up the first time someone edits it, and never holds up anyone else.

The **lint** check ([`.forgejo/checks/lint.py`](../../.forgejo/checks/lint.py))
blocks only on real faults: a workflow error, a shell script that cannot
run, an undefined name, a JavaScript error, a secret in a new commit, an
action not pinned to a commit hash, a vulnerable crate the pull request
brings in, a docs link that goes nowhere. Everything else (style notes,
spelling, advisories already in the lockfile, old flake inputs) is a yellow
`lint/warnings` status that blocks nothing. It checks what changed between
`BASE` and `HEAD`; run by hand without them, it checks your commits since
they left `forge/main` (uncommitted edits are not seen).

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
- **A new crate's tests come along by themselves.** Every workspace
  member's tests are in `checks.tests`. A test that needs a file outside
  its crate, or a program from another crate, says so in `flake.nix`
  (`crateTest`, or the crate's override in `crateOverrides`), or it will
  not find it in the per-crate build. Today that is `HOME`,
  `VERIFY_PAGES_DIR` (the pages, for the gate's tests and dd's e2e, which
  runs a gate), git, ssh and rclone on PATH, and for dd the mail Worker's
  tests beside the cli, git-remote-dd beside dd, and `CARGO`.
- **A new unit belongs to a service.** Every systemd service and timer our
  modules put on a box is named by some service's needs (its name, or one
  of its `units`); `checks.needs-units`
  ([`nix/tests/needs-units.nix`](../../nix/tests/needs-units.nix)) fails the
  flake otherwise, so placement and the fit check see everything a box runs.
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

## Writing VM tests that don't flake

A VM test that fails one run in five blocks every pull request behind it,
and is soon rerun until it passes, which hides the day it fails for a real
reason. Most of ours that did so waited in one of four wrong ways. The
right ways are in [`nix/tests/lib/waits.py`](../../nix/tests/lib/waits.py);
a test gets them with `helpers = true;` in its `.nix` and `from waits import
...` in its script (`nix/lib/vm-test.nix` hands the folder to the driver's
python, its type check too).

```
start_fresh(box, unit)          run a oneshot once more and wait for that run to end;
                                a run already under way is waited out, never joined.
                                Returns the run's invocation id. Fails if the run fails.
journal_of(box, unit, inv)      that run's journal lines, no other run's
journal_has(box, unit, inv, s)  whether that run wrote s
wait_unit_active(box, unit)     a long-running unit up
wait_socket(box, port | path)   something listening on a port or a unix socket
fetch_has(box, url, s)          whether url's body holds s (curl to a file, then grep -c)
wait_fetch_has(box, url, s)     wait until it does
```

What `lint` refuses on lines a pull request adds to any `.py` under
`nix/tests` (the VM scripts, and the browser flows and fakes in its
folders)
(`vm_test_waits` in [`lint.py`](../../.forgejo/checks/lint.py)); lines
already there are moved over when their test is next touched:

- **`| grep -q`.** grep leaves at its first match, the command writing into
  the pipe dies of SIGPIPE with the rest unwritten, and the pipe fails on a
  page that held the word. Vault's `curl -sf .../use-app.js | grep -qF
  'commonty://open?'` failed with curl's exit code 23 on 2026-10-18 for
  exactly this. Use `fetch_has`, or write to a file and `grep -c` it.
- **`time.sleep(`, and Playwright's `wait_for_timeout(`.** A fixed pause is
  either too short on a loaded runner or too long everywhere else. Wait for
  the thing: `wait_until_succeeds`, `wait_unit_active`, `wait_socket`; in a
  browser flow `wait_for_selector`, `wait_for_function` or `expect`. One
  that must stay (a polling loop's own beat, a device that ignores input
  for a moment) says why on its own line: `# sleep-ok: <reason>`. When
  ruff format wraps the call, the comment may sit on any of its lines.
- **`systemctl start X` then `journalctl -u X | grep ...`.** The unit's
  journal holds every run's lines. Store's read-back check (`journalctl -u
  dd-store-copy.service | grep -q 'read back is not what was sent'`, failed
  2026-10-19) read a run started by its timer, not the one the test
  started, and its facts check raced a run a timer began while the test
  was still setting up. Forge-follow's handover waited on a journal line a
  previous run had written. `start_fresh` runs one of its own and
  `journal_of` reads only it.
- **A journal grep in `succeed()` just after a start.** The line may not be
  written yet, or may be an earlier run's. `start_fresh` and `journal_has`,
  or `wait_until_succeeds` if it truly comes later.

`start_fresh` can't wait on an instance of a template unit, such as
`dd-store-copy-final@forgejo`. Nothing holds an instance once its run ends,
so systemd unloads it, and `systemctl show` then gives a start timestamp of
0 and no invocation: `start_fresh` waits for a run of its own until it times
out. No timer starts such an instance, so no run of it can be under way to
be joined. Start it with `systemctl start` and judge it by its exit status,
and make the unit fail when it didn't do its job (forge-follow's and
takeover's `hand_over`).

`checks.waits` holds the helpers to it: a oneshot that a timer starts every
three seconds, each `start_fresh` checked to be a run of its own with only
its own lines; `fetch_has` on a 2 MB page with the word at its start, twenty
times; a socket that comes up late.

## When a release does not land

A box switches to a release and keeps it only if it comes up healthy. A
unit that fails while the switch is running fails the switch, and the box
goes back to what it ran before. So a unit that runs on a timer must never
fail during a switch:

- its first run is a few minutes after its timer starts (`OnActiveSec`,
  not `OnBootSec`, which fires at once on the switch that adds it);
- a switch does not restart it (`restartIfChanged = false`);
- something it needs not answering yet (the forge restarting under the
  same switch, a file not published yet) is "next time", exit 0, never a
  failure.

`checks.needs-units` fails the flake on any timer of ours that uses
`OnBootSec`, so the first of these is held before it merges.

A switch can also leave a calendar timer active with no next run
(`systemctl list-timers` shows NEXT as `-`): node2's nightly backup timer
did, in the switches of 2026-10-12, and no backup ran for five days.
`dd-timer-watch` (box/timer-watch.nix) restarts any such timer within the
hour on every box and counts it (`dd_timers_restarted`), and the
stale-backup check backs up again before it mails anyone
(storage/backup.nix).

Releases 212, 213 and 224 rolled back for exactly this (`dd-build`,
`dd-ci-resume`, and `dd-storage-view`, on `OnBootSec`, hitting dd-control
restarting under the same switch). When a box stays on an old release, its
`journalctl -u dd-agent` names the switch that failed, and
`journalctl --since <then> | grep "Failed to start"` names the unit.

## When a run is red

- **Read the first failure.** The run stops at the first failed job, so the
  rest show "Has been cancelled"; only the one that failed has the reason.
- **style or lint** list what to fix, by file and line. Run the same script
  locally with the pull request's base:
  `BASE=$(git rev-parse forge/main) HEAD=$(git rev-parse HEAD) TITLE='…' BODY='…' nix shell .#ci-tools -c python3 .forgejo/checks/style.py`.
- **same_build** means the two boxes got a different store path for one
  system: they evaluated the commit differently (a different input, an
  impure evaluation), or one recorded nothing. It compares paths only, and
  a system's path is fixed by its derivation, not by the bytes it builds,
  so it does not catch a build that is not reproducible. That is dd-attest's
  at publish: two boxes must report the same closure digest
  ([releases.md](releases.md#dd-attest-accounting-for-every-path)).
- **"context canceled"** in a log means the run was killed from outside,
  not that the code failed. ci-resume runs it again within a few minutes.
- **Never merge around a red run.** If a check is wrong, fix the check in
  its own pull request.
