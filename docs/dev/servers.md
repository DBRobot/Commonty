# Servers and their layout

`servers/` is everything that runs on a box: our own programs, the pages
the gate serves, and the patches to the upstream programs we package. This
page is how that folder is laid out, and how one service goes from its
folder to a page a member opens on a box: served, placed, built and tested.

## The top of the tree

```
servers/   what runs on a box: programs, pages, patches
client/    what runs on people's devices: the dd cli, its libraries, the browser wasm
app/       the app: the same client crates behind a window, and copies of the pages
nix/       how a box is made of those: modules, bundles, tests
scripts/   what a person runs around a box: reports, reinstall, restore, the laptop
secrets/   each box's secrets, sops-encrypted
fleet/     what the fleet is: the boxes, their reports, the members, and the
           owner's policy (policy.json: owner, domain, repo, allowanceGb,
           people.<name>.{ssh,paperKey})
data/      data the services are built with (data/games/catalogue.json)
```

`client/`, `fleet/`, `data/`, `nix/`, `scripts/` and `secrets/` have pages
of their own: [the client](client.md), [the fleet](fleet.md),
[data](data.md), [Nix](nix.md), [scripts](scripts.md) and
[secrets](secrets.md).

The Rust is one cargo workspace ([`Cargo.toml`](../../Cargo.toml)); its
members are the crates in `servers/`, `client/` and `app/`. The one Go
program on a box is the control plane, in `servers/system/control`.

## A service's folder

A service has a folder of its own, `servers/<service>/`, and holds only the
parts it has:

```
servers/<service>/
  Cargo.toml, src/     its own program (a crate in the workspace)
  web/
    pages/<page>.html  its pages, as files: the frame, no inline script
    <page>.js          each page's module, beside it
    <page>.css         what the page adds to the shared look
    icons/             pictures the pages load
  templates/           askama templates, rendered on the server
  *.patch              changes to an upstream program we package
```

Most services have no program of their own. Their pages are served by the
gate, and what runs on the box is either the gate or an upstream program
from nixpkgs that their NixOS module sets up. The crates' names are not
always the folder's:

```
servers/gate            crate verify        the gate's old name, still in its unit (dd-verify),
                                            its module (nix/modules/gate/verify.nix) and its
                                            environment (VERIFY_*)
servers/library         crate library-gate
servers/system/release  crate release       binaries dd-agent, dd-attest
```

## The special folders

- **`gate/`** is the front door. It answers nginx's sign-in check for every
  service on the box, keeps the sessions and the directory, and serves every
  page in `servers/*/web/`. Its `src/` holds the program, with one module
  per service it speaks for (`adblock.rs`, `photos.rs`, `library.rs`,
  `docs.rs`, ...); `templates/` the few pages it renders (sign-in, join,
  enrol, waiting, downloads); `web/` the sign-in scripts and the platform
  icons. It runs on every box
  ([`nix/roles/core.nix`](../../nix/roles/core.nix)).
- **`web-shared/`** is what every page uses: `home.css` and `bar.css` (the
  look), `kit.js` (fetching, elements, states), `shell.js` (the bar),
  `webauthn.js`, the fonts, the favicon and the service icons. It has no
  `web/` of its own: the folder is the shared web. [UI guidelines](ui.md)
  is how to use it.
- **`home/`** is the home pages: home, settings, friends, a friend's link,
  and add-a-device. They belong to no one service.
- **`system/`** is how a box runs itself. `system/release` is the release
  agent (`dd-agent`, which moves the box to a signed release, and
  `dd-attest`, which vouches for one) and the release format the cli signs.
  `system/control` is the control plane: `dd-control` on every box, the
  witness, and the relay Worker ([the control plane](control-plane.md)).

## The services

One line per folder. "Module" is under `nix/modules/`; "test" is a VM test
in `nix/tests/` unless it says otherwise; "crate tests" run in
`checks.tests`.

| Folder | What it is | Code and module | Tests | Page |
|---|---|---|---|---|
| `adblock` | Pi-hole for the house, switched from its page | page; `adblock/pihole.nix`; gate `adblock.rs` | `adblock` | [packaged.md](packaged.md) |
| `chat` | the Chat page at `llm.<domain>` | page; `llm/llm.nix`, `llm/llama-cpp.nix`, `llm/search.nix` | `llm` | - |
| `docs` | these pages, at `<domain>/docs` | page; gate `docs.rs`; `roles/gateway.nix` | crate tests, lint's docs links | - |
| `files` | the Files page, a member's encrypted library | page; `media/webdav-media.nix` | `library` | [libraries.md](libraries.md) |
| `fleet` | Settings' Backups, Devices and Network tabs | scripts and a stylesheet; gate `fleet.rs`, `network.rs`, `signins.rs` | crate tests, `network` | - |
| `forge` | the Git pages over Forgejo's API | page; `forge/forgejo.nix`, `forge/live.nix` | `forge`, `members-runner` | [forge.md](forge.md) |
| `games` | the games manager and its pages | `src/`, `templates/`, `web/`; `games/games.nix` | `games`, crate tests | [games.md](games.md) |
| `gate` | the front door | `src/`, `templates/`, `web/`; `gate/verify.nix` | crate tests, `directory`, `network`, `library`, `checks.nginx` | [gate.md](gate.md) |
| `home` | the home, settings, friends and add-a-device pages | pages; gate `home.rs`, `friends.rs`, `adddevice.rs`, `storage.rs` | crate tests | [gate.md](gate.md) |
| `library` | the libraries bucket behind the gate: presigned urls, listing, trash, WebDAV; members' disk images (`/_dd/images`) | `src/` (in the gate); `library/libraries.nix` | `library`, crate tests | [libraries.md](libraries.md) |
| `mail` | the Mail page: a member's locked inbox, opened on their device | page; `library/mail.nix`; gate `mail.rs`; the mail Worker (`client/mail`), `client/mailsend`, the app's `mail.rs` | `mail-worker`, `mail-page`, crate tests | [mail.md](mail.md) |
| `media` | the Movies & TV page and its player | page; `media/webdav-media.nix` | `library`, `transcode` | [libraries.md](libraries.md) |
| `observe` | the Metrics page over Thanos | page; `observe/metrics-ui.nix` | `metrics`, `thanos`, `metrics-ui` | - |
| `smarthome` | Smart home: the door in front of Home Assistant on a household's home controller (a Pi, not a box), and Home Assistant's Commonty theme | `door/` (crate smarthome-door), `web/`, `theme/`; `smarthome/home-assistant.nix`, `smarthome/door.nix`, `smarthome/tang.nix`, `smarthome/cache-door.nix`; the system in `nix/controller` | `home-controller`, `dns-worker`, crate tests | [home-controller.md](home-controller.md) |
| `status` | the uptime page at `<domain>/status` | page; gate `status.rs`; `observe/uptime.nix` | crate tests, `thanos`, `checks.nginx`, `site-headers`, `site-a11y` | [gate.md](gate.md#status-statusrs) |
| `photos` | Ente, patched, and the gate's Photos sign-in | patches, `templates/`, `web/`; `photos/ente.nix`; gate `photos.rs`, `demo_photos.rs` | `photos`, `pgrestore`, crate tests | [packaged.md](packaged.md) |
| `system/control` | the control plane | Go; `box/control.nix` | `checks.control` | [control-plane.md](control-plane.md) |
| `system/release` | the release agent and attestation | `src/`; `box/agent.nix` | `release`, crate tests | [releases.md](releases.md) |
| `transcode` | one file for one viewer, decrypted in memory and transcoded | `src/`; `library/transcode.nix` | `transcode` | [libraries.md](libraries.md) |
| `vault` | Vaultwarden, patched, and the public Send page | patch, `web/`; `vault/vaultwarden.nix`, `vault/pwned.nix` | `vault` | [packaged.md](packaged.md) |
| `web-shared` | the shared look, bar, kit, fonts and icons | files only | `checks.stylesheet`, crate tests | [ui.md](ui.md) |

## How a page is served

```
browser ── https://<host>.<domain>/... ──► nginx on the box
   │                                         │ the page's location: proxy to the gate,
   │                                         │ e.g. git.<domain>/ ─► /_dd/git
   │                                         ▼
   │                                       gate (127.0.0.1:4181)
   │                                         signed in? no ─► /_dd/login?rd=...
   │                                         yes ─► the page's file, as it is
   │◄── servers/forge/web/pages/git.html ────┘
   │
   ├── /_dd/static/git.js, home.css, ... ─► gate: STATIC, by name
   ├── /_dd/web/marked.js, dd_web.wasm  ─► gate: the web bundle (.#web)
   └── /_dd/me ─► gate: who is looking, the bar's menu, the tiles
```

Everything the gate serves as a file is listed in
[`servers/gate/src/pages.rs`](../../servers/gate/src/pages.rs), in four
tables:

```
STATIC    (name, path in the tree, type)   served at /_dd/static/<name>
PAGES     (name, path in the tree)         a page, sent by its route's handler
ICONS     (key, path)                      a tile's mark, drawn into /_dd/me
OS_ICONS  (key, path)                      a platform's logo on the downloads page
```

- **A file's URL is its name, not its path.** `servers/forge/web/git-diff.js`
  is `/_dd/static/git-diff.js`. The names are one flat space across every
  service, so a new file needs a name no other service uses: a service's
  files carry its name (`git-*.js`, `demo-photos.css`). A page's module
  imports the others as `./kit.js`, which the browser asks for at
  `/_dd/static/kit.js`.
- **A page has a route.** `PAGES` names the file; a handler in
  [`servers/gate/src/lib.rs`](../../servers/gate/src/lib.rs) decides who
  gets it (`signed_in_page`, or its own check, like `files_page` letting
  the demo in only where the box has a demo library) and sends a signed-out
  browser to `/_dd/login`. The page itself holds nobody's name: it fills in
  from `/_dd/me` (`shell.js`).
- **Read once, at start.** The gate reads every file in the four tables
  when it starts (`pages::load`), from `VERIFY_PAGES_DIR`. A file that is
  missing stops the gate starting, rather than a page answering 404. Each
  file gets an ETag from its bytes: a browser asks every time
  (`no-cache`) and is told 304 when it already holds them. Fonts are
  cached for a week.
- **`/_dd/web/`** is the other directory: our Rust compiled to wasm
  (`client/web`), and the players and libraries a page loads (`hls.js`,
  `marked.js`, `purify.js`), pinned by hash in the flake (`webDist`). It is
  `.#web`, handed to the gate as `VERIFY_WEB_DIR`. Only `.js` and `.wasm`
  are served from it.
- **The pages are their own derivation.** `.#pages` in
  [`flake.nix`](../../flake.nix) is the tree filtered to
  `servers/web-shared`, every `servers/<service>/web` and `docs/`, laid out
  as in the tree. A page changing makes a new pages directory and the same
  gate binary. In its tests the gate reads the files from the checkout
  instead (`pages::tree`).

### The content policy

[`nix/modules/gate/headers.nix`](../../nix/modules/gate/headers.nix) adds
the headers to every nginx location, by where the location proxies:

```
to the gate (:4181) or the games manager (dd-games.sock)
    our pages: scripts from this site only, no inline script, wasm allowed,
    data from our own names (and TMDB for Movies & TV); reports to /_dd/csp
to Vaultwarden (:8222)
    HSTS only: it sends its own set
anything else
    HSTS, nosniff, referrer policy, frame-ancestors 'self'
```

A page that needs another origin (a new API it calls from the browser) is a
change to `ours` in that file. What the policy blocks is logged by the
gate (`csp_report`).

### The app's copies

The app serves the signed-in pages members use from copies it carries, at
the site's own paths ([`app/src/site.rs`](../../app/src/site.rs)).
[`app/carried`](../../app/carried) lists them, one line each:

```
servers/files/web/pages/files.html    pages/files.html
servers/files/web/files.js            static/files.js
```

[`scripts/sync-app-web`](../../scripts/sync-app-web) copies them into
`app/web/_dd/`. The flake's `checks.stylesheet` compares every copy with
its source and fails while one differs, or while `app/web/_dd` holds a file
the list does not name. A change to a carried file changes its copy in the
same pull request.

### The exceptions

- **Games** serves its own pages. The manager is its own program, behind
  `games.<domain>`: its templates in `servers/games/templates`, its
  `games.css` and `games.js` compiled in (`include_str!`). It still gets the
  content policy, and the same sign-in check in front.
- **Photos** is Ente's own web app, patched (`servers/photos/*.patch`,
  applied in [`nix/modules/photos/ente.nix`](../../nix/modules/photos/ente.nix)),
  with the shared `bar.css` copied in at build. The gate's part is the
  sign-in step (`servers/photos/templates/photos.html`, found by askama
  through [`servers/gate/askama.toml`](../../servers/gate/askama.toml))
  and the demo's Photos. The app serves its own copy of the client from
  its release ([`app/src/local.rs`](../../app/src/local.rs)).
- **Passwords** is Vaultwarden, patched
  (`servers/vault/vaultwarden-hash-refresh.patch` and
  `servers/vault/vaultwarden-app-loopback.patch`) and restyled
  (`user.vaultwarden.scss.hbs`, built into the web vault's own
  `/css/vaultwarden.css` by `servers/vault/web/vaultwarden-css.py`). nginx puts the bar into its pages with a
  `sub_filter`, and sends `/_dd/static/` on `vault.<domain>` to the gate.
  The app serves its own copy of the web vault from its release
  ([`app/src/local.rs`](../../app/src/local.rs)).

## How a service gets onto a box

```
nix/lib/fleet-placement.nix   node1: core, gateway, ..., adblock, from its capability
   │                       (its report, box.json, secrets file; nix/roles/bundles.json)
   ▼
nix/roles/adblock.nix   imports the modules; its secrets; its tile; its backups
   │
   ▼
nix/modules/adblock/pihole.nix   options.dd.adblock, the units, the gate's environment
nix/modules/adblock/needs.nix    what it needs of a box (typed, nix/lib/needs-type.nix)
   │
   ▼
flake.nix   nixosConfigurations.node1 = core modules + every bundle it is built with
```

### Bundles

A bundle (`nix/roles/<bundle>.nix`) is modules that move together, with
what they read from sops, their tile on the home page
(`dd.home.services`), and the paths to back up (`dd.backup.paths`).
Nobody gives a box a bundle. The flake builds one `nixosConfiguration`
per box: the bundles its capability allows
([`nix/lib/fleet-placement.nix`](../../nix/lib/fleet-placement.nix)
`carried`), and the modules every box gets (secure boot, disk unlock,
the control plane, ports, the box report).

[`nix/roles/bundles.json`](../../nix/roles/bundles.json) says what each
bundle brings (`services`, by the needs' names), what its modules read
from the box's own sops file (`secrets`), and which bundles must share the
box (`with`: they reach each other on localhost). A box is built with
core, and with every bundle of which it can run a service that does not
run everywhere, whose secrets its file holds and whose companions it is
built with too. What it can run is
[`capability.nix`](../../nix/lib/capability.nix)'s, from its signed
report and its owner's `box.json`.
The control plane then switches each placed service on where the plan
puts it.

Every box also keeps itself tidy ([`nix/roles/core.nix`](../../nix/roles/core.nix)
and the modules it names):

- `nix.gc` weekly, keeping 14 days, with min-free 10G and max-free 30G;
- nix-daemon held to `MemoryHigh` 60% and `MemoryMax` 75% of RAM, and
  max-jobs one per 6G, at most three for every four threads (a crate's
  build is mostly one thread): node1 6, node2 2;
- builds and CI together (the nix daemon, the build orders' `dd-build`,
  the host runner's jobs) in `work.slice`, held to all of RAM but a fifth,
  at least 3G, at most half ([`nix/lib/work-memory.nix`](../../nix/lib/work-memory.nix)):
  node1 50G, node2 11.9G. Past it the kernel kills inside the slice, and
  the runner (`OOMPolicy=continue`) fails only the job whose process died;
- the nix-cache bucket cleaned daily, keeping the last 10 releases'
  closures plus 7 days ([`cache-gc.sh`](../../nix/modules/storage/cache-gc.sh),
  where the forge runs);
- the runner's cargo-target aged out after 14 days, and docker pruned
  weekly on the members' runner;
- Prometheus keeping 30 days locally.

### Needs, and placement

Each module area has a `needs.nix` beside it: per service, what it is,
what it uses (memory and storage, measured where it runs), what it
requires and what makes it faster, who reaches it and what it reaches,
what it depends on, and what of people's data it can read. The shape is
typed in [`nix/lib/needs-type.nix`](../../nix/lib/needs-type.nix), so a
needs file that says something unknown fails to evaluate:

```
per service      what, units, instances, perHouse, everyBox, servesCode, when,
                 trust, traffic, alwaysOn, spread, grows, moves, base/variants
trust            owner, household, own or any; every service off everyBox says one
secrets          what it reads from the box's own sops file, by name
traffic          class (high, medium, low), upMbps, downMbps
grows            photos, libraries, code, passwords
requires/faster  arch, gpu, memory, disk, place, threads, windowsLicense, kvm,
                 nestedKvm, uefi, wiredInitrd, tpm
uses             memory, storage, estimated
network          outbound: none, internet or a list of hosts
                 inbound: none, localhost, lan, network or public
depends          <svc>.local
privacy          none, metadata or plaintext
```

[`nix/lib/needs.nix`](../../nix/lib/needs.nix) lists every file and merges
them; `.#needs` is the result as JSON.

`units` names the systemd services and timers a service is made of besides
one named after it (a name ending in `*` covers a template's instances, or
a name made at build time). Every unit this repository's modules put on a
box belongs to some service, and no timer of ours counts from boot:
[`nix/tests/needs-units.nix`](../../nix/tests/needs-units.nix) fails the
flake otherwise.

Placement has two halves. What a box *can* run is decided in the release;
what it *does* run is decided by the control plane.

**Can.** [`nix/lib/capability.nix`](../../nix/lib/capability.nix) judges
every box against every service, one service at a time, and never by load.
A box can run a service when it has what the service requires (from its
signed report and its owner's box.json), when the service's memory and
storage floor fit, when the box is one its `trust` allows (the fleet owner's
boxes; or a box of a household it serves; `own`, its box's owner's things, or
any), when its line is not
metered for `high` traffic, and when everything the service needs on the
same box can run there too. A box that cannot run what every box runs
(`everyBox`) cannot join. For each service it can run, the box gets a
score from what is known before anything runs: what makes the service
faster, memory to spare, mains power for what must always answer, threads,
room for the pool. A line no box.json calls flat (`internet.metered:
false`) counts as metered. The owner's own boxes usually come out able to
run nearly everything; that is the point. Whether they do is the control
plane's call.

The release ships this table, from the reports in `fleet/boxes`, and each
box also judges itself: `dd-capability`
([`box/capability.nix`](../../nix/modules/box/capability.nix)) runs the
same `capability.nix`, with the release's needs (`.#needs`), policy and
the box's own box.json, on the report the box made at this boot
(`/var/lib/dd/report/hardware.json`). It writes
`/var/lib/dd-facts/can.prom` (`dd_can_live 1`, `dd_can{service=…}`
scores), which the heartbeat carries. It runs at boot after the report
and again on every switch whose needs or rules changed; on no report or a
failed judgment it removes the file and the release's table stands. Its
judgment only narrows the release's table (a box that lost memory or a
disk stops being given what no longer fits), never widens it: no box's
own word gives it work. A box that gains a GPU or memory is placed for it
once its new report is in a release (`scripts/box-report`). Running the Nix rules on the box, not a Go copy of them, keeps
one set of rules; `checks.placement` fails if a box judged this way, on
the release's report, would come to anything but the release's table.

`checks.placement` ([`nix/tests/placement.nix`](../../nix/tests/placement.nix))
fails when a box cannot join, when it carries a service it cannot run, or
when it runs a service none of its bundles lists.
`checks.placement-fleet` holds the same rules to a dozen made-up computers
(a mini PC, NASes, Xeon servers, a gaming PC, laptops, a Pi, a VPS, a thin
client, an office PC, in
[`placement-fleet/fleet.json`](../../nix/tests/placement-fleet/fleet.json))
and to answers a person would give: no TPM, no joining; postgres wants an
SSD; the biggest pool ranks first for storage. A change that makes those
answers stop making sense fails there, by name.

**Does.** `.#placement-inputs`
([`nix/lib/placement-inputs.nix`](../../nix/lib/placement-inputs.nix)) is
the table the control plane places from: each box's scores, what its
owner's box.json or the policy bar it from whatever its hardware
(`barred`: trust, a metered line), what it is built with (`built`), and
per service whether the fleet wants it (some box is built with it, and
the policy setting in its `when` is on), how many, spread or by house, the
line it wants, its memory, what moving it carries, and `moves`: `better`
(the default) or `on-failure`; and what a box that joined on its own key
may run (`joined`: `services`, given nothing until `attested`; `named`, the same
with a name under the domain, built and carried but not given; `own`, its
owner's or their household's things). How the control plane chooses is in
[the control plane](control-plane.md#placement).

Each box measures what it can tell only by running: its line, once a day
against Cloudflare's speed endpoints, unless it is metered
([`netspeed.nix`](../../nix/modules/box/netspeed.nix)); and what each
service's units hold in memory at most, every 5 minutes
([`service-memory.nix`](../../nix/modules/box/service-memory.nix)). Both
go into its heartbeat.

### Ports

Every fixed port a service listens on is in `dd.ports`
([`nix/modules/box/ports.nix`](../../nix/modules/box/ports.nix)). The
nixpkgs services are listed there, by whether the box runs them; ours are
listed there too, by their unit. Two services claiming one port fail the
build, not the box's switch, and an hourly `dd-ports-check` names any port
that is listening and was never declared. Ports from 32768 up are the
kernel's and are not listed. The gate is on 4181, the control plane on
4195.

### nginx and signing in

A service's site is an nginx virtual host in its module,
`useACMEHost = config.dd.domain; forceSSL = true;`. Its locations are of
two kinds:

```nix
# the page: the gate's, which checks who is looking itself
locations."= /" = {
  proxyPass = "http://127.0.0.1:${toString config.dd.verify.port}/_dd/chat";
  extraConfig = "proxy_set_header X-Original-URI $request_uri;";
};
# the service: nginx asks the gate first
locations."/v1/" = {
  proxyPass = "http://unix:/run/dd-llm/llm.sock";
  extraConfig = ''
    auth_request /_dd/verify;
    error_page 401 = @login;
    error_page 403 = @waiting;
  '';
};
```

- **`auth_request /_dd/verify`** sends every request to the gate's
  `/verify` first. It answers 200 with `X-Auth-Request-Preferred-Username`
  and `X-DD-Role` (member, demo, guest), 401 for nobody (`@login` sends a
  browser to sign in and back), or 403 for someone signed in who may not
  (`@waiting` sends them home). A service that wants the name takes it with
  `auth_request_set` and passes it on itself, as games does with
  `X-DD-User`.
- **The host has to be one the gate stands in front of.**
  `dd.verify.hosts` in
  [`nix/modules/gate/verify.nix`](../../nix/modules/gate/verify.nix)
  is the list of subdomains that get the gate's `/_dd/` paths, `@login`,
  `@waiting` and the `/_dd/verify` subrequest. Lists from every module add
  up: a new service's module adds its own subdomain.
- **The session cookie never reaches a service.**
  [`strip-cookie.nix`](../../nix/modules/gate/strip-cookie.nix) takes
  `dd_session` out of every location that proxies anywhere but the gate.
- **Public or not.** Every host is on the fleet's network only, unless it
  is in `dd.public.hosts`
  ([`nix/modules/gate/public.nix`](../../nix/modules/gate/public.nix)),
  which puts it behind the Cloudflare tunnel. A tile with `demo` set gets a
  demo door, `demo-<service>.<domain>`, that lets the demo session in and
  nobody else.

### Secrets

A role that reads a secret imports
[`nix/roles/_sops.nix`](../../nix/roles/_sops.nix) and declares it:
`sops.secrets.<name> = { };`, with a `sops.templates` entry where a
program wants an environment file. Each box's secrets are in
`secrets/<box>.yaml`, encrypted to that box's TPM key and to the editor's;
no box can read another's. They are edited with
`dd secret run -- set secrets/node1.yaml '["name"]' '"value"'`. A box with
no role that declares a secret has no sops at all. [Secrets](secrets.md)
has the whole of it.

### Talking to the gate

A service the gate speaks for (adblock, photos, libraries, the network)
gets its settings into the gate through the gate's unit, from its own
module:

```nix
systemd.services.dd-verify = lib.mkIf config.dd.verify.enable {
  environment.VERIFY_ADBLOCK = "http://127.0.0.1:8053";
  serviceConfig.LoadCredential = [ "pihole-password:${cfg.passwordFile}" ];
};
```

[`servers/gate/src/main.rs`](../../servers/gate/src/main.rs) reads them
into the gate's config; unset, that part of the gate is off.

### What a module also says

- `dd.box.plaintext = [ "<service> (what it reads)" ];` when it reads
  people's plaintext. Nothing refuses it; the box publishes the list as a
  metric, so it stays in view.
- `dd.backup.paths` for state that has to survive the box.
- A unit on a timer never fails during a switch
  ([CI: when a release does not land](ci.md#when-a-release-does-not-land)).
- A service that keeps members' data counts it into the allowance: a
  `dd.usage.<service>` query
  ([`nix/modules/storage/usage.nix`](../../nix/modules/storage/usage.nix),
  with `counts` and `memberSuffix`), which becomes `usage/<service>/<member>`
  facts in the box's heartbeat. A new kind of data also needs its value in
  `counts`, its field in the gate's `Usage`
  ([`storage.rs`](../../servers/gate/src/storage.rs)) and its service in
  storage-view's list of who may count what (`KINDS` in
  [`storage_ledger.rs`](../../client/cli/src/storage_ledger.rs))
  ([libraries.md](libraries.md#storage)).
- How long it keeps what someone deleted, so that the sum stays within 90 days (below).

### Retention

Anything deleted is gone from every box and every backup within 90 days.
Each place keeps its own window:

```
ZFS snapshots      15-minute x4, hourly x24, daily x7: a week. Weekly and
                   monthly are masked on every box, and dd-snapshot-trim
                   (every 6 h) destroys any automatic snapshot over 7 days
                   (storage/snapshots.nix)
restic             --keep-daily 7 --keep-weekly 4, pruned after every
                   nightly backup: under 5 weeks (storage/backup.nix)
garage metadata    its snapshots 7 days (storage/backup.nix)
postgres dumps     hourly, one .prev; older ones only in the two above
library trash      30 days (servers/library/src/dav.rs TRASH_SECS), or at
                   once with Delete forever or Empty trash
Passwords          TRASH_AUTO_DELETE_DAYS=30, EVENTS_DAYS_RETAIN=30
someone who left   nothing kept: deleted at once (leaving.md)
someone removed    30 days (servers/gate/src/leaving.rs WINDOW)
Photos             museum's trash: 30 days (a constant, no setting), or at
                   once when emptied; then the file's objects at museum's
                   next run (museum-delete-now.patch; 45 days upstream)
nginx access log   3 weekly files (gate/acme.nix)
journald           MaxRetentionSec=14day (roles/core.nix)
```

The windows follow one another, so the promise is their sum: something in
a trash for 30 days can still be in a nightly backup for 5 weeks, and in
the snapshots of that backup for a week, 72 days in all. A photo goes
after 30 + 0 + 7 = 37 (82 before museum was patched). Leaving and removal have their own sums
([leaving.md](leaving.md#the-guarantee)), 90 at most.

[`nix/tests/retention.nix`](../../nix/tests/retention.nix)
(`checks.retention`, in CI's flake check job) adds them up for every box
and fails past 90 days: each first window (the library trash and a
removal, read from the Rust; Vaultwarden's two settings, where unset is
forever; museum's trash, account and object delays, read from the patched
source each box runs), then the longest restic keep (a backup that is never pruned, or
keeps a way it cannot read, fails too), then the snapshots. Journald and
the nginx log are held to 90 days on their own. A new service that keeps
deleted things for a time adds its setting there.

## How it is built

[`flake.nix`](../../flake.nix) builds the Rust two ways.

- **The programs on the boxes** (`.#verify`, `.#agent`, `.#games`,
  `.#transcode`, and `.#dd`) are built per crate from
  [`Cargo.nix`](../../Cargo.nix), which crate2nix makes from `Cargo.lock`.
  A dependency moving rebuilds that crate and what uses it. Each crate's
  source is its own folder. After `Cargo.lock` changes:
  `nix run nixpkgs#crate2nix -- generate`; `checks.cargo-nix` fails while
  the two disagree.
- **The lint checks, the app's tests, the app and the wasm** are built
  with crane. `srcFor`
  filters the tree to what one build needs: the workspace's manifests, the
  folders it is built from (`crateSrc` follows the path dependencies in the
  manifests, so the list does not go stale), with their templates. Every
  workspace member it leaves out gets an empty stub target, since cargo
  loads the whole workspace. Dependencies build once, from `depsSrc`, which
  is the lockfile and the manifests alone, so a page or a source file
  changing never rebuilds them.
- **The pages** are `.#pages`, apart from the gate (above).
- **The control plane** is Go: `dd-control` with `buildGoModule`, the
  witness as wasm from the same vendored modules, and `.#control-worker`,
  the relay Worker `dd release publish` uploads.

## How it is tested

- **Crate tests**: `checks.tests` runs every workspace crate's tests but
  the app's (`checks.tests-app`), each its own build from `Cargo.nix`
  inside the box program that carries it, cached crate by crate
  ([CI](ci.md#how-a-run-fits-together)). `checks.clippy` lints each crate
  as its test build is compiled (clippy standing in for rustc, `-D
  warnings`), and `checks.fmt` and `checks.clippy-app` run beside it. The gate's tests read the
  pages from the tree, so a page that names a script the table does not
  have fails there.
- **VM tests**: boxes booted and driven. A test is two files,
  `nix/tests/<name>.nix` (the boxes, built from the same modules the real
  boxes import, usually on top of
  [`nix/tests/box.nix`](../../nix/tests/box.nix)) and `nix/tests/<name>.py`
  (the scenario). [`nix/lib/vm-test.nix`](../../nix/lib/vm-test.nix) hands
  the script one name, `nix`, holding what the Nix side computed (its
  `scriptEnv`), so nothing is pasted into the Python. Run one with
  `nix build .#checks.x86_64-linux.<name>`.
- **Evaluation checks**, seconds each: `boxes` (no box names a role,
  every bundle is a file, every box builds), `placement`, `nginx` (every
  box's config through
  `nginx -t`), `stylesheet` (the app's copies), `cargo-nix`, `ci`,
  `plaintext-label` (a module that reads plaintext labels itself),
  `box-reports`, `net-check` (the app's network engine: go vet and its
  tests), `control` (the control plane, offline), `mail-worker` (the
  mail Worker in a browser) and `dns-worker` (the DNS Worker at its API). `clippy-app` and `tests-app` are the app's
  lint and tests.

A VM test is listed twice: in `vmTests` in `flake.nix`, and in the
`vm_tests` matrix in
[`.forgejo/workflows/entry_point.yml`](../../.forgejo/workflows/entry_point.yml).
`checks.ci` fails when the two differ. In CI, `files_modified` decides what
runs: a change under `servers/` turns on the nix, rust and VM parts, a
change under `docs/` only the nix part (the pages are built). The VM tests
run as one job each, after `build_programs` has built the box programs
([CI](ci.md)).

## Adding a service

Modelled on ad blocking: an upstream program, a page, the gate as its only
door, and one role.

1. **Its folder.** `servers/<service>/`, with only what it has:
   - a page: `web/pages/<service>.html` from an existing page's frame, and
     `web/<service>.js` and `.css` beside it ([UI guidelines](ui.md));
   - a program: `Cargo.toml` and `src/`, a member in the root
     `Cargo.toml`, then `nix run nixpkgs#crate2nix -- generate`, a package
     in `flake.nix` (`perCrate "<crate>"`), and the package in the `boxes`
     part of `build_programs` in `entry_point.yml`;
   - an upstream program: nothing here unless it needs a patch,
     `servers/<service>/<what>.patch`, applied in its module.
2. **Its page in the gate.** Each file in `STATIC`, under a name no other
   service uses, and the page in `PAGES`
   ([`servers/gate/src/pages.rs`](../../servers/gate/src/pages.rs)). A
   route in `servers/gate/src/lib.rs` that checks who is looking. What the
   page asks of the box goes in a module of the gate,
   `servers/gate/src/<service>.rs`, with its routes beside the page's, and
   its settings read in `main.rs` from `VERIFY_<SERVICE>*`. Add the page to
   the gate's page tests.
3. **The app**, if members use it there: its files in `app/carried`, then
   `scripts/sync-app-web`.
4. **Its module.** `nix/modules/<service>/<program>.nix`:
   `options.dd.<service>` with `enable`, the units, the gate's environment
   (`systemd.services.dd-verify`, above), the nginx host with the page
   proxied to the gate and anything else behind `auth_request`. A new
   subdomain goes in `dd.verify.hosts`; a public one in `dd.public.hosts`.
   Its ports go in the list in `ports.nix`, by its unit, whether the
   program is ours or nixpkgs'. `dd.box.plaintext` if it reads
   people's data. A `dd.usage.<service>` count if it keeps members' data
   (above).
5. **Its needs.** `nix/modules/<service>/needs.nix`, measured where it
   runs (`estimated = true` until it has run anywhere), and the file in
   the list in `nix/lib/needs.nix`.
6. **Its bundle.** `nix/roles/<bundle>.nix`: the module, `./_sops.nix`
   and its `sops.secrets` if it has any, `enable = true`, its tile in
   `dd.home.services` (with `demo` set if the demo may look), its
   `dd.backup.paths`. The bundle in `nix/roles/bundles.json` with its
   `services`, `secrets` and `why`; every box able to run it is then
   built with it. Its secrets in `secrets/<box>.yaml` with `dd secret
   run`, before the release that needs them.
7. **Its test.** `nix/tests/<service>.nix` and `.py`, booting
   `./box.nix` and the module with test values for the secrets, driving
   the failure cases as well as the working one. The name in `vmTests` and
   in the `vm_tests` matrix.
8. **Its page here.** A line in the table above, and a page of its own
   under `docs/dev/` linked from [`docs/README.md`](../README.md) when it
   has more to say than a line.
