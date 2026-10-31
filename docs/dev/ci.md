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
├─ files_modified ──── what changed, and so which jobs and checks run
│   ├─ flake_check ───── the flake evaluates; plaintext labels, box list, needs units, this file
│   ├─ inputs_kept ───── every job's keep step roots the builds a GC made CI redo
│   ├─ worker_tests ──── the mail, DNS, id and control Workers under wrangler (one job per check)
│   ├─ page_tests ────── the Mail and Files pages, links staying in their tab (one job per check)
│   ├─ site_checks ───── the site through nginx and the gate: headers, roles, a11y, speed, links
│   ├─ build_programs ┬─ build_systems (every box's system, built on every box)
│   │                 │    └─ systems_agree (both builds of a system are one path)
│   │                 └─ vm_tests (one job per test)
│   ├─ rust ──────────── fmt, clippy, the workspace's tests
│   ├─ app_checks ────── the app's lint and tests, its network engine's
│   ├─ build_installer ─ the box installer's drive, built and --rebuilt on every runner box
│   │    └─ installer_agrees (every box made the same bytes)
│   └─ lint ──────────── real faults block; the rest is a yellow warning
├─ style ─────────────── the title, the body, the commits, the files touched
│
└─ ci_ok ─────────────── every job passed or was rightly skipped
```

## The jobs, and when each runs

"Chosen" means job by job, by what each tests (Choosing the tests, below):
a job whose checks were all left out skips, and the skip still counts as a
pass for `ci_ok`. "Main's alone" means main's full run runs it and a pull
request never does. A draft runs `style` and `lint` alone; an edit to a
commit that already passed runs `style` alone.

```
files_modified    always. Says what each job runs (nix/tests/ci-select/plan.py).
style             always (a draft too). The title, body, commits, files touched.
lint              unless the commit already passed. Real faults block.
flake_check       chosen. Evaluates every check and every box's system
                  (nix-eval-jobs; in main's full run every package and dev
                  shell too), then builds the chosen evaluation-only checks
                  (registry eval, and the downloads' recipes, app-release).
inputs_kept       chosen. nix/tests/ci-keep.nix: each job's keep step
                  (keep-inputs.sh) names the builds a box's GC made CI do
                  again (the cross rustc, crane's dependency builds, crates).
worker_tests      its chosen checks: mail-worker, dns-worker, sso, control,
                  control-relay, control-upgrade, control-together,
                  control-signfail.
page_tests        its chosen checks: mail-page, files-page, media-page, same-tab,
                  entry-pages.
site_checks       its chosen checks: site-headers, site-roles, site-a11y,
                  site-speed, site-links.
build_programs    a VM test was chosen. dd and the pages (cli); verify, agent, games,
                  computers, transcode (boxes): what the VM tests carry,
                  built once per box.
build_systems     a pull request: the systems its change reaches that are not
                  main's alone (the home controller's), each once on any
                  runner. Main's full run: every box's system on every runner
                  box, recorded as build/<box>@<runner>.
systems_agree     main's alone. Both runners' paths for a system are one;
                  records build/<box>, which dd release publish signs.
vm_tests          the chosen VM tests, one job each.
rust              chosen: fmt, and each chosen crate's clippy and tests
                  (rustChecks.<crate>); every crate in main's full run.
app_checks        main's alone (main-only.json). clippy-app, tests-app, net-check.
build_installer   main's alone, when the installer was chosen. The
                  box installer's drive, built and --rebuilt on each runner,
                  recorded as installer@<runner>.
installer_agrees  with build_installer. Every runner made the same bytes.
ci_ok             always. Every job above passed or was rightly skipped.
```

- **files_modified** decides what the change can affect, from the paths it
  touches, and every other job skips itself when its part did not change. A
  skipped job still reports, so the run always has an answer.
- **The jobs run side by side.** Nothing waits on another job unless it
  needs that job's output (the VM tests need the programs `build_programs`
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
  `legacyPackages.x86_64-linux.ciEval` in main's full run (every check,
  package, dev shell and box system) and `ciEvalPR` in a pull request's
  (every check and box system: a module change that breaks a test box's
  evaluation is found there even when that test does not run), each in a
  worker that starts afresh past 1.5G, one
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
- **A release never kills a job.** A switch leaves a runner alone, and
  it restarts into a new config only once it holds no job
  ([forge.md, The runners](forge.md#the-runners)).
- **A job killed from outside** (the forge restarting or moving under it,
  a runner stopped or its box gone) is run again by itself, with the jobs
  that need it and `ci_ok` after it, by
  [`ci-resume.py`](../../nix/modules/forge/ci-resume.py) on whichever box
  runs the forge; the rest of the run keeps what it passed. A job that
  failed of itself is never run again ([forge.md, Cancel and
  resume](forge.md#cancel-and-resume)).

## Main's full run, and what publish signs

A pull request's run checks what its change can reach. A push to `main`
(each merge) runs everything on that very commit: every test, the jobs
that run there alone (`main-only.json`), every box's system built on every
box and compared (`systems_agree`), the installer built twice on each. A newer push cancels the run before it, so a batch of
merges costs one full run. `dd release publish` signs a commit only once
the forge's own record shows that run passed: a run of `entry_point.yml`,
made by a push to `refs/heads/main`, on exactly that commit, the newest such
run successful (`release_cmd.rs`, `full_run`). It never reads a commit
status for this: any CI job's token can post a status of any name on any
commit, a pull request's too. A full run's job killed from outside is run
again, as a pull request's is (ci-resume.py), and the run passes once it
does.

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

- **It runs only when it can matter.** Give it a set in
  [`tests-this.json`](../../nix/tests/ci-select/tests-this.json): the code
  it checks, read from the test (Choosing the tests). `checks.ci-select`
  refuses a registered check without one. A check that runs on every pull
  request must be one that every pull request can break.
- **It is cached by content.** Prefer a nix derivation (a check in the
  flake) over a script: a run whose inputs did not change gets the last
  answer from the store instead of working it out again.
- **It runs alongside the others.** Add `needs:` only for an output it
  really uses. A new VM test is its own job in the `vm_tests` matrix, never
  a step in someone else's.
- **It stops the run when it fails.** End the job with the stop-run step,
  like the others.
- **A new crate's tests come along by themselves.** Every workspace
  member's tests are in `checks.tests`, and its lint and tests on their own
  in `rustChecks.<crate>`, which a pull request builds when the crate's set
  (`rust:<its path>` in tests-this.json) is touched. A test that needs a file outside
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

## Choosing the tests

`files_modified` runs
[`nix/tests/ci-select/ciselect.py`](../../nix/tests/ci-select/ciselect.py),
which reads the change from git and hands it to
[`plan.py`](../../nix/tests/ci-select/plan.py): one answer per job. The
replay of the history (below) calls the very same function.

- **A job runs when the change touches what it tests.** Each job's set in
  [`tests-this.json`](../../nix/tests/ci-select/tests-this.json) is the
  code its assertions reach, read from its test script and the modules,
  programs and pages those assertions drive, with why for each path; not
  its closure, which is nearly the whole tree. Every VM test, every
  browser and Worker check, every evaluation-only check, each workspace
  crate (`rust:<its path>`), and the jobs that are not one check (`rust`
  for fmt, `app`, `app-release`, `flake` for the evaluation, `keep`,
  `systems`, `systems(home-controller)`, `installer`) has one. The VM tests
  share `_shared_by_family.vm` (the test library, the test box,
  flake.lock). A plain path covers everything under it; `*` stays in a
  directory, `**` crosses them.
- **Docs alone** (`docs/`, a `.md` at the top, `.claude/`, `.github/`) run
  style and lint, nothing else, and `ci_ok` passes. A docs file a `.nix` or
  `.rs` file names by path (the terms, for `checks.terms-version`) counts as
  code.
- **A component's insides** (Interfaces, below) whose face held run that
  component's own checks and nothing that merely reads it.
- **A new test** is a name in [`nix/tests/registry.nix`](../../nix/tests/registry.nix)
  (`vm`, `files`, `eval`) beside its files and its set, not an edit to
  `flake.nix` or the workflow. A change that only adds or removes names
  there runs the new tests; any other change to it runs what tests the
  registry (`ci`, `ci-select`, the evaluation).
- **flake.lock runs every job a pull request runs**: it pins nixpkgs (the
  Rust and Go toolchains, every service), crane and the rust overlay.
  Nothing else is under everything: flake.nix, the workflow or a shared
  library reach the jobs whose sets name them, and main's full run checks
  the rest.
- **Main's alone.** A job that has run on at least 300 pull request runs
  and 300 minutes and never caught a real failure is on
  [`main-only.json`](../../nix/tests/ci-select/main-only.json): main's full
  run runs it, a pull request never does. A failure of the box, the forge,
  CI itself, or a test's own race is not a catch. 300 runs without one
  bounds the job's catch rate under 1% at 95% confidence (the rule of
  three). `history/replay.py --main-only` writes the file from the history;
  a job main's full run then catches failing comes off it. So do every
  box's system built on every box, `systems_agree`, and the installer pair.
- **A path no job tests** runs nothing beyond style and lint, and
  files_modified names it ("no job tests ..."): no check could say
  anything about it. Give the job that should check it a path in its set.
- **The app** (`app_checks`: its lint and tests, the network engine's) is
  main's alone. Its downloads' recipes evaluating (`app-release`) stays on
  pull requests, in flake_check: a minute beside the app's twenty.

`checks.ci-select` ([`edges.py`](../../nix/tests/ci-select/edges.py)) holds
the selector to made-up changes, one per rule above, and holds the sets to
the tree: every registered check, crate, system and component's own check
has a set, every path a set names is in the tree, and `main-only.json`
names only jobs that exist. A set that drifts from its test is caught in
main's full run, which runs everything; give the set the path in the pull
request that fixes it.

## Interfaces

Every component has insides and a face. A pull request that changes a
component's insides alone, and leaves its face as it was at the base, runs
the component's own checks and its contract checks; the VM tests that only
read it are left out, since nothing they see of it moved. A face that
changed walks out to every job whose set names the component's files.
Main's full run runs everything either way.
[`interfaces.py`](../../nix/tests/ci-select/interfaces.py) holds the map:

```
gate     insides  servers/gate/src/*.rs (its pages are not insides)
         face     every route: method, path, handler signature, the types it
                  takes and answers and their fields, JSON keys, statuses,
                  headers, redirects; the environment it reads, the box
                  paths and ports it names (faces/rust_routes.py)
         own      fmt, its crate's (verify) and the cli's (dd, whose end to
                  end runs a gate) lint and tests, site-headers, site-roles,
                  sso
control  insides  servers/system/control/{cmd,place,state,validator,wire,
                  third_party}/*.go, testdata aside (the Rust signers read it)
         face     apidiff on state, state/computers, state/ident and wire;
                  goface: HTTP routes, flags, environment, files, ports and
                  every json-tagged struct (apidiff does not see a tag)
         own      control (every package's go test, and the relay, witness
                  and validators under wrangler), control-relay,
                  control-upgrade, control-together, control-signfail
```

Both ends are read in files_modified, from `git archive` of each: seconds,
offline (apidiff loads the packages from dd-control's vendored modules).
A face that cannot be read counts as changed. A face misses what a
handler does with the same shapes; that is what the contract checks are
for, and main's full run is the backstop. `faces_test.py` holds the readers
to small made-up changes in the ci-select check.

## The history: what the selector is scored against

[`history/`](../../nix/tests/ci-select/history) holds every CI run the
forge recorded, job by job (`data/history-jobs-*.csv`): its minutes, box,
result, the derivations its log says it built, and a class. Each job's
minutes count as one of these: superseded by a newer push, stopped when
another job failed, failed for its box (killed, out of memory, out of
disk, the network), a flake (the very derivation it failed on passed
elsewhere), a no-op (it built nothing), determined (it built a
derivation that had already passed), overhead, or needed. A failure that is
none of those, and that a code change then fixed, is real
(`realfail.py`), with the files the breaking change touched.

[`replay.py`](../../nix/tests/ci-select/history/replay.py) chooses every
pull request run of the history again, in the order they were made, with
`plan.py` itself, and reports each run's and each pull request's wall
time on the runner slots and every real failure the choice leaves to
main's full run. Nothing in it is fitted to the failures: the sets were
read from the tests, the faces are read from each run's own two commits
(`faces.py`, the code read where it lived then), and `main-only.json`'s
rule is applied at each run from the runs before it alone. Each job counts
the minutes it took in that run, or its usual minutes (the median of its
last 30 good runs) when the run did not have it or it was cancelled,
killed or flaked; a test that did not exist yet is not added. The
simulated wall time matches the measured one (the median of actual over
simulated is 1.01 over 443 runs). [`catches.json`](../../nix/tests/ci-select/history/catches.json)
says, from each log, which of today's jobs would catch a failure whose job
was not one check (a flake_check, rust, programs or systems failure), and
which failures were not the change's code (the box, the forge, CI itself,
a test's own race). [`relevance.py`](../../nix/tests/ci-select/history/relevance.py)
is the job-by-job table: minutes spent and real failures caught.

To bring it up to date: `export.sh DIR` (copy only the logs not already in
DIR/L: the whole log directory is slow over the fleet's network),
`logfacts.py DIR/L DIR/tasks.csv > DIR/logfacts.json`, `history.py DIR`,
`realfail.py DIR > DIR/realfail.json`, `changes.py DIR > DIR/changes.json`,
`phases.py DIR > DIR/phases.json`, `faces.py DIR > DIR/faces.json` (with
apidiff, goface and go on PATH, `FACES_GO_MODULES=mod`), then
`replay.py DIR > DIR/replay.json` and `replay.py DIR --main-only >
nix/tests/ci-select/main-only.json`. A new failure whose job is not one
check gets a line in catches.json, read from its log. All of it reads the
forge; none of it writes.

## The site checks

The site as a browser meets it. The no-vm ones are the `site_checks` job
(ciselect.py `GROUPS`), beside `page_tests` (the Mail and Files pages as
the app serves them) and `worker_tests` (the Workers):

- **No vm** ([`nix/tests/site.nix`](../../nix/tests/site.nix)): node1's own
  nginx config and the real gate, with the pages a release carries, and a
  stand-in for every service behind them
  ([`site/stack.py`](../../nix/tests/site/stack.py)). One of each kind of
  person is signed in ([`site/people.py`](../../nix/tests/site/people.py)):
  signed out, a guest, a sign-up not let in, a banned member, a member by
  invite, a member by a box (agreed identity) and the owner, who is the
  admin. Seconds each on a quiet machine; on a busy runner headers and
  roles take 10-25 s, speed about a minute, and accessibility two to five.
  - `site-headers`: HSTS, nosniff, Referrer-Policy and the content policy
    on every name, written out again from headers.nix; the session cookie's
    flags; no cookie from the front page or the docs; every GET route of
    the gate asked signed out is refused unless it is listed as public;
    cross-origin reads only where llm.nix and forgejo.nix mean them; the
    session never reaches a service.
  - `site-roles`: a table of places by person, written from gate.md,
    joining.md and guests.md.
  - `site-a11y`: axe-core on every kind of page in the site map; serious
    and critical findings fail. `site/a11y.py` `audit(page)` is the one
    call the site-links crawler makes on every page it reaches.
  - `site-speed`: for each main page, the page's own CPU from a click on a
    link to it until it is drawn, in units of a fixed piece of page work
    timed first in the same browser, and a first visit's bytes and
    requests, against budgets a little above what main measured.
  - `site-links` ([`nix/tests/site-links`](../../nix/tests/site-links)):
    every link the site's pages show, followed in a browser as a member and
    a stranger, each way a browser finds the box (public DNS, the fleet's
    DNS, from outside), failing on a link that opens no real page, a new
    tab, or nothing, and on a page with a script error, a content-policy
    refusal or a failed request; the app's pages' links are read against
    what the app and the site serve. Every page it opens is audited by
    `a11y.audit` too.
- **A vm** ([`nix/tests/site-journeys.nix`](../../nix/tests/site-journeys.nix)),
  its own job in the `vm_tests` matrix like every VM test: each service's
  one main thing against the real services on one box.

Each has a `KNOWN` list (or `KNOWN_BARE_404`): what main gets wrong today,
with why. A finding on that list does not fail. One that stops being found
fails too, so the list only gets shorter.

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
journal_of(box, unit, inv)      that run's journal lines, no other run's; also the
                                last lines journald reads after the run has ended
journal_has(box, unit, inv, s)  whether that run wrote s
journal_cursor(box)             where the journal is now
journal_since(box, unit, c)     every run's lines since that cursor: for a thing
                                whichever run comes first does, a timer's or the test's
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
- **systems_agree** means the two boxes got a different store path for one
  system: they evaluated the commit differently (a different input, an
  impure evaluation), or one recorded nothing. It compares paths only, and
  a system's path is fixed by its derivation, not by the bytes it builds,
  so it does not catch a build that is not reproducible. That is dd-attest's
  at publish: two boxes must report the same closure digest
  ([releases.md](releases.md#dd-attest-accounting-for-every-path)).
- **installer_agrees** means the runner boxes built the box installer's
  drive (`installers.x86_64-linux.box`) to different bytes, and the next
  app release would stop on it. It runs when the installer was chosen
  (above); [joining.md](joining.md#the-drive) says what
  made it differ before and how it is made now.
- **"context canceled"** at the end of a job's log, or a log that stops in
  the middle of a step with no end line, means the job was killed from
  outside, not that the code failed. ci-resume runs that job again, and
  `ci_ok` after it, within a few minutes.
- **Never merge around a red run.** If a check is wrong, fix the check in
  its own pull request.
