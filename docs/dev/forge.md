# The forge for Git

The forge is where the fleet's code lives: its repositories, pull
requests, issues, CI runs and releases, and members' own repositories
beside them. It is [Forgejo](https://forgejo.org), run on one box at
`git.<domain>`, with our own pages in front of it. Every public repository
can also be read and cloned by anyone, without an account, at
`code.<domain>` ([The public door](#the-public-door)). Forgejo keeps the data
and does the work; the pages people see are ours, and draw themselves from
Forgejo's API as whoever the gate says is looking.

This page is how the forge is put together. How CI uses it (the workflow,
what blocks a merge, how to change a check) is [its own page](ci.md).

## Where it is

```
nix/roles/forge.nix                   the bundle: forgejo, the tile on home, backups
nix/roles/runner.nix                  the runner bundle: both runners and the nix cache key
nix/modules/forge/
  forgejo.nix                         Forgejo's settings, sign-in, nginx, the setup units
  door.nix                            code.<domain>: the public repositories, read-only, as nobody
  forgejo-setup.sh                    main's rule, through the API
  live.nix                            the database trigger behind the pages' live updates
  ci-cancel.py                        a failing job stops its run
  ci-resume.py                        a job a restart killed is run again, by itself
  forgejo-runner.nix                  the host runner: the fleet's own CI
  members-runner.nix                  the members' runner: Docker under gVisor
  runner-start.sh                     a runner waits for the forge, fetches its token as root, drops to its user, registers, and runs
  runner-token.sh                     the runner's registration token, from the forge's box, opened with this box's key
  runner-keys.nix                     the forge's box makes the runners' tokens, each sealed to its runner box
  runner-restart.nix, .sh             a release's runner and docker config wait for no job running, up to a limit
  needs.nix                           what each part needs of a box
servers/forge/web/                    the Git pages (pages/git-public.html: the public door's)
servers/gate/src/forge_events.rs      the pages' live updates, from the gate
nix/tests/forge.nix, forge.py         the forge, booted
nix/tests/nginx-door.py               the public door, served and asked (the nginx check)
nix/tests/members-runner.nix, .py     a member's job, kept in its box
nix/tests/runner-restart.nix, .py     a switch under running jobs, host and member: they end green
```

Forgejo runs on one box at a time, the holder of its lease (node1 today),
and the `forge` bundle is built only beside the `gateway` one
([`nix/roles/bundles.json`](../../nix/roles/bundles.json) `with`):
Forgejo uses the box's PostgreSQL, the one Photos and Passwords use. The
`runner` bundle is separate: every box that can run VM tests and holds
the runners' tokens is built with it.

## Who talks to whom

```
browser ── https://git.<domain>/... ──> nginx (the forge's box)
                                          │
   /owner/repo, /owner/repo/pulls, /  ────┼──> gate :4181  /_dd/git   the one page (servers/forge/web)
   /_dd/...  (scripts, sign-in, events) ──┼──> gate :4181  /_dd/...
                                          │
   /api/..., *.git, raw, archive, logs,   │
   actions/runs/<n>/cancel (POST) ────────┼──> auth_request /_dd/verify ──> gate: who is this?
                                          │       X-WEBAUTH-USER: <name>
                                          │         │
                                          │         v
                                          │    /run/forgejo/forgejo.sock ──> Forgejo ──> PostgreSQL
                                          │    (group forgejo, dir 0750)          │        "forgejo"
   /_dd/ci/cancel/<run> ──────────────────┴──> ci-cancel.py :3003 ──> socket       │
                                                                                   │ trigger: pg_notify
   gate ── LISTEN dd_forge (as dd-verify, no table) ──────────────────────────────┘
     └── /_dd/git/events?repo=<id> ── Server-Sent Events ──> the page

runner boxes ── https://git.<domain> ──> nginx ──> Forgejo   (fetch jobs, report, push statuses)
ci-resume.py (timer) ──> socket
```

Forgejo listens on nothing but its socket. Everything outside the box
reaches it through nginx, and everything on the box that acts for someone
(the setup units, ci-cancel, ci-resume) opens the
socket as one of the boxes' own identities: the setup units as the admin,
ci-cancel and ci-resume as the CI identity, which is no admin.

## Signing in

Nobody has a password on the forge, and nobody signs up there. Forgejo is
set to believe a header, `X-WEBAUTH-USER`, for both its pages and its API,
and to make the account the first time it sees a name
([`forgejo.nix`](../../nix/modules/forge/forgejo.nix), `service` and
`security`). Registration and the password form are off, mail is off, and
emails are kept private (`noreply.<domain>`).

nginx fills in the header. Every location that goes to Forgejo (`toForge`)
first asks the gate who this is (`auth_request /_dd/verify`), takes the
name it answers with, and sets `X-WEBAUTH-USER` to it. Whatever the
browser sent in that header is replaced either way, so nobody names
themselves. Two cases differ:

- **Nobody signed in.** The gate answers 401, and nginx hands the request
  to `@anonymous`, which sends it on with an empty header and no cookie.
  Forgejo then shows what it shows a stranger: public repositories only
  (`REQUIRE_SIGNIN_VIEW = false`). That is why a public repository can be
  cloned without signing in.
- **The demo.** A map turns the name `demo` into an empty header, so the
  demo is nobody to the forge: public repositories, and no account is ever
  made for it. The demo reaches the forge at `demo-git.<domain>`
  ([`nix/roles/forge.nix`](../../nix/roles/forge.nix) lands it on the
  fleet's own repository); on a demo door the gate answers 403 rather than
  401 for nobody, so that door is never the anonymous forge for the whole
  internet (`verify` in [`servers/gate/src/lib.rs`](../../servers/gate/src/lib.rs)).

Forgejo's own `/user/login` is sent to the gate's passkey page
(`/_dd/login?rd=...`). Our session cookie, `dd_session`, never reaches
Forgejo: every location that proxies anywhere but the gate forwards the
cookie with it taken out
([`nix/modules/gate/strip-cookie.nix`](../../nix/modules/gate/strip-cookie.nix)).

Forgejo keeps a session cookie of its own, and when the header is empty
it signs in whoever that cookie names. So whenever the header is empty
(nobody, or the demo) no cookie at all reaches Forgejo: the `toForge`
locations send `$forge_cookie`, which is empty when `$forge_user` is, and
`@anonymous` sends none. A session the gate has ended is ended on the
forge too. A location changes what it sends through `ddCookie`
(strip-cookie.nix), never a `proxy_set_header Cookie` of its own: nginx
sends every one it is given, so a second line would put the cookie back.

**No person is the forge's admin**, the fleet's owner included. The admin
is the box's own identity, `commonty-box` (`dd.forgejo.admin`), which only
the box's units act as, over the socket: branch protection and deleting
whoever left. Cancelling and rerunning CI runs is the CI identity's,
`commonty-ci` (`dd.forgejo.ci`): no admin, a collaborator who may write to
the fleet's own repository (what Forgejo asks of whoever cancels or reruns
a run there) and nothing else. `forgejo-admin` makes both ahead of time
with random passwords nobody uses, at every start and hourly, makes the
box's identity the one admin, and gives the CI identity its one
repository once that exists. Nobody signs in as either:

- nginx maps no one to their names (`$forge_user`), so a person by either
  name would be nobody here, and the gate refuses both as a member's name,
  at sign-up and in the directory's held names
  (`directory::BOX_IDENTITIES`);
- on `git.<domain>`, `/api/v1/admin` is 404, the `Sudo` header is dropped
  and a `sudo` query refused (403), whoever asks;
- `dd-forge-rules`, at every start, takes admin from every other account
  and keeps it on the box's identity, so an account made an admin by hand,
  or the fleet's owner's from before, is not one after the next start.

What a person could once do as admin, the box does on a request the
member it affects has made (deleting a leaver's account), or a
repository's own owner does (approving a fork's run, below).

### Why the socket, and why its group matters

Forgejo believes `X-WEBAUTH-USER` from whoever reaches it.
`REVERSE_PROXY_TRUSTED_PROXIES` names only loopback, but on this box
loopback is not enough: the host runner runs CI jobs here, and game guests
run here too. Anything that could open a TCP port on 127.0.0.1 could say
"I am the admin".

So Forgejo listens on a unix socket, `/run/forgejo/forgejo.sock`
(`PROTOCOL = "http+unix"`, mode `660`), in a directory only the `forgejo`
group may enter (`RuntimeDirectoryMode = "0750"`). The members of that
group are the whole list of what may speak for someone:

```
nginx            users.users.nginx.extraGroups = [ "forgejo" ]
dd-ci-cancel     DynamicUser, SupplementaryGroups = [ "forgejo" ]
dd-ci-resume     DynamicUser, SupplementaryGroups = [ "forgejo" ]
forgejo-admin    runs as forgejo
forgejo-protection   runs as root
```

The runner's user, which is what a CI job runs as, is not in it. Adding a
unit to the `forgejo` group lets it say it is the box's admin identity;
treat it that way. The old
port, 3001, has nothing listening, and the forge VM test checks that.

## Which addresses are ours

Every address on `git.<domain>` is ours unless it is on Forgejo's list.
Ours all get the same page: nginx rewrites them to the gate's `/_dd/git`,
which serves `servers/forge/web/pages/git.html` to anyone signed in and
sends anyone else to sign in (`git_page` in
[`servers/gate/src/lib.rs`](../../servers/gate/src/lib.rs)). The page then
reads the address and draws the view.

```
Forgejo's own (toForge: sign-in header, or anonymous)
  /api/  /assets/  /avatars/  /repo-avatars/  /attachments/  /user/  /login/  /-/  /.well-known/
  /<owner>/<repo>.git/...                       git over https
  /<owner>/<repo>/info/refs, info/lfs, git-upload-pack, git-receive-pack
  /<owner>/<repo>/raw/ archive/ media/ releases/download/ attachments/
  /<owner>/<repo>/actions/runs/<n>/jobs/<n>/attempt/<n>/logs
  /<owner>/<repo>/actions/runs/<n>/cancel      POST only, signed in (cancelRun: no anonymous fallback)

Special
  = /user/login        302 to /_dd/login (the passkey page)
  = /fleet-repo.json   { "repo": "<owner>/<name>" }: the repository the boxes are built from
  /_dd/ci/             ci-cancel.py on 127.0.0.1:3003, no sign-in
  /_dd/                the gate (every gate host has it: nix/modules/gate/verify.nix)

Ours (rewritten to /_dd/git)
  everything else: /, /new, /search, /notifications, /settings, /<owner>,
  /<owner>/<repo>, .../tree, blob, commits, commit, branches, releases,
  activity, issues, pulls, compare, actions, actions/runs/<n>, settings/<pane>
```

Forgejo's own HTML pages (anything not on its list) are never reached
from outside; a link into one of them lands on ours. The page keeps a copy
of that list (`FORGE` in [`git-core.js`](../../servers/forge/web/git-core.js))
so that a link to a clone, a raw file or a log is a real page load, not a
view. The two lists must agree: each says where the other is.

Git over ssh goes through the box's own sshd on port 22
(`START_SSH_SERVER = false`): Forgejo keeps the `forgejo` user's
`authorized_keys`, sshd does the rest.

The gateway also reaches the socket, for the app's downloads:
`<domain>/download/v<x.y.z>/<file>` is rewritten to that tag's release
file on the fleet's repository, sent as nobody with no cookie and no
`Authorization` ([`nix/roles/gateway.nix`](../../nix/roles/gateway.nix)).

## The public door

`code.<domain>` is the forge for anyone: every public repository, to read
and to clone, and nothing else. It is its own server block
([`door.nix`](../../nix/modules/forge/door.nix)), and one of the public
names the Cloudflare tunnel carries (`dd.public.hosts`,
[gate.md](gate.md#public-names-the-bare-domain-and-the-demo)).

**Why a name of its own.** `git.<domain>` is the members' forge: on the
fleet's network only, and every request to Forgejo there carries the name
the gate gives it. Opening that name to the tunnel would send members'
sessions through Cloudflare (a proxied name resolves there for everyone),
and leave one server block deciding per request whether it speaks for
someone. `code.<domain>` never speaks for anyone, so nothing there can go
wrong in that direction; members' Git is exactly as it was.

**Nobody, always.** Every location to Forgejo sends no `Cookie`, an empty
`Authorization`, `X-WEBAUTH-USER`, `-EMAIL`, `-FULLNAME`, `Sudo` and OTP
header, whatever the request carried, and drops `Set-Cookie` on the way
back; the gate's locations the same. A query naming `token`,
`access_token` or `sudo`, or any parameter whose name is percent-encoded,
is 403 (Forgejo would otherwise take a token from the query). A path with
a `.` or `..` segment, plain or encoded, is 400, so what nginx matched is
what Forgejo is asked. A 401 or 403 from Forgejo (what a stranger gets for
a private repository, or a limited organisation's) becomes a plain 404,
without the `WWW-Authenticate` that would say "this exists, sign in".

```
let through, GET or HEAD only, as nobody
  /  /<owner>  /<owner>/<repo>[/tree/… /blob/… /commits[/…] /commit/<sha>
     /branches /releases[/tag/<t>] /src/…]        the gate's /_dd/git/public
  /_dd/static/<file>  /_dd/web/<file>              the gate: the page's files
  /_dd/me                                          401 from nginx: nobody here
  /api/v1/repos/search, /users/<u>/repos, /repos/<o>/<r>[/branches /tags
     /languages /commits /releases /contents[/…] /raw/… /git/commits/<sha>[.diff]
     /git/trees/<ref> /commits/<ref>/status /compare/<a>...<b> /releases/tags/<t>]
  /<o>/<r>/raw/…  /media/…  /releases/download/…   files
  /<o>/<r>/archive/<ref>.<ext>                     archives (paced harder)
  /avatars/<id>  /repo-avatars/<id>  /assets/img/<file>
  /<o>/<r>[.git]/info/refs?service=git-upload-pack
  POST /<o>/<r>[.git]/git-upload-pack              clone and fetch
  /robots.txt                                      crawlers get owner and repo front pages only

refused
  any other method                                 405
  info/refs for anything but upload-pack           404
  git-receive-pack, LFS, every other /api/…, the
  runners' API, /user/…, /login/…, /-/…, /admin,
  /notifications, /settings, packages, /_dd/…
  but the two above, any other repository page    404
```

The API list is what the views marked for anyone call, and nothing more
(below); `/repos/issues/search` and the like are not an owner and a
repository. Forgejo itself decides what nobody may see among those: public
repositories only, no private or limited account's, and an account's own
address never (its API gives nobody the noreply one).

**Limits.** Each location below has its own, in place of the public
names' (64 connections and 50 requests a second per address, which still
hold for every other address on the name):

```
pages, API, files      dd_code          20 r/s per address, burst 100; 32 connections
info/refs, upload-pack dd_code_git       2 r/s per address, burst 20; 2 connections
archives               dd_code_archive   6 a minute per address, burst 3; 2 connections
both of the last two   dd_code_all       8 at once from everyone together
upload-pack            a 4 MB request, read in 15 s; 60 s between reads either
                       way; 4 MB/s per connection; streamed, never spooled to disk
```

**Headers.** The page and its files get a content policy of their own,
stricter than the signed-in pages': `default-src 'none'`, scripts, styles,
images and fetches from the door alone, no frames either way, no form but
to itself (`ddCsp` in [headers.nix](../../nix/modules/gate/headers.nix)).
What Forgejo answers gets `default-src 'none'; sandbox`, so a file opened
on its own runs nothing. The page and its files are the gate's: `no-cache`
with an ETag, fonts a week. Nothing on the door is anyone's own, so
nothing is kept for one person and not another.

**The page.** `pages/git-public.html` is the Git page with its bar marked
`data-public`: the same modules, in their public mode (`open` in
git-core.js). Only the views registered with `route(…, true)` are drawn
(a repository's code, files, commits, a commit, branches, releases; the
list of public repositories; an owner's), nothing asks who is looking, the
bell and "Running now" are not there, and the forge's own `git.<domain>`
links (clone addresses, avatars, downloads) are drawn as this name's.

## Forgejo's settings

All in [`forgejo.nix`](../../nix/modules/forge/forgejo.nix):

- **Package**: `forgejo-lts`, PostgreSQL over its socket, LFS on.
- **Repositories** are public, and a push may create one
  (`ENABLE_PUSH_CREATE_USER`). There are no private ones: see
  [Encrypted repositories](#encrypted-repositories). The landing page is Explore, though no
  one sees it: `/` is ours.
- **Attachments** to an issue or a pull request: images, PDF, text, logs,
  Markdown, CSV, JSON, patches, archives and video, up to 100 MB. Never
  `.svg` or `.html`: served from `git.<domain>`, either would run as
  whoever opens it.
- **Release files**: the same, plus the app's packages (`.deb`,
  `.appimage`, `.apk`, `.exe`), up to 400 MB each. `dd release app`
  uploads them; the downloads page serves them.
- **Webhooks** may go to the internet only (`ALLOWED_HOST_LIST =
  "external"`): never to this box, the house or the tailnet.
- **Actions** are on. A bare `uses:` would be looked up on GitHub
  (`DEFAULT_ACTIONS_URL = "github"`); our workflows write theirs out in
  full and pin them.
- **Strangers** see public repositories (`REQUIRE_SIGNIN_VIEW = false`),
  no account's address (`DEFAULT_KEEP_EMAIL_PRIVATE`, and `SHOW_USER_EMAIL
  = false` for Forgejo's own pages, which no one outside reaches). A
  member's account or organisation set to limited or private is not there
  for them at all. Commits carry whatever address their author's git
  wrote: that is in the repository, not the forge.
- **Plaintext.** The forge reads the repositories as they are pushed, so
  its box is labelled as holding plaintext (`dd.box.plaintext`).

### Encrypted repositories

A repository on the forge is plaintext on its box, so the forge offers no
private ones: "private" would be a promise the box cannot keep. The Git
pages offer public only, a push creates a public one, and the forge's
database refuses a new private repository or a change to private
(`dd-forge-rules` in `forgejo.nix`: Forgejo itself has no setting that
forbids them). One that was private before stays so until its owner makes
it public.

Code that must stay private goes in an encrypted repository, through the
`dd` remote helper ([`client/gitremote`](../../client/gitremote/src/main.rs)):

```
git remote add origin dd::https://git.<domain>/<you>/<name>.git
git push -u origin main
```

Every push encrypts on the device before anything leaves it; the forge
stores opaque files in an ordinary repository and cannot read them. So an
encrypted repository has no pull requests and no Actions: those need the
box to read the code.

### The setup units

The repository itself is pushed to the forge, never made by it. What the
forge needs around it is made again on every start, idempotently:

```
forgejo-secrets      Forgejo's own secrets; makes its state dirs as root first
forgejo              after zfs-datasets, RequiresMountsFor the state dir
forgejo-admin        the boxes' own identities, the one admin, the CI identity's repository (hourly too)
forgejo-protection   main's rule (forgejo-setup.sh); "nothing to protect" until the repo exists
dd-forge-rules       no private repositories; host runners for the fleet's repos alone; no person an admin (forgejo.nix)
dd-forge-notify      the trigger for live updates (live.nix), after every Forgejo start
dd-ci-cancel         the cancel route, always running
dd-ci-resume         oneshot on a timer
```

**main's rule** is declared in Nix and set through the API on every
release, so a rebuilt box gets it back and a change made on the Settings
page does not last (the page says so for the fleet's repository):

```
status check   entry_point / ci_ok (pull_request)
outdated       block_on_outdated_branch = true
approvals      0
push           nobody, the admin included: main takes merged pull requests only
```

`ci_ok` is the one job that waits on every other, so the rule names one
check that always exists. [`ci.md`](ci.md) is what that means for a pull
request. `forgejo-setup.sh` also deletes an old `DD_CI` repository secret,
which the cancel route no longer needs.

### Cancel and resume

Forgejo's API can neither cancel a run nor run one again; its web routes
can. A person cancels a run from the Git pages; a failing job and a
restart are handled by small services on the forge's box, as the CI
identity (no admin), through the socket.

- **By hand**: the Actions list has a Cancel button on each run that is
  waiting, blocked or running, and a run's page has one in its header
  while it is; both only for someone who may push to the repository
  (`permissions.push`), and never on the public door. The button asks
  first, then POSTs to Forgejo's own web route,
  `/<owner>/<repo>/actions/runs/<n>/cancel` (`actions.Cancel` in
  Forgejo's `routers/web/repo/actions/view.go`, behind
  `reqRepoActionsWriter`). nginx sends it to Forgejo as the person signed
  in (`cancelRun` in `forgejo.nix`: POST only, a 1 KB body, one a second
  per sign-in with a burst of 10, and a 401 for nobody, where the other
  Forgejo locations fall back to anonymous). Forgejo decides the rest:
  whoever may not write the repository's Actions gets a 404, the demo
  (nobody to the forge) included. Since 15, Forgejo's CSRF check is Go's
  `http.CrossOriginProtection` (`verifyAuthWithOptions` in
  `routers/web/web.go`), with no token: a POST whose `Sec-Fetch-Site` is
  not `same-origin`, or, without that header, whose `Origin` is not the
  request's `Host`, is a 403. So another site's page, a sibling
  `<service>.<domain>` among them, cannot cancel as a member. The page
  then asks again once; the run's new state also arrives as any other
  change does (live updates, below).

- **ci-cancel** ([`ci-cancel.py`](../../nix/modules/forge/ci-cancel.py)):
  a failing job's last step, `stop-run`, posts its run id to
  `https://git.<domain>/_dd/ci/cancel/<id>`. The service answers 202 at
  once, looks the run up, and cancels it only once the forge itself has
  one of that run's jobs on record as failed (it looks every 5 s for two
  minutes, since the asking job's own failure lands a moment later). It
  takes no secret: all anyone can do with it is stop a run that already
  failed.
- **ci-resume** ([`ci-resume.py`](../../nix/modules/forge/ci-resume.py)):
  a job killed from outside is run again by itself, through the forge's
  own per-job rerun (`/<owner>/<repo>/actions/runs/<n>/jobs/<i>/rerun`,
  `RerunJob` in Forgejo's `services/actions/rerun.go`), which runs that
  job at its next attempt and every job that needs it, transitively,
  after it: `ci_ok` needs every job, so it is run again too, and reads
  its needs' latest results. It does so while the rest of the run is
  still going; the jobs that passed keep their result. Two ways a job is
  killed, both seen on the fleet:
  - its runner stopped under it: the job ends "failed" and its log's last
    line is `context canceled` (runs 771, 786);
  - the forge went away (restarted, or moved to another box): the runner
    can no longer report, the log stops in the middle of a step with no
    end line (`🏁 ...`), and the forge, back, marks the task failed once it
    has heard nothing from it for `ZOMBIE_TASK_TIMEOUT` (10 minutes). The
    jobs that need it, `ci_ok` too, are left blocked, so the run never ends
    by itself. Runs
    873, 880, 881 and 884 lost jobs so to the forge restarting under a
    switch on 2026-10-24, and run 900 five across its move of 2026-10-25;
    before this, none of them was run again, since no log said `context
    canceled`.

  A job that ended itself (its end line, then no `context canceled`) is
  never run again, nor one the forge stopped after three hours (a hang),
  nor one that failed in its first moments with its runner still there (a
  full disk, run 850). A run with any such job is left red, and so is one
  someone cancelled (a person, a newer push, a failure's stop-run). Every 3
  minutes (first 5 minutes after the timer starts, never inside a switch)
  it looks at the last 50 runs: a pull request's run, or main's full run,
  of `entry_point.yml`, the newest for its ref, made within two days; a
  job is run again until its third attempt. It keeps nothing on its box:
  the attempt is the forge's own record, so when the forge moves the next
  pass, wherever the forge now runs (it is one of the forge's units in
  its needs, placed with it), carries on where the last left off. The
  forge not answering is "next time", exit 0. It has
  `restartIfChanged = false`: release 213 rolled back because it ran while
  the forge restarted under the switch. `checks.ci-resume` holds its
  choices to a made-up forge, and `runner-restart` to a real one: a
  runner killed under a pull request's job, the job marked failed, run
  again alone, the job after it and `ci_ok` then passing.

## The Git pages

`servers/forge/web` is one page and a set of modules. The gate serves
them as files under `/_dd/static/` (`servers/gate/src/pages.rs`); there is
no build step.

```
pages/git.html     the bar, the bell, #rhead (a repository's header), #app; purify.js, git.js, shell.js
git.js             imports every view, asks who is looking, starts the bell, draws
git-core.js        $ and el (from kit.js), put/ic, api(), the cache, the router, watch(), the bell, the bar's search
git-home.js        /, /new, /notifications, /search, /settings (tokens, keys, profile), /<owner>
git-repo.js        a repository: header and tabs, code and files, commits, a commit,
                   branches, releases, activity; "Running now" on the fleet's repository
git-pulls.js       issues and pull requests: lists, a conversation, commits/checks/files,
                   the merge box, compare and open
git-actions.js     runs; one run as a graph of its jobs (read from the workflow's needs:)
                   or as jobs and their logs. The graph is laid out by ELK (elk.js in
                   the web bundle, loaded on that page alone): columns as the run goes,
                   square lines that share their path, a matrix of more than four jobs
                   as dots until "Show all"; Cancel on a live run (Cancel and resume)
git-settings.js    a repository's settings: general, people, rulesets, secrets and
                   variables, runners, webhooks, deploy keys, labels, mirror, careful
git-diff.js        a git diff parsed into files, drawn unified or side by side
git-charts.js      the year's squares and the week's line, as SVG
git.css            what a forge adds to home.css: diffs, the run graph, placeholders (states are home.css's)
```

**How a view draws.** Each view module calls `route(pattern, view)`; the
router in `git-core.js` matches the address and calls the view with the
match, the query and `current()`, which turns false as soon as another
view starts drawing, so a slow answer never draws over a newer page.
Links inside the page are followed on pointer-down, without a reload
(`history.pushState`); links to `/_dd/` or to Forgejo's own addresses are
real page loads.

**How it reads the forge.** Everything comes from `/api/v1`, on the same
name, with the browser's cookie; nginx turns that into
`X-WEBAUTH-USER`. Forgejo decides what each person may see and do; the
page is the same file for everyone. A 401 sends the person to sign in,
except for the demo, which is nobody to the forge and is told so. Raw
files, diffs and logs come as text through `text()`. Markdown is rendered
with `marked.js` (imported when first needed) and cleaned with DOMPurify
before it is drawn. Two things come from outside the API: `/fleet-repo.json`
says which repository the boxes are built from, and on that repository
only, `/_dd/fleet.json` (the gate) fills the "Running now" list with each
box and its release.

**Caching**, as [ui.md](ui.md) asks:

- `api()` keeps every read for a minute, and two views asking the same
  thing share one request. Any write clears it all; a view that must see
  now passes `fresh: true`.
- The router keeps what the last 40 addresses looked like. Going back to
  one shows it at once, and the view then draws it again from what the
  forge says now.
- A repository is asked once per visit (`repo()` in `git-repo.js`), its
  branches and tags once.
- A tab with the most to fetch (pull requests, issues, actions, releases,
  activity) starts fetching when the pointer reaches it (`warmers`).
- A repository row's check dot, the languages and "Running now" fill in
  after the page is drawn, never before.

**Asking little, in parallel.** What a view does ask goes out together:

- The code and commits views ask for their list alongside the
  repository, taking the address's first part as the ref, and ask again
  only if the refs say otherwise or the forge's repository is not the one
  the address named. The address's owner and name are encoded into the
  API path (`git-path.js`, tested under node by `checks.git-path`), and
  `.` or `..` asks nothing early. A file's page asks for every open
  folder of the tree beside it at once.
- What a list asks for each row (a commit's or a pull request's checks,
  a branch's "ahead · behind") is asked only once the row is near the
  window (`whenSeen`). Ahead and behind are two commit lists of one,
  counted by the forge (`commits?sha=<tip>&not=<base>`), not compares.
  The pull requests list takes every open one's head from one list, not
  one call each.
- Activity reads its year of commits and its runs by the page: the first
  page says how far back a page reaches, and the pages that get to the
  span are asked together.
- `git.html` preloads every module, so the page's scripts arrive at once
  rather than each after the one that imports it.

The page fires `git:shown` (drawn from the kept copy) and `git:drawn`
(drawn fresh) with the time each took, for measuring click-to-paint.

**Live updates.** Forgejo has no webhook for a run or a job starting, and
a webhook to its own box is refused on purpose, so the database says
instead ([`live.nix`](../../nix/modules/forge/live.nix)):

```
action_run, action_run_job, commit_status  (AFTER INSERT OR UPDATE)
   │ pg_notify('dd_forge', {kind: run|job|status, repo, run, job, index, sha, status|state, private})
   v
gate: LISTEN dd_forge, as dd-verify (CONNECT only, no table); reconnects every 5 s
   │ broadcast, filtered by ?repo=<id>, and only where "private" is false
   v
GET /_dd/git/events?repo=<id>  (members only; 401 nobody, 403 the demo)
   │ Server-Sent Events, keep-alive every 20 s, X-Accel-Buffering: no
   v
watch(repoId, until, on) in git-core.js: gathers 250 ms, then asks the API again
```

The announcement is ids and a state, nothing else; the page asks the API
for what changed, as the person looking. The Actions list redraws on a run
or job change; a run's graph redraws on its own changes. The log view
reads the log every 3 seconds while its job runs. `dd-forge-notify` puts
the trigger in after every Forgejo start, so an upgrade that rebuilds a
table gets it back. The gate is told where to listen by
`VERIFY_FORGE_EVENTS`; without it, `/_dd/git/events` is 404 and the pages
do not update live. The bell (unread notifications) is asked every two
minutes.

## The runners

Two runners, both from the `runner` bundle
([`nix/roles/runner.nix`](../../nix/roles/runner.nix)), on every box built
with it (node1 and node2 today). Both run as the plain system user
`forgejo-runner` (not the NixOS module's dynamic user, whose state
directory is mounted `noexec` and broke every cargo build script), share
`/var/lib/gitea-runner`. Each one's main process waits for the forge to
answer at `https://git.<domain>` with a certificate the box believes, for
as long as it takes, then registers and runs the runner
([`runner-start.sh`](../../nix/modules/forge/runner-start.sh)). Nothing
waits before the main process: a switch would wait with it, and the forge
is on another box (release 358 on node2 failed both runners while it was
down). If the registration on disk is for another address, or the token
or labels changed, the runner registers again.

```
                 host runner (<box>)                members' runner (members-<box>)
module           forgejo-runner.nix                 members-runner.nix
labels           nix:host  <box>:host                members:docker://ghcr.io/catthehacker/ubuntu:act-22.04
runs             the fleet's CI (runs-on: nix,      a member's workflow (runs-on: members)
                 or a box's name for build_systems)
where            on the box, as forgejo-runner,     in a container under gVisor (runsc)
                 nix on the path, the box's store
capacity         at each start, min(threads/2,      min(2, threads/4, memory GiB/4)
                 min(work slice, memory - services' 4 cpus, 4 GiB, 1024 pids, 1 h per job
                 measured peak - a tenth) / 4G),
                 at least 1 (runner-capacity.sh):
                 node1 4, node2 2 (2026-10-27)
token            made by the forge's box, sealed    the same, global
                 to the runner box; scoped to the
                 repo's owner (runner-keys.nix)
caches           act, nix and cargo-target kept;    none (cache.enabled = false);
                 untouched for 14 days goes         docker prunes what is a week old
```

**The host runner** runs a job straight on the box, so what a green run
built is already in that box's store when the release goes out, and
signed into the nix cache with the box's key (`storage/cache.nix`). That
also means a job is code running on the box as `forgejo-runner`; the
module says so, and is labelled `forgejo-runner (runs contributed code)`.
Two rules keep that safe: the runner's user is not in the `forgejo`
group (above), and the host runners take the fleet's own repositories
alone: those of the fleet repository's owner (`dd.forgejo.hostRunnerOwner`,
the owner in `dd.repo`), which is a scope, not an admin. The forge
enforces the second, whatever token registered a runner: a trigger in its
database (`dd-forge-rules`) moves any runner registered for every
repository to that owner's scope, unless `members` is its only label. A
runner the old rule gave the old admin keeps its scope: the old admin was
that same owner. A pull request from someone else's fork runs only once
someone who may write to the repository's Actions approves it, which is
its own owner (Forgejo's rule for fork runs; no site admin is needed).
**A release never stops the host runner under a job.** Release 331
changed its unit, the switch restarted it, and the next pull request's run
(786), in flight, died ("context canceled ... signal: terminated"). So:

- `restartIfChanged = false`: a switch leaves the running runner alone,
  on its old config.
- `dd-runner-restart` (a timer, every minute) compares the unit on disk
  with the one the runner started from (`systemctl cat`, hashed, recorded
  by an `ExecStartPre` as root in `/run/dd-runner-restart/`) and restarts
  the runner when they differ and no process but the runner's own is in
  its unit, which is when it holds no job
  ([`runner-restart.nix`](../../nix/modules/forge/runner-restart.nix),
  `dd.runnerRestart.units`).
- The wait has a limit: the job limit and 10 minutes (3 h 10 min here).
  When a change was first seen is kept in `/run/dd-runner-restart/`
  across the timer's runs. Past the limit the runner is restarted all the
  same, which is a drain (below), so a stream of jobs back to back cannot
  hold a release off for good. How long each change has waited is a fact,
  `dd_runner_restart_pending_seconds{unit=...}`, and `dd-fleet-alerts`
  mails a box where one has waited over an hour.
- A stop is a drain: `shutdown_timeout = 3h` (the runner's job limit) and
  `KillMode = mixed`, so the runner alone gets the signal, takes no new
  job, and exits once its jobs have ended. A job taken in the instant
  before the restart finishes. So does a box shutting down: it waits for
  the jobs, up to 3 h.
- The runners' own API (`/api/actions/`) goes to Forgejo as nobody,
  never through the gate: the runner's token is its name there, and a
  switch that restarts the gate (67 s on node1 in 331) left the runner
  unable to report a job's end, which the forge then failed.

`nix.settings.eval-cache` is off on a runner box: it remembered derivation
paths a garbage collection had removed. The `<box>:host` label is how
`build_systems` builds every box's system on every box
([`ci.md`](ci.md)).

**The members' runner** gives every member CI without letting a job reach
the box. Docker's default runtime is gVisor, so a job sees a sandboxed
kernel. The container gets no host volume, no Docker socket, no secrets,
`no-new-privileges`, and a network that reaches the internet and nothing
of ours: the `DOCKER-USER` chain drops everything from its subnet
(`172.30.0.0/24`) to the tailnet, private ranges, link-local and
loopback, and `nixos-fw` drops anything from it to the box itself. It is
Docker's default bridge on purpose: on a user-defined network, Docker's
own name server is reached by NAT rules gVisor does not carry, and nothing
resolved. The `members-runner` VM test checks that a container reaches
none of it and sees gVisor's kernel.

**A release never kills a member's job either.** A member's job is a
container, so it dies when the members' runner stops (its default stop
cancelled the job) or when docker stops (dockerd stops its containers).
Both units are in `dd.runnerRestart.units` with a job counted as a
running container the members' runner started: each carries the label
`dd.runner=members-<box>` (the runner's container options; a workflow's
own options cannot set labels), and other containers on the box do not
count. A switch leaves both on their old config, and `dd-runner-restart`
restarts each once none of those containers runs. The runner's stop is a
drain, as the host runner's (`shutdown_timeout = 65m`, its job limit is
1 h; `KillMode = mixed`).

The wait is bounded, since a member who keeps jobs running back to back
would otherwise put off a fix to the sandbox for good:

- **Past the limit** (the 65 min drain and 10 min) the change goes in
  anyway. For docker: the members' runner is stopped, so it takes no new
  job and its jobs get the drain; once it is down docker restarts and the
  runner starts again. A job still running at the end of the drain is
  cancelled.
- **A change to the sandbox itself** does not wait for idle at all: the
  runtime (`runsc`'s path, so a new gVisor, and its flags), docker's
  `default-runtime`, and its network and iptables settings (`bip`, `icc`,
  `iptables`, ...), or the runner's container settings. The next pass
  stops the members' runner and **cancels the jobs running on the old
  sandbox** (their containers are killed, and the forge records them as
  failed); docker restarts on the pass after. A sandbox it cannot tell
  counts as a change: each start of docker or the runner records the
  sandbox's hash (a boot's too), so no record means one started before
  this release or a record that went, and that restarts the same way
  (once, on the release that brings this). The firewall's `DOCKER-USER`
  rules are not in this: a switch reloads the firewall, and they apply to
  running containers at once. The runner `wants`
docker and no more: under `requires`, a docker restart (a crash, say)
first stopped the runner, whose drain waited on a job that waited on
docker, for an hour. Docker's `live-restore` is not on: it keeps the
container through a restart of dockerd, but the runner runs each step
through dockerd (`docker exec`), and losing dockerd fails the step and the
job all the same (tried in the VM test). The `runner-restart` VM test runs a member's job
through a switch that changes the members' runner and docker's
`daemon.json`; then a member's job that never ends through one past a
shortened limit, beside a container that is no job's, and one through a
change to gVisor's flags.

## Storage and backups

```
/vault/forgejo     <bulk pool>/enc/forgejo: repositories, LFS, custom/conf
                   recordsize 128K, zstd, auto-snapshot; in the box's restic backup
postgres "forgejo" dumped hourly to /vault/backups/postgres (storage/postgres-backup.nix),
                   which is in the restic backup too
```

The state dir is its own dataset under the encrypted root. Forgejo waits
for `zfs-datasets` and the mount, or it would set itself up on the root
disk under the mountpoint. Each owner's repository sizes (`size +
lfs_size`) are counted from Forgejo's own table for the storage ledger
(`dd.usage.forgejo`, `storage/usage.nix`).

## The VM test

[`nix/tests/forge.nix`](../../nix/tests/forge.nix) boots one box with the
forge and the full gate, the state dir a plain directory. It checks, in
order ([`forge.py`](../../nix/tests/forge.py)):

1. Forgejo is on its socket, and nothing answers on port 3001.
2. `forgejo-admin` and `forgejo-protection` finish; with no repository,
   protection says "nothing to protect".
3. Once a repository exists, main's rule is `ci_ok`, up to date, no
   approvals, and no direct pushes; the only admin is the box's identity,
   and the repository's owner is a person, not an admin.
4. A private repository is refused, made or changed; a runner registered
   for every repository is moved to the fleet repository's owner, unless
   it is the members'. An account made an admin by hand is not one after
   `dd-forge-rules` runs again. (That `git.<domain>` answers 404 to
   `/api/v1/admin`, refuses a `sudo` query and drops the `Sudo` header is
   checked on every box's real nginx config: `checks.nginx`,
   [`nginx-door.py`](../../nix/tests/nginx-door.py).)
5. ci-resume's timer is set, and a pass over no runs fails nothing
   (a killed job run again: `runner-restart`).
   The CI identity is no admin, may write to the fleet's repository once
   `forgejo-admin` has run again, and cancels a run there through the
   socket as ci-cancel does.
6. No `DD_CI` secret; the cancel route answers 202 to a run id and 404 to
   anything else, and logs a run that does not exist as not found.
7. Live updates: nobody signed in gets 401 from `/_dd/git/events`; a
   member watching gets the commit status the test posts, with its sha;
   `dd-verify` may connect to the database and may not read a table.
8. Nobody, as the public door asks: reads and clones a public
   repository; does not see a limited organisation's repository by name,
   in a search or in its list, and git asks them to sign in for it (which
   the door turns into a 404); and is shown no account's address, even
   one set to public.
9. Cancelling by hand, through nginx (on in this test, with a throwaway
   certificate): a run no runner takes waits; nobody signed in gets 401,
   the demo and a member who may not write the repository are refused,
   and it keeps waiting. Once she may write: a cross-site POST, a foreign
   `Origin`, a GET and the public door are all refused; from the Git page
   (`Sec-Fetch-Site: same-origin`) it is 200 and Forgejo reports the run
   cancelled.

The door itself is the nginx check's (`nix build
.#checks.x86_64-linux.nginx`, which runs without a VM):
[`nginx-door.py`](../../nix/tests/nginx-door.py) serves the forge box's
own nginx config in the build, with stand-ins for Forgejo's socket and the
gate that write down what reaches them, and asks as an outsider through
the tunnel's listener. Every address above that is let through must reach
the right one, with no cookie, `Authorization`, `Sudo` or user header
however the request was dressed, and come back with no cookie and the
door's content policy; every refusal must reach neither, and a private
repository's 401 must come back a plain 404. It also checks that
`git.<domain>` closes the connection on an outsider and that a flood of
ref listings and of archives is told to wait. Each case is printed in the
build log.

It is one job in CI's `vm_tests` matrix.

## Changing it

- **A Forgejo setting** goes in `forgejo.nix`, with the reason beside it.
  Anything set by hand in Forgejo's admin pages is lost on a rebuilt box.
- **Never open another way to Forgejo.** No TCP port, no unit added to the
  `forgejo` group unless it is meant to act as anyone it names. A new
  nginx location to Forgejo uses `toForge`, so the header is always the
  gate's.
- **A new Forgejo address** (a page or file that must come from Forgejo
  itself) goes in nginx's list in `forgejo.nix` *and* in `FORGE` in
  `git-core.js`.
- **A view for anyone** is `route(pattern, view, true)`: it must read
  only what nobody may read, and every API path it calls must be on the
  door's list in `door.nix`, and its address on the door's page list. Add
  both to `nginx-door.py`.
- **A new view** is a `route()` in the module it belongs to, using
  `api()`, `current()` and the shared pieces in `git-core.js`. Follow
  [ui.md](ui.md): draw from the kept copy first, ask in parallel, warm the
  tab, every state drawn. Measure with `git:shown`/`git:drawn`.
- **Something new to watch live** is another trigger in `live.nix` (ids
  and a state only, never content) and a `watch()` in the view. The forge
  VM test should see it arrive.
- **main's rule** changes in `forgejo.nix` (`RULE`), never only on the
  Settings page. If it names a new check, `ci_ok` is still the one to
  name; see [ci.md](ci.md).
- **A runner change** is checked by the VM tests that use it: a sandbox
  change by `members-runner`; anything a host job needs on its path goes
  in `hostPackages`. A unit on a timer follows the rules in
  [ci.md](ci.md#when-a-release-does-not-land).
