# Test selection, replayed against main

31 merged pull requests (2722dfb72 to 9f17b5969), each base..head. Ground truth: every
check's drvPath at both ends (`nix eval` of checks.x86_64-linux). covers.json as traced at the oldest commit,
never again, with what each commit's .nix files name, as CI has it. A skipped check whose drvPath moved is a
false negative unless the pages and docs alone moved it (sim-pages.py).

## False negatives

None.


## Skipped although the drvPath moved: pages and docs only

| PR | check | job |
|---|---|---|
| 600a22917 fix: the remote computers' base image is the same  | adblock | vm_tests |
| 600a22917 fix: the remote computers' base image is the same  | forge | vm_tests |
| 600a22917 fix: the remote computers' base image is the same  | home-controller | vm_tests |
| 600a22917 fix: the remote computers' base image is the same  | leaving | vm_tests |
| 600a22917 fix: the remote computers' base image is the same  | library | vm_tests |
| 600a22917 fix: the remote computers' base image is the same  | metrics-ui | vm_tests |
| 600a22917 fix: the remote computers' base image is the same  | network | vm_tests |
| 600a22917 fix: the remote computers' base image is the same  | photos | vm_tests |
| 600a22917 fix: the remote computers' base image is the same  | sso | mail_worker |
| 600a22917 fix: the remote computers' base image is the same  | tests | test |
| 600a22917 fix: the remote computers' base image is the same  | thanos | vm_tests |
| 600a22917 fix: the remote computers' base image is the same  | vault | vm_tests |

## Savings

- VM test jobs: 877 before, 604 now; 288 of them had a moved drvPath (nothing can skip those).
- Job-minutes, by the timing study's medians: 1705 before, 1609 now (6% fewer).
- The old path lists skipped 15 checks whose drvPath moved, in 3 pull requests:
  - 7eb264692 fix: Mail's search test asserts how little a searc: adblock directory forge leaving library metrics-ui network photos release thanos unlock vault
  - 600a22917 fix: the remote computers' base image is the same : tests
  - b5318140a fix: the nightly check gives way to a new release : clippy-app tests-app
- Docs-only pull requests: 0, each down to style and lint.

| PR | change | files | chosen by | VM before | VM now | VM moved | mail now/moved | min before | min now |
|---|---|---|---|---|---|---|---|---|---|
| 2722dfb72 | fix: the full and the recent store copy each wri | 2 | chosen | 28 | 14 | 4 | 3/0 | 35.3 | 28.7 |
| f5317225c | tests: the store test starts each copy with star | 2 | chosen | 28 | 1 | 1 | 1/0 | 25.0 | 13.6 |
| b931c85e2 | placement: a service may move only on failure, a | 14 | everything: nix/lib/capability.nix: unde | 28 | 28 | 11 | 6/2 | 53.9 | 55.9 |
| 7813b0f17 | placement: remote computers are placed like any  | 12 | everything: nix/lib/capability.nix: unde | 28 | 28 | 11 | 6/2 | 53.9 | 55.9 |
| 87a2aa8a4 | remote computers: the box side from remote-compu | 35 | everything: .forgejo/workflows/entry_poi | 29 | 29 | 18 | 6/5 | 91.3 | 91.3 |
| a7b6680ca | remote computers: a computer's lease is the log' | 13 | everything: flake.nix: under everything | 29 | 29 | 2 | 6/1 | 34.8 | 34.8 |
| 511ba1fb8 | remote computers: the box runs the agreed record | 27 | everything: nix/lib/placement-inputs.nix | 29 | 29 | 18 | 6/4 | 91.3 | 91.3 |
| d15b777a0 | tests: the control test's never-runs check watch | 1 | chosen | 29 | 1 | 0 | 2/1 | 25.5 | 14.6 |
| 5ad24d7a7 | tests: the control test's never-runs check watch | 1 | chosen | 29 | 1 | 0 | 2/1 | 25.5 | 14.6 |
| 7eb264692 | fix: Mail's search test asserts how little a sea | 1 | chosen | 0 | 15 | 12 | 4/1 | 20.4 | 55.4 |
| 2d26a73a5 | fix: node1 carries no compute role until the com | 1 | chosen | 29 | 2 | 1 | 4/3 | 28.9 | 19.4 |
| 81774333c | ci: lint run by hand checks from where the branc | 2 | everything: .forgejo/checks/lint.py: und | 29 | 29 | 10 | 6/1 | 54.3 | 54.3 |
| 4ec72cef7 | fix: a remote computer's first boot does not reb | 5 | chosen | 29 | 5 | 1 | 1/0 | 30.4 | 19.8 |
| f2d83fc0d | fix: a handover copies a writer's last state out | 22 | everything: nix/lib/needs.nix: under eve | 29 | 29 | 15 | 6/2 | 70.2 | 72.1 |
| f094f04f6 | fix: the initrd waits for its boot partition bef | 8 | chosen | 29 | 5 | 4 | 1/0 | 35.2 | 24.8 |
| 600a22917 | fix: the remote computers' base image is the sam | 2 | chosen | 29 | 5 | 11 | 1/1 | 55.2 | 19.8 |
| f8c244d92 | feature: edit and sign any PDF in Files - marks, | 25 | everything: .forgejo/workflows/entry_poi | 29 | 29 | 16 | 6/4 | 84.3 | 84.3 |
| 51cd072ba | feature: Mail and Files together - attachments s | 21 | everything: flake.nix: under everything | 29 | 29 | 16 | 6/4 | 84.3 | 84.3 |
| daccd159b | feature: Files searches the whole library, shows | 23 | everything: .forgejo/workflows/entry_poi | 29 | 29 | 16 | 6/5 | 84.3 | 84.3 |
| 50d94feb8 | fix: the PDF redaction test looks for the accoun | 1 | chosen | 29 | 11 | 10 | 2/1 | 52.4 | 45.4 |
| 5a622cd72 | fix: an ssh login coming or going can no longer  | 6 | everything: .forgejo/workflows/entry_poi | 30 | 30 | 1 | 6/0 | 31.7 | 31.7 |
| 7adebce3a | feature: Mail and Photos through one key route - | 26 | everything: Cargo.lock: under everything | 30 | 30 | 18 | 6/6 | 91.4 | 91.4 |
| 2feb3b340 | test: Mail's and Photos' checks wait for the fra | 2 | chosen | 30 | 3 | 1 | 2/1 | 26.6 | 15.8 |
| b5318140a | fix: the nightly check gives way to a new releas | 1 | chosen | 30 | 18 | 16 | 5/2 | 83.5 | 82.0 |
| 9a0444ba9 | app: the Linux download compiles its dependencie | 2 | everything: flake.nix: under everything | 30 | 30 | 0 | 6/0 | 29.7 | 29.7 |
| 270317509 | fix: a resident model is loaded once its downloa | 1 | chosen | 30 | 8 | 1 | 3/0 | 26.8 | 18.0 |
| 76cc35a08 | feature: a library its owner lets another member | 10 | everything: client/identity/src/lib.rs:  | 30 | 30 | 19 | 6/6 | 92.4 | 92.4 |
| 85eb7b937 | fix: letting a site through no longer fails when | 2 | chosen | 30 | 17 | 15 | 5/2 | 79.9 | 76.2 |
| 517d7cf54 | feature: Calendar and Contacts - every calendar  | 40 | everything: .forgejo/workflows/entry_poi | 30 | 30 | 18 | 6/6 | 91.4 | 91.4 |
| 8eebcf10f | feature: calendars and contacts in the programs  | 28 | everything: Cargo.lock: under everything | 30 | 30 | 11 | 6/5 | 57.8 | 57.8 |
| 9f17b5969 | feature: a calendar from a link - Google's or Ou | 16 | everything: Cargo.lock: under everything | 30 | 30 | 11 | 6/5 | 57.8 | 57.8 |

## Made-up changes (edges.py)

| case | change | chose | ok |
|---|---|---|---|
| a test's own file | M nix/tests/acme.py | 1 of 40: acme | yes |
| a new test helper file | A nix/tests/lib/later.py | 12 of 40: acme forge-follow metrics-ui net-follow pg-follow photos store switch-race takeover unlock waits work-memory | yes |
| a test helper changed | M nix/tests/lib/waits.py | 12 of 40: acme forge-follow metrics-ui net-follow pg-follow photos store switch-race takeover unlock waits work-memory | yes |
| a renamed module | D nix/modules/forge/live.nix A nix/modules/forge/live-pages.nix | 11 of 40: acme computers control dns-worker forge forge-follow joined-box members-runner net-follow store takeover | yes |
| a new directory in a crate | A servers/games/src/catalogue/mod.rs | 15 of 40 | yes |
| a new crate | A servers/newsvc/Cargo.toml A servers/newsvc/src/main.rs | 29 of 40 | yes |
| a new top-level directory | A tools/run.sh | everything (tools/run.sh: no check claims it) | yes |
| a path nothing claims | M scripts/sync-app-web | everything (scripts/sync-app-web: no check claims it) | yes |
| a crate the app builds on | M servers/system/release/src/bin/dd-attest.rs | 25 of 40 | yes |
| Cargo.lock bumped | M Cargo.lock | everything (Cargo.lock: under everything) | yes |
| nix/lib changed | M nix/lib/script.nix | everything (nix/lib/script.nix: under everything) | yes |
| the workflow changed | M .forgejo/workflows/entry_point.yml | everything (.forgejo/workflows/entry_point.yml: under everything) | yes |
| docs alone | M docs/dev/ci.md A docs/guide/new.md M README.md | docs only | yes |
| the terms (a check reads them by name) | M docs/policies/terms.md | 12 of 40: adblock forge home-controller joined-box leaving library metrics-ui network photos sso thanos vault | yes |
| docs with code | M docs/dev/games.md M servers/games/src/main.rs | 15 of 40 | yes |
| a page alone | M servers/web-shared/home.css | 20 of 40 | yes |
| a page removed | D servers/web-shared/home.css | 20 of 40 | yes |
| a check new to covers.json | M nix/tests/acme.py | 2 of 40: acme forge | yes |
