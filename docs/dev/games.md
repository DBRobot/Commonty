# Game servers

A member picks a game on the Games page, presses Start, and a dedicated
server for it comes up on a box, reachable by the friends they invite. This
page is how that works: the catalogue, the manager, one VM per world, the
ports and the firewall, the memory budget, guests, and how to change it.

The parts:

```
data/games/catalogue.json          every game that can run: pelican eggs, resolved
data/games/resolve.py, sizes.py    how the catalogue is made (run by hand, committed)
data/games/covers/<steam id>.jpg   the covers
servers/games/                     the manager (Rust, axum): pages, records, units
nix/modules/games/games.nix        the manager's service, the dd-game@ template, the firewall chain
nix/modules/games/game-guest.nix   the guest machine, the same for every game
nix/modules/games/game-run.py      what runs in the guest: install, configure, run, report
nix/modules/games/games-access.sh  fills the firewall chain from the manager and the gate
nix/modules/games/games-measure.sh records each game's real memory peak
nix/modules/games/needs.nix        what the role asks of a box, per game
nix/roles/game.nix                 the role: the module, the home tile, the backups
nix/tests/games.nix, games.py      the VM test
```

## How it fits together

```
member's browser                                  member's / friend's game client
  │ https://games.<domain>/                         │ <box net address>:<port>
  ▼                                                 │ over the Commonty network (commonty0)
nginx ── auth_request /_dd/verify ──► gate          ▼
  │      X-DD-User, X-DD-Role from its answer     nixos-fw ── dd-games chain: one rule per
  ▼                                                 │          invited device and port
/run/dd-games/dd-games.sock                         ▼
dd-games (the manager, user dd-games)           qemu hostfwd (tcp+udp, same number)
  │ records/<id>.json   (the host's truth)          │
  │ instances/<id>/     (shared with the guest)     ▼
  │ systemctl start|stop dd-game@<id>           ┌─────────────────────────────┐
  │   (polkit lets dd-games do only that)       │ guest VM (NixOS, own kernel)│
  ▼                                             │ game.service → game-run.py  │
dd-game@<id>.service ── dd-game-run ──────────► │ /instance  = 9p share of    │
  memory, cores, ports from the record          │   instances/<id> on the box │
                                                └─────────────────────────────┘
  │ /run/dd-games/access.json (every 5 s)
  ▼
games-access (timer, 15 s) ── POST /internal/games/access ──► gate: who may play, their devices
  └─► iptables/ip6tables -A dd-games -s <device> --dport <port> -j nixos-fw-accept
```

- The page is a vhost, `games.<domain>`, behind the gate like every other
  ([games.nix](../../nix/modules/games/games.nix), the
  `services.nginx.virtualHosts` block). nginx sets `X-DD-User` and
  `X-DD-Role` from the verifier's answer and clears `Authorization`.
- The manager listens on a unix socket, not a port. It believes the
  `x-dd-user` header, so only nginx (in the `dd-games` group) may open it.
  The VM test checks that `127.0.0.1:4182` answers nothing
  ([games.py](../../nix/tests/games.py)). `DD_GAMES_BIND` can still be a
  TCP address, for tests.
- Each server is a systemd instance, `dd-game@<id>`, of one template. The
  template boots the same guest for every game; the record says which game.

## The catalogue

The catalogue is [`data/games/catalogue.json`](../../data/games/catalogue.json):
111 games today, 87 with covers, 30 that run under Wine. It is made from the
pelican eggs (the Pterodactyl/Pelican panel's game definitions,
[pelican-eggs/games-steamcmd](https://github.com/pelican-eggs/games-steamcmd))
by two scripts, run by hand, with the result committed. The catalogue's
`eggs` says which checkout it was made from: the repository and the commit
(`e17e2c3d`, 2026-08-24, today). resolve.py records it, and refuses a
checkout with changes in it.

```
git clone https://github.com/pelican-eggs/games-steamcmd eggs
git -C eggs checkout <commit>
data/games/resolve.py eggs data/games/catalogue.json data/games/covers/
data/games/sizes.py   data/games/catalogue.json
```

[`resolve.py`](../../data/games/resolve.py) reads every `egg-*.json` and
keeps those with a Steam app id (`SRCDS_APPID`, `APP_ID`, `APPID` or
`STEAM_APPID`). For each:

- **name and id**: the egg's name without "server", "dedicated server",
  "(wine)", "(proton)"; the id is that, lower-cased and dashed.
- **wine**: true when the egg's docker image says wine or proton.
- **game and cover**: the game's own store id, found with Steam's store
  search, and its `library_600x900.jpg` saved as `covers/<id>.jpg`. A name
  Steam did not know is not asked again; delete the entry to retry.
- **settings**: every egg variable that is not a port. `OURS` maps common
  names (`SERVER_NAME`, `PASSWORD`, `MAX_PLAYERS`, `WORLD`, ...) to our four
  (name, password, players, world), shown first on the card. `HIDE` keeps
  steam logins, branch ids and the like out of sight; they keep their
  defaults. `AUTO_UPDATE` becomes "Update on start", off by default. A
  variable with `PASSWORD` or `PASSWD` in it is kind `password`; an
  `in:a,b` rule becomes a choice list.
- **ports**: every variable matching `PORT$|^PORT_|_PORT_`, plus
  `SERVER_PORT` always. Their egg defaults are kept as `port_defaults`, but
  only as a note: the manager hands ports from its pool.
- **fixed_ports**: from `FIXED_PORTS`, curated by hand, for games that tell
  their clients a port of their own and ignore the one they are given.
  Today only Satisfactory (`SERVER_PORT` 7777, `RELIABLE_PORT` 8888).
- **implicit_ports**: from `IMPLICIT_PORTS`, by id, curated by hand: ports
  a game opens by itself at an offset from `SERVER_PORT`, with no egg
  variable to name them. Today the three Valheims, whose Steam query is
  `SERVER_PORT` + 1 (`QUERY_PORT`). Only where the game is known to do it.
- **memory_mib**: what the guest gets, from `memory()`: 6144, or 8192 for a
  Wine server, unless `MEMORY` names the game.
- **recipe**: the egg's `startup` line, its ready line (`config.startup.done`),
  its `stop` command, its `config.files` rewrites and its install script,
  as they are.
- One entry per id: a Linux egg replaces a Wine one; otherwise the first
  found (in path order) stays.

[`sizes.py`](../../data/games/sizes.py) adds `disk_bytes`: the public
branch's manifest sizes of the server's depots from `api.steamcmd.net`, the
Linux depots, or the Windows ones for a Wine server. 107 of 111 have it.
The largest is Counter-Strike 2 at about 70 GiB.

What a game takes:

```
memory      the catalogue's memory_mib                 Game::memory (lib.rs), needs.nix
cores       4, capped at the box's threads           Game::cores, Manager::create
disk        disk_bytes × 1.5 + 1 GiB, at least 2G,   needs.nix (placement only)
            15G when Steam does not say
```

Nothing in an egg says how much memory a server needs, so resolve.py gives
each game its class, and the manager and the placement both read that one
figure. [`games-measure.sh`](../../nix/modules/games/games-measure.sh)
runs every five minutes (`dd-games-measure.timer`) and keeps, per game, the
highest `MemoryPeak` systemd has counted for any running `dd-game@` unit
(guest and qemu together), in `/var/lib/dd-games/measured.json`. Nothing
sizes from it: a guest's kernel fills whatever memory it is given with its
own cache, and the host's cache of the share's files is charged to the same
unit, so a peak says what a world was given more than what its game needs.

## A world's life

A member opens a game (`/game/<id>`), fills the card and presses Start. From
there:

```
create ──► records/<id>.json written, instances/<id>/instance.json copied, unit started
  │        guest: install (once) → put back a kept world → rewrite config files → run
  ▼
starting / downloading the game / running ── stop ──► stopped ── start ──► ...
                                                         │
                                         ┌───────────────┴───────────────┐
                                         ▼                               ▼
                               keep world, delete server       delete server and world
                               worlds/<owner>/<game>-<time>/   instances/<id>/ gone
                                         │
                                         ▼
                               start a server with it (a new create, world copied in)
```

**Create** ([`Manager::create`](../../servers/games/src/lib.rs)), under one
lock, refuses when:

- the member already keeps `kept_per_member` servers (5,
  `dd.games.keptPerMember`; stopped ones count, they hold ports and disk);
- the member already has `per_member` up (1, `dd.games.perMember`);
- the box's memory budget has no room (below);
- a fixed port is taken ("one of this game per box"), or the pool is empty.
  A game's implicit ports go with its `SERVER_PORT`: a pooled one is a port
  whose next ones (at the game's offsets) are free in the pool too.

Then it builds the server's environment: the egg's defaults, then what the
member set on visible settings (checked against choices, numbers, and no
control characters), then the ports. A password setting whose catalogue
default is non-empty is replaced with 20 random characters unless the member
gave their own; the egg is public, so its default is too. An empty default
stays empty: for most games that means no password. The id is the game's id
with its dashes dropped and a number after it, cut to fit 24 characters
(`valheim1`). The record is saved, `desired` is `running`, and the unit is
started with `--no-block`.

**Install and run** happen in the guest
([`game-run.py`](../../nix/modules/games/game-run.py), under `steam-run`):

1. Install: skipped when `server/.installed` exists and "Update on start" is
   off. Otherwise it writes `updating` to `status` and runs the egg's own
   install script with `chown`, `apt`, `apt-get` stubbed and `sudo` a
   pass-through. The eggs' `/mnt/server` and `/home/container` are links to
   `/instance/server`, so their paths need no rewriting. A failed install
   is a try that came to nothing (below).
2. Kept world: if `/instance/world` exists, its files are copied over the
   install, and it is removed.
3. Config files: the egg's `config.files` find/replace, for ini, json,
   properties, yaml, xml and plain files. A failure is logged, not fatal.
4. Run: the startup line with each `{{VAR}}` turned into `$VAR`, so a
   member's server name never becomes shell. Wine servers get Xvfb on `:99`
   and a prefix in `server/.wine`; `proton run` is a shim for `wine`. The
   runner writes the install's and the game's output to `/instance/game.log`;
   past 8 MiB it becomes `game.log.1`, replacing the one before, since the
   share is backed up. `status` becomes `running` when the egg's ready line
   is printed or when `ss` sees `SERVER_PORT` listening, whichever is first
   (some eggs' ready lines are stale).
5. Stop: SIGTERM to the guest service sends the egg's stop command down the
   game's stdin (or SIGINT for `^C`), waits 60 s, then terminates. However
   the game exits then, `status` is `stopped`.

**Tries that come to nothing**: an install that fails, a game that exits on
its own, or the runner itself failing. Each is counted in
`/instance/tries`, and the guest's unit starts the runner again in 15 s,
with `status` saying `retrying: ...` (the page: starting). At the third the
runner writes `failed: <why>, 3 times` and exits cleanly, so nothing starts
it again; the page shows "failed" and why, and offers Stop. The count goes
back to nothing when the game is up, and `dd-game-run` removes it at every
start of the machine, so Stop then Start is three more tries.

**State** shown on the page ([`Manager::state`](../../servers/games/src/lib.rs))
comes from the unit and from `status`:

```
unit failed                               failed
unit not active, desired running          starting   (between states)
unit not active, desired stopped          stopped
unit active, status "running"             running
unit active, status "updating"            downloading the game
unit active, status "failed: ...",        failed     (the reason on the owner's page)
  desired running
unit active, anything else                starting
```

Each has one of the shared state words (`State::tone`, docs/dev/ui.md):
running ok, starting and downloading working, failed broken, stopped off.

**Stop** sets `desired` to `stopped` and stops the unit. The template's
`ExecStop` ([`dd-game-stop`](../../nix/modules/games/games.nix)) sends
`system_powerdown` over the machine's QMP socket, gives the guest 60 s to
shut the game down, then sends `quit`, then waits 15 more; systemd's
`TimeoutStopSec` is 90.

**Configure** changes the visible settings; a running server is stopped and
started for them.

**Keep world, delete server** ([`keep_world`](../../servers/games/src/lib.rs)):
the server must be stopped, and the member may keep `worlds_per_member`
worlds (5, `dd.games.worldsPerMember`). Every regular file under `server/` modified
after `.installed`, except `steamcmd/` and `.steam/`, is copied to
`worlds/<owner>/<game>-<unix time>/`, with a `world.json` holding the
settings. Then the instance directory and record go. Symlinks are never
followed: the tree is the guest's.

**Start a server with it** ([`restore_world`](../../servers/games/src/lib.rs))
is a create with the world's settings, with the world's files copied to the
new instance's `world/` after its record is written and before its unit
starts, so the guest always finds them; it puts them over the install
before it runs. The kept world is removed once handed over.

**Delete** needs the server stopped, and removes its directory, record and
players list.

**After a reboot**, or the manager's own restart,
[`Manager::restore`](../../servers/games/src/lib.rs) starts every server
whose record says `running` and is not up. A deploy does not touch them:
the template has `restartIfChanged = false`, `stopIfChanged = false` and
`X-StopOnRemoval = false`, so a newer guest takes effect when that server
next restarts.

### Where it lives

```
/var/lib/dd-games/                       node1: dataset tank/enc/games
  records/<id>.json          the host's record: owner, memory, cores, ports, env, recipe, desired
  players/<id>.json          who the owner invited
  control/<id>.qmp           the machine's QMP socket
  instances/<id>/            9p-shared with the guest as /instance
    instance.json            the guest's copy of the record (it can write this; nothing reads it back)
    status                   starting | updating | running | stopped | retrying: ... | failed: ...
    tries                    tries that came to nothing since the game was last up
    game.log, game.log.1     the install's and the game's output, 8 MiB each at most
    server/                  the install and the world (/mnt/server, /home/container in the guest)
    world/                   a kept world waiting to be put back
    disk.qcow2               the guest's own disk, 64G sparse
  worlds/<owner>/<name>/     kept worlds, with world.json
  measured.json              each game's memory peak
/run/dd-games/dd-games.sock  the manager's socket
/run/dd-games/access.json    what the firewall reads
```

Backups ([roles/game.nix](../../nix/roles/game.nix)) take `instances`,
`records`, `players` and `worlds`, without `disk.qcow2`. The game files and the saves are ordinary
files on the box because the install goes to the share, not the guest's
disk.

The record lives outside the share on purpose. The guest can write
everything under `instances/<id>/`, so the manager reads the owner, memory
and ports only from `records/`, and every read of a guest-written file
(`status`, `game.log`) is capped, refuses symlinks, and takes only a
regular file (`tail_of`). The VM test rewrites the owner in the shared copy
and checks nothing changes. An instance directory with no record (a backup
from before records were in it) is never made a server from its
`instance.json`: the manager names it in its log at start and leaves it.

## One VM per world

Each server is its own qemu guest
([game-guest.nix](../../nix/modules/games/game-guest.nix)): a small NixOS,
the same for every game and every server. A dedicated server is a big
closed binary, often modded; it gets a kernel, a disk and a network of its
own and nothing of the box. The host's side is kept narrow too: qemu runs as
`dd-games` with `NoNewPrivileges`, `ProtectSystem=strict`, write access only
to `instances/` and `control/`, and `/dev/kvm`.

[`dd-game-run`](../../nix/modules/games/games.nix) reads the record (never
the guest's copy) and starts the guest with:

```
-m <memory> -smp <cores>                    from the record
-qmp unix:control/<id>.qmp                  for a clean stop, outside the share
hostfwd=udp::<p>-:<p>,hostfwd=tcp::<p>-:<p> every port, both ways, same number inside
NIX_DISK_IMAGE=instances/<id>/disk.qcow2
-machine accel=tcg -cpu max                 only when dd.games.kvm = false
```

The guest:

- has the instance directory at `/instance` over 9p with
  `securityModel = "none"`. Its `game` user has uid and gid 951, the same as
  the box's `dd-games`, so files need no mapping.
- has no firewall of its own: qemu forwards only the server's ports.
- reaches the internet through qemu's user network (Steam's CDN, GitHub,
  whatever an install script fetches).
- carries the runtimes the eggs reach for: .NET 8 and 9, Mono, a headless
  JRE, Python, and Wine (`wineWowPackages.stable`, winetricks, Xvfb, a
  `proton` shim). `steamcmd` and `steam-run` come from a package set of the
  guest's own that allows exactly those unfree packages, so the box's
  package set is unchanged.

## Ports and the firewall

A server's ports are open only to the devices of its owner and the people
they invited, and only on the Commonty network (`commonty0`), never the
LAN or the owner's tailnet.

**Pool and fixed ports.** Each port variable of a game gets the next free
port in `portBase .. portBase + portCount - 1` (27000-27199 by default),
unless the catalogue fixes it. A fixed port is that number or nothing, so
there can be one of that game per box. Ports belong to a server while it
exists, running or not. The guest listens on the same number the host does,
because some games tell clients their own port.

The same set (the pool plus every fixed port in the catalogue, and the
implicit ports beside a fixed `SERVER_PORT`) is worked out once, in
games.nix (`dd.games.fixedPorts`, and `dd.games.reach` as headscale writes
ports), and used in three places:

- [`ports.nix`](../../nix/modules/box/ports.nix) reserves them, so no
  service of ours can take one;
- [`headscale.nix`](../../nix/modules/net/headscale.nix) lets every member's
  device reach them on any box tagged `tag:box` (the flake's
  `fleetGamePorts` is every `game` box's `dd.games.reach`);
- the box's firewall narrows that to the invited devices.

A pooled server's implicit ports are in the pool, so they need nothing more
here; the manager holds them in the record with the rest, so qemu forwards
them and the firewall opens them.

**The chain.** [games.nix](../../nix/modules/games/games.nix) adds a chain
`dd-games` (iptables and ip6tables) and puts a jump to it first in
`nixos-fw` for everything arriving on `commonty0`. A packet no rule in it
takes falls back to the rest of the firewall. Nothing else opens these
ports.

**Filling it.** Two loops:

```
manager, every 5 s and on start/stop/invite
  writes /run/dd-games/access.json:
  {"servers":[{"id","owner","players":[...],"ports":[...]}]}   only servers meant to be up

games-access.service, every 15 s (and after every firewall reload)
  POST [{owner, players}] to the gate's /internal/games/access
  gate answers, per server: the owner if a member; each player still the owner's
    friend and a member or guest; and their devices' network addresses
  one rule per (address, port), tcp and udp:  -s <addr> --dport <port> -j nixos-fw-accept
  rebuilt only when the list changed; emptied when the manager or the gate cannot say
```

[games-access.sh](../../nix/modules/games/games-access.sh) checks every
address and port is only that before it goes into a rule. A friendship
ended or a guest removed closes the door within one round.

**The address players type** is the box's address on the Commonty network,
from `/run/commonty-net-address` (written by the join keeper), falling back
to `dd.box.net`. The server page shows it with `SERVER_PORT` (or the first
port), and lists any other ports.

## Memory and disk

`dd.games.memoryMiB` is the memory every running guest and its qemu may
have together. It defaults to a third of the box's RAM, from the hardware
report (`dd.box.memoryBytes`), and never less than 4096; a box's
`box.json` can set it as `gamesMemoryMiB`. On node1 (62.5 GiB)
that is about 21300 MiB, the "budget is 20G" in
[fleet/roles.json](../../fleet/roles.json).

```
each running server counts   memory + 512 (QEMU_OVERHEAD_MIB)
a new one fits when          sum(running) + its memory + 512 <= budget
its memory                   min(game's memory_mib, budget - 512)
```

Stopped servers count nothing toward memory, but count toward the 5 a
member may keep. Cores are 4, capped at the box's thread count.

Disk is not budgeted at run time. Placement uses
[needs.nix](../../nix/modules/games/needs.nix): the base asks for x86_64,
KVM, 100G of disk, internet out, players in over the network, and nginx,
the gate and the Commonty network on the same box. Each catalogue game is a
variant with its own memory, threads and disk (above). The role's own entry
in [fleet/roles.json](../../fleet/roles.json) asks for KVM and 24 GiB.

## Someone who left

The manager asks its gate who left every minute (`/internal/leaving`):
their servers stop at once, and 90 days later every server and kept world
of theirs is deleted (`Manager::forget`, [leaving.md](leaving.md)).

## Friends and guests

Who may play is the gate's, not the manager's
([servers/gate/src/friends.rs](../../servers/gate/src/friends.rs)).

- A member makes a friend link on Friends (`/_dd/friends`) and sends it.
  Whoever opens it while signed in becomes their friend, both ways.
  Someone who signs up to open one is a **guest** of the member who made it.
- The gate lets a guest reach `games.<domain>` and nothing else
  (`guest_host`, in `verify`); the home page shows them the Games tile alone.
- In the manager, anyone the gate does not call `member` or `demo` is a
  guest (`guest()` in [main.rs](../../servers/games/src/main.rs)). A
  guest's `/` is a redirect to `/servers`, and they cannot open a game, create,
  restore a world or invite (403).
- On a server's page the owner ticks who plays. The names are checked
  against the gate's list of the owner's friends, asked at that moment
  (`GET /internal/games/friends/<owner>`: members and guests only); the
  owner and anyone not on the list are dropped. If the gate does not
  answer, nobody is invited.
- An invited person sees the server under "Invited to", with its address
  and its state, which their page follows while it starts, but not its
  settings, its controls, why it failed, or its log; the log is the owner's
  (a game writes passwords and what players type into it). Anyone else
  gets 404 for it.
- The **demo** can look at the library and the card, not start: the role
  sets `demo = "read"` on the tile, so the gate refuses the demo's POSTs,
  and the card shows no Start button for the user `demo`.

## The manager's routes

All behind the gate; the user is `x-dd-user`, no user is 401.

```
GET  /                         the library (guest: redirect to /servers); ?q= filters, ?n= a notice
GET  /game/{id}                the library with one game's card open
GET  /servers                  servers you host, servers you're invited to, your kept worlds
GET  /server/{id}              one server: state, address, settings, players, log (yours or invited)
GET  /server/{id}/state        {state, label, tone} for the page to poll; the owner also
                               gets log and reason (pages::state_json)
GET  /cover/{id}               the cover jpg, cached a week
GET  /static/{games.css,games.js}
GET  /health                   "ok"
POST /create/{game}            a new server from the card's form
POST /start/{id}  /stop/{id}  /configure/{id}  /delete/{id}  /keep/{id}
POST /players/{id}             p:<name>=on for each invited friend
POST /worlds/{name}/start      a new server with a kept world
POST /worlds/{name}/delete
```

A POST answers with a redirect back to its page; an error rides along as
`?n=<message>` and is shown as a note. Every action on a server goes
through `owned`, so only its owner can change it.

The pages are askama templates in
[servers/games/templates/](../../servers/games/templates/), filled in
[pages.rs](../../servers/games/src/pages.rs), with the look in the shared
`/_dd/static/home.css` (its tokens, its state words, the bar) and what
[web/games.css](../../servers/games/web/games.css) adds to it, and
[web/games.js](../../servers/games/web/games.js) built into the binary. They
work without the script; it filters the library as you type, closes the
card on Escape, copies the address, asks before a delete (`data-confirm`,
no inline script), and polls `/state` every 3 s while a server is starting
or downloading. The bar is the shared one (home.css brings `bar.css`;
`shell.js`). Not yet as [ui.md](ui.md) has it: the pages are server-drawn
templates, not the kit's cache-first pages; games.js polls with its own
fetch, not the kit's `load`; covers and cards keep a 1px ring
(`--shadow`), the ribbon its drop shadow, and a cover-less game its
gradient. A few games (Satisfactory, Valheim, Palworld) have their own
"how to join" line (`how_to_join`).

## Configuration

Set by [games.nix](../../nix/modules/games/games.nix):

```
dd.games.enable       on in roles/game.nix
dd.games.catalogue    data/games/catalogue.json
dd.games.covers       data/games/covers
dd.games.perMember    1      servers one member may have up at once
dd.games.keptPerMember   5   servers one member may keep, stopped ones too
dd.games.worldsPerMember 5   worlds one member may keep apart from a server
dd.games.memoryMiB    max(4096, RAM/3), or box.json's gamesMemoryMiB
dd.games.portBase     27000
dd.games.portCount    200
dd.games.kvm          true   false only where nested KVM cannot boot a guest
dd.games.manager      true   false runs the template alone
```

The manager's environment:

```
DD_GAMES_DIR               /var/lib/dd-games
DD_GAMES_CATALOGUE         the catalogue           DD_GAMES_COVERS     the covers
DD_GAMES_BIND              /run/dd-games/dd-games.sock (default 127.0.0.1:4182)
DD_GAMES_PORT_BASE/COUNT   the pool                DD_GAMES_PER_MEMBER perMember
DD_GAMES_KEPT_PER_MEMBER   keptPerMember           DD_GAMES_WORLDS_PER_MEMBER worldsPerMember
DD_GAMES_MEMORY_MIB        memoryMiB
DD_GAMES_ADDRESS           dd.box.net              DD_GAMES_ADDRESS_FILE /run/commonty-net-address
DD_GAMES_ACCESS            /run/dd-games/access.json
DD_GAMES_GATE              http://127.0.0.1:<verify port>
DD_GAMES_HOME              https://home.<domain>/
```

The manager runs as `dd-games` (uid/gid 951, in `kvm`). A polkit rule lets
that user `start` and `stop` units matching `dd-game@[a-z0-9]+.service`
and nothing else. It waits for `zfs-datasets.service`, since on node1 the
directory is a dataset that arrives owned by root.

## The VM test

[nix/tests/games.nix](../../nix/tests/games.nix) and
[games.py](../../nix/tests/games.py) run a box with the module and a
catalogue of one stand-in game, `probe`: its install makes a directory and
its startup is `python3 -m http.server` on `SERVER_PORT`. Nothing comes
from Steam. The box has 4 GiB, a budget of 2048 MiB (so the guest gets
1536) and its pool at 27015. The guest is a VM inside the test VM, so the
test needs a builder with nested KVM. It is one of the `vm_tests` jobs in CI.

It checks, in order:

- no TCP port; 401 without a user; the library and the card for a member;
  the demo sees the card without Start; the static files are served;
- a member creates a server, it comes up and its address is on `/servers`;
  a second one is refused;
- rewriting the owner in the guest's copy of the record changes nothing;
- a file written in the world is served by the game;
- a `switch-to-configuration test` leaves the game's process alone;
- another member cannot stop it, read its state or log, or see it at all;
- a guest is sent to `/servers` and cannot create; no role at all counts
  as a guest;
- an invite of someone who is not a friend invites nobody, and
  `access.json` names only the owner;
- after a reboot the server is back with its world;
- a stop is a clean shutdown (`Result=success`), and the world can be kept,
  restored into a new server, and finally deleted.

The manager's own logic has unit tests in
[lib.rs](../../servers/games/src/lib.rs): ownership and invites, the
per-member limits, fixed against pooled ports, the memory budget and the
pool running out, restore after a reboot, settings and restart, keeping
and restoring a world, and the capped, no-symlink reads.

## Changing it

**Refreshing or adding games.** Games come from the eggs, so a new game is
a new egg. Check out the pelican eggs at the commit you mean (the
catalogue's `eggs.commit`, or newer), run `resolve.py` with the checkout,
the catalogue and the covers directory, then `sizes.py`, and commit the
catalogue and any new covers; the catalogue then names that commit. `resolve.py` keeps the store ids it found
before. Nothing else needs editing: `needs.nix`, the port reservations and
the headscale rules all read the catalogue. Then try the game on a box: a
recipe the guest cannot run shows as failed, or stuck in starting, with the
reason in the log.

**A game that must keep its own port.** Add it to `FIXED_PORTS` in
`resolve.py` (by lower-cased name, variable to port), rerun it, and commit.
The flake's port lists pick it up. Keep it to games that really tell
clients their own port; every fixed port is one of that game per box.

**A runtime a game needs.** Add the package to `environment.systemPackages`
in [game-guest.nix](../../nix/modules/games/game-guest.nix). If it is
unfree, add it to `allowUnfreePredicate` in `guestPkgs` in games.nix. A
running server picks up the new guest when it next restarts.

**What an egg does that the guest does not.** Install scripts run as they
are, with `apt`, `chown` and `sudo` stubbed. A new `config.files` parser, a
different stop, or something an egg's startup expects goes in
[game-run.py](../../nix/modules/games/game-run.py).

**Memory per game.** `memory()` and `MEMORY` in resolve.py; rerun it.
Both the manager and needs.nix read the catalogue's `memory_mib`.

**A port a game opens by itself.** Add it to `IMPLICIT_PORTS` in
`resolve.py` (by id, variable to offset from `SERVER_PORT`) only where the
game is known to do it, rerun it, and commit.

**Ports or the firewall.** The pool is `dd.games.portBase` and `portCount`;
the flake and headscale read them from each game box's configuration. The
chain's shape is in games.nix's `extraCommands`; its rules are written only
by games-access.sh. A rule on any other interface, or one not limited to
the gate's addresses, would open a world to people the owner did not
invite.

**Run the VM test** (`games`) for any change to the module, the guest, the
runner or the manager.
