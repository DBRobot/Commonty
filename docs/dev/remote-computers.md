# Remote computers

Status: the records, placement, the lease and the box's side are built; a
computer runs on a box and stops. Snapshots and moves with their disks
are not built yet (below, Open).

1. **The records**: configs and computers as signed items, judged in
   `state/computers` beside identity, served at `4195/computers`. Below,
   under The records.
2. **Placement** (the control plane's): each computer is a placed service
   with a lease, fitted to the boxes' `computers.prom`, never moved while
   its box is alive.
3. **The box's side**: dd-control holds each computer's lease where the
   plan puts it and starts `dd-computer@<id>`; the guest, the page,
   `dd computer`, the outbound rules and the allowance.
4. **Snapshots**, so a computer can come back on another box. Built: a
   nightly backup of each computer's `/home` disk (Backups, below). Not
   built: the 15-minute repository, and coming back by itself.

A box hosts computers when its own report says it has KVM on x86_64 and
its owner does not bar them: it is then built with the `compute` bundle
(nix/lib/fleet-placement.nix, nix/roles/bundles.json) and the release's
table gives it `computers` (nix/lib/placement-inputs.nix). No list says
which boxes; node1 hosts them by its capability.

A member keeps a computer of their own on the fleet: a VM with its own
disk, reached over the Commonty network, that reaches the web. They make
it from a config they save (memory, threads, disk, packages, who may reach
it, where it may run), from the browser or the CLI. It runs until they stop
it, keeps their files, and moves to another box only if its box fails and
they said it may.

It is built from what game servers already do
([games](games.md)): one QEMU/KVM guest per computer, a record on the
host, a template unit, a firewall chain refilled from who may reach it.

## What a member decides

- **Memory, threads, disk**, each its own and changeable. A member's
  computers together, running or not, stay within 32G, 8 threads and 500G of disk
  (`fleet/policy.json` `computers`, for now). Disk counts toward their storage
  allowance, as game worlds do from now on (below).
- **Packages**, in a Nix file: a NixOS module for the guest
  (`environment.systemPackages`, services, users' shells). Saving it
  rebuilds the guest's system. `/home` is a disk of its own and survives
  every rebuild.
- **Who may reach it**: the member's own devices, always, on SSH, with
  the keys its record names (below); and more ports for themselves or
  named friends.
- **Where it may run**: on their own boxes only, or on anyone's. Anyone's
  says first, and has to be confirmed before the computer is made:

  > Whoever has that box in their house can read this computer's disk
  > and memory: your files, and anything you sign in to on it, such as
  > Claude. Run it on someone else's box?

  A member with no box of their own can only choose anyone's. The
  computer can't be made without the yes.

A config is a saved thing apart from the computers made from it: make
three the same, change the config, rebuild one. Every member has one
from the start, the Default config, `default` (4G, 2 threads, 32G, build tools and the
usual languages). Saving a config called `default` replaces it for
that member.

**SSH keys are made on the member's devices.** Only the public half goes
into the signed config or computer (`keys`, OpenSSH public key lines, at
most 8); the guest's sshd lets in exactly those. `dd computer make`
makes a key pair for the computer on the device it runs on
(`~/.config/commonty/computers/<name>.device-key`, 0600 in a 0700
directory, with a comment only this flow writes; a file there it did not
make, or not the user's own 0600, is refused, and nothing in `~/.ssh` is
ever taken) and puts its public line in the item;
the page takes public keys pasted from a device's `.pub` file. A key
saved later reaches a running computer within a minute, and one left
out stops working. No private key is ever on a box.

## How it fits together

```
browser: compute.<domain>        CLI: dd computer ...
          │                           │
          ▼                           ▼
     gate (who they are) ──► the computers API ──► control plane: the agreed state
                                                    computer records, configs,
                                                    owner, consent, port
                                                          │
                                place.Decide: which box, from each box's
                                computers.prom facts and the owner's consent
                                                          │
                                                          ▼
     the box the plan names ── lease svc/computer-<id> ── dd-computer@<id>.service
       (dd-control: flag /run/dd-placed/computer-<id>)    after dd-computer-prepare@<id>
                                                   qemu: system.qcow2, home.qcow2
                                                   in /var/lib/private/dd-computer/<id>
                                                          │
                                   every 15 min: ZFS snapshot → restic → the
                                   computer's bucket → copies on other boxes
```

## The computer

- **One service per computer**, `computer-<id>`, with a lease of its own,
  `svc/computer-<id>`, its own fence and its own move. The unit is
  `dd-computer@<id>`; dd-control maps the one to the other (placed.go
  `units`, and box/placed-fence.sh).
- **Its record is agreed state**, not a file on one box: a box that dies
  can't take the only copy of what the computer is with it. It holds the
  owner, the config, the consent, its port, and whether it should be up.
  Nothing secret.
- **A user of its own**: its prepare, its QEMU and its share run as
  `dd-computer-<id>` (systemd's `DynamicUser=`), which alone owns its
  directory (`StateDirectory=dd-computer/<id>`, 0700, under
  `/var/lib/private`, which no other user enters) and its QMP and console
  sockets (`/run/dd-computer/<id>`). A guest that broke out into its QEMU
  holds its own disks and key and nothing of another computer's. On disk
  the directories are 0700 and owned by `nobody` (65534): systemd maps
  each into its own computer's units only. Nothing on a box may run as
  `nobody`. Root's
  scripts never follow a link in those directories (`find -P`, `du -P`,
  `dd iflag=nofollow`), and the network key goes in as that user.
- **Two disks**, in its directory on the box that runs it and nowhere
  else: `system.qcow2`, from the release's image, and
  `home.qcow2`, which nothing but deleting the computer removes. Both are
  grown to the record's disk size, never shrunk.
- **The system disk follows the release**: `system.base` beside it names
  the image it was made from, and a start (`dd-computer-run`) whose
  release has another image makes the system disk again from the new one.
  A switch never restarts a running computer, so it takes the new base at
  its next start: a stop and start, or its box's reboot. Everything
  outside `/home` goes back to the image (the owner's Nix file is applied
  again at that first boot, `dd-apply`); `/home` is untouched. The SSH
  host keys live on `/home` (`/home/.commonty`), so the owner's
  `known_hosts` still matches. A computer made before `system.base` was
  kept has none, and is made again at its next start. (A ZFS dataset of its
  own, `tank/enc/computers/<id>`, with the disk size as its quota, comes
  with the snapshots.)
- **The guest** is the same NixOS base for every computer, built by the
  release. The member's Nix file reaches it on a read-only config disk and
  the guest builds its own system from it (`nixos-rebuild`, nixpkgs pinned
  by the release). The member's Nix is never evaluated or built on the
  host.
- **The image is the same bytes on every box**, as everything a box runs
  must be (dd-attest, [releases.md](releases.md)). make-disk-image's own is
  not: a kernel writes its file system (a random directory hash seed and
  inode generations, the VM's times), nix numbers its paths in no fixed
  order and at the build's time, perl's hash order sets `/etc/shadow`, and
  parted picks the MBR's id. So `computers.nix` keeps its tree and its
  boot sectors and makes the file system again with `mkfs.ext4 -d`
  (sorted, a fixed UUID and hash seed, every time clamped to 1980), with
  nix's paths renumbered by path and the disk id fixed. No machine-id or
  host key is in it: the guest makes those on its first boot.
  `nix-store --realise --check` on its derivation rebuilds and compares.
- **Stop and start** as games do: QMP `system_powerdown`, a minute, then
  `quit`. A switch never restarts a running computer.

## Network

- **In: the Commonty network only.** Each computer has a port from a
  fleet-wide pool, kept in its record so it stays the same when it moves,
  forwarded to the guest's SSH, and more forwarded ports as its config
  says. A name, `computer-<name>.<owner>.<domain>`, follows the lease like every
  other service's (the DNS Worker). No public name, no tunnel.
- **Who may reach it**: a `dd-computers` chain beside `dd-games`, refilled
  every 15 seconds (games-access.sh, which empties the chain on any answer
  that does not read) from what the box's page server writes
  of the computers running here (`/run/dd-computers/access.json`: owner,
  players, ports) and the gate's answer of their devices' addresses.
- **Out: the web.** The guest's traffic leaves through QEMU's user network,
  so on the host it is the `dd-computers` user's own sockets, and the host's
  firewall filters it by owner (`-m owner --gid-owner dd-guests`):
  - refused: port 25; the house's networks (RFC 1918, link-local, CGNAT
    and so the Commonty network, and the house's public IPv6 networks as
    the box's own addresses on them say when its firewall starts); the host's loopback, which QEMU's user
    network otherwise hands the guest at `10.0.2.2`, and every other
    address of the host's own (a public IPv6 one reaches its services
    through lo, which its firewall lets in): `-m addrtype --dst-type LOCAL`;
    DNS alone goes to the host's resolvers;
  - everything else allowed: the web, `api.anthropic.com`, package
    mirrors, the fleet's public names. (A limit per computer, tc on its
    cgroup, so one can't take a house's line, is not built.)
- **On the Commonty network itself**, for a computer kept to its owner's
  own boxes: as `dd-computer-prepare@<id>` finishes, the box that holds
  its lease asks the gate for its key (`POST /_dd/network/computer`,
  `{box, id, time, sig}`, signed by the box's TPM, as a box asks for its
  own), and the answer, sealed to that box's secrets key, goes into the
  guest's share alone (`net.json`), never to a device. The gate gives one
  only from one fresh agreed state (dd-control's `/computer-lease`: the
  computer and its lease at one height): the computer agreed up and kept
  to its owner's boxes, the asking box holding `svc/computer-<id>`
  unexpired, and that box its owner's by the release (box.json), as
  placement has it; once a minute per box and computer. The key is single use and lives ten minutes; it is tagged
  `tag:computer`, made in the boxes' user, and bound in the gate to the
  computer and its owner by the key's id (`computer-nodes.json`), never by
  a name the guest gives itself. A computer has one node: the one it had
  comes off before a new key. The guest joins through the front door with
  the release's netbridge, as a box does. Its node key is kept in memory
  only (tailscaled `--state=mem:`), never on its disk, so it never travels
  with the disk; a new key at each start, and the key before that, if
  unused, is expired, so a computer has one way onto the network at a
  time.
  - **What it reaches**: a tagged node is no one's device, so none of the
    owner's box-admin ports (22, 53, 80, 3443), with the one exception below; the policy gives
    `tag:computer` what a member's device reaches of the boxes (443, the
    game and computer ports), and the gate counts its address as its
    owner's, so the owner's game servers and other computers let it in as
    they do their devices. Git goes over https (443); git over ssh is the
    box's sshd on 22, an admin port, and stays shut to it, whoever owns
    the computer, unless it is the exception below. To push from a computer, its owner makes a forge API token
    (Git, Settings, API tokens, `write:repository`) and gives the
    computer that: `git push https://<owner>:<token>@git.<domain>/...`
    (or a credential helper). The gate does not know the token and lets
    the request through as nobody; the forge takes the token as the
    owner, and nothing more than the token's scopes. An SSH key on the
    owner's forge account does nothing from a computer.
  - **The exception, an admin computer**: `fleet/policy.json`
    `computers.admin` lists computers by id, today the owner's `claude`
    (`311fda8dea2d9a20`), which the owner administers the boxes and
    releases from. The gate tags that computer's key `tag:computer` and
    `tag:admin-computer`, and only when its agreed owner is the fleet's
    admin (`policy.owner`); another's computer by a listed id gets no key
    at all. The policy gives that tag the admin's ports (22, 53, 80, 443,
    3443), and each box adds whichever node carries it to its `dd-owner`
    firewall set (`owner-devices.sh`), by the tag, so no address is written
    down and a rejoin's new one follows. Taken off the list, its node
    comes off at the next reap, and it rejoins with its plain tag at its
    next start. Every other computer stays shut out.
    **What this exposes.** A computer runs as a QEMU process on its box, and
    nothing in it is hidden from that box: root on the box hosting it can
    read everything on that computer, its disks and its memory, so its SSH
    key, and any release key kept there. Whoever has root on that box can
    then log in to every box as admin, and release if a signing key is there.
    The fleet treats no box as trusted, and this is a door through that rule,
    taken knowingly for one computer on a box of the owner's own.
  - **When it goes**: the gate's reaper (every 5 minutes, and when someone
    leaves) takes off every `tag:computer` node that the same rule would
    not give a key now to the box it gave the key to (not agreed up, not
    kept to its owner's boxes, its lease lapsed, or moved to another box,
    even another of the owner's), whose owner is no longer a
    member, or whose key the gate did not make. It fails closed: an
    unknown computer (a 4xx) is a no; while dd-control cannot be read, a
    node is kept on the computer's last fresh answer for ten minutes at
    most, then taken off; with no agreed state on the gate at all, no
    computer has a node. A computer whose node was taken off rejoins at
    its next start, when its box asks for a new key.
  - **A computer on anyone's box gets the internet only, never the
    Commonty network**: that box's owner could read the key out of it and
    be the owner's device there.

## Placement and moves

- **What a box can host**: each box writes `computers.prom` to its facts, and its
  heartbeat carries it:
  - `dd_computer_max_mem_bytes`: its memory, less 2G, less 1.5 times what
    its services hold at their measured peak (`services.prom`, from
    `dd-service-memory`; the computers' own units not counted), the factor
    `dd.computers.headroom`. With no measure, or one older than 30 minutes,
    less the needs' sizes of every service it is built with instead, as
    before measures: such a box never offers more. `dd.computers.reserveMiB`
    sets a fixed reserve in place of both;
  - `dd_computer_max_threads`: its threads, less one;
  - `dd_computer_max_disk_bytes`: what `tank/enc/computers` has free under its quota.
  - `dd_computer_used_bytes{computer="<id>"}`: what each computer keeps
    here, what a move here would not carry.
  Computed on the box every minute (`dd-computer-facts`). A box with no KVM, or
  one draining or failing, writes zeros. These are the whole room;
  placement takes off what it puts there.
- **These facts are the box's own word** until boxes attest them. So
  they only ever size a box already trusted to host (below), never past
  the hardware its signed report gives the release (memory, threads, its
  largest pool) nor what its heartbeat measures (memory, cores). A box
  that lies can draw computers to itself among those boxes, or push them
  away, and nothing more.
- **Where it may run**: only boxes of the release whose table gives them
  `computers` (the needs entry the follower adds: KVM, x86_64) and whose
  owner or the policy does not bar it; never a stranger's box, and never
  a box on its own word. Among those, the ones whose facts fit it and,
  for "my boxes only", the ones whose box.json owner is the computer's
  owner. A member's enlisted boxes join this once they are placed at all
  (joining), and only those that qualify on their own key.
- **When it moves**: only when its box fails. `place.Decide`
  ([computers.go](../../servers/system/control/place/computers.go))
  keeps it on the live box that runs it however much better another box
  scores, and whatever room that box now says it has (one that no longer
  fits is in the plan's `doesNotFit`); a draining box keeps what it runs
  until it leaves and gets no new one. A new box is the one that fits
  and carries least of its disk over, then the one with most memory
  left. What it carries is what the box of the release that last ran it
  said it kept (`dd_computer_used_bytes`), less what the new box says it
  keeps; no third box's word counts. With "my boxes only" and one box, it
  waits for that box.
- **On the box**: a computer is a writer under a lease, as postgres is.
  dd-control asks for `svc/computer-<id>` where the plan puts it, the
  computer is agreed up and it fits this box's own check of the policy
  (the owner's computers here within their totals, every port in the pool
  and none twice); once the lease is its own it raises
  `/run/dd-placed/computer-<id>` with the lease's epoch and until when it
  may run, and starts `dd-computer@<id>`. It renews the lease every round.
  When the plan moves it or it is powered off, it stops the unit, waits
  for the stop, takes the flag down and gives the lease back.
- **The lease rule** (state/computerlease.go), on every validator, the
  witness too: `svc/computer-<16 hex>` is granted, and renewed, only while
  the computer is in the agreed log, up, and its owner a member, and only
  to a live box that placement may give it: listed in the release's
  `rules.computerHosts` (the boxes whose table gives them `computers` and
  whose owner does not bar them, flake.nix), saying it has KVM on x86_64,
  and for "own" the computer's owner by the release (box.json). An
  enlistment is the member's word and counts for nothing here, as in
  placement. Given back always. A computer powered off loses its lease at
  the next renewal.
- **Fenced**: a box that is cut off, or whose state is stale, sends no
  renewal and its lease lapses; the fence (placed.go `fence`, and
  `dd-placed-fence` every 2 s, which does not need dd-control) stops
  `dd-computer@<id>` before the lease could have lapsed, so no other box
  can hold it while it still runs here.
- **The guest's unit**: `dd-computer-prepare@<id>` (no `dd-guests` group,
  so it may reach loopback) reads the agreed record from dd-control,
  checks it again (the policy's bounds, the pool, the owner's totals among
  the computers held here), and writes `record.json`, the guest's share
  (`owner.json`: name and keys; `member.nix`) and a `.prepared` marker.
  `dd-computer@<id>` wants it (not requires): it runs only on a marker
  made for its own start, which it takes, so a prepare that failed fails
  the start and the unit's restart asks again. Both start only where the
  flag is. Every 30 s `dd-computers-share` starts `dd-computer-share@<id>`
  for each one running, which rewrites its share as its own user, so a key
  or Nix file saved reaches it; with dd-control down it does nothing.
- **What the guest says back** (whether its Nix file applied) goes on a
  small raw disk of its own; nothing on the box reads it, and the page
  does not show it.
- **What it loses**: a move restores the newest snapshot, so up to 15
  minutes of changes and a copy round (5 minutes). The old box's dataset
  is kept, named for when, and never deleted without the owner's yes.

## Backups

Each night ([`computers/backup.nix`](../../nix/modules/computers/backup.nix))
a ZFS snapshot of the dataset holding the computers' directories, taken
for the run and destroyed after it, gives restic each computer's
`home.qcow2` and `record.json` as of one moment, in a repository of their own
at `computers/` in the box's backup bucket, under the box's backup password.
The system disk is left out: it is the release's image and the owner's Nix
file, and churns by gigabytes an hour under a build. The computer never
notices. A store short of room is not filled for it: below the floor
([storage.md](storage.md)) the run or the copy is skipped, and it says so.

Restoring is by hand: on a box with the password, the bucket brought back
(`scripts/restore-box`), `restic -r s3:.../backups-<box>/computers restore
latest`, the file put in the computer's directory with the computer stopped.

## Snapshots (not built)

- Every 15 minutes, a ZFS snapshot of the computer's dataset, read by
  restic into the computer's bucket, `computer-<id>`, in the box's own store.
  restic's chunks mean a 50G disk with a few changes sends a few changes.
  The store's copies send them on to the boxes the plan picks.
- **The repository's key** is made by the first holder and sealed to every
  box able to run the computer, as postgres's archive key is. A box
  keeping a copy that can't run it holds ciphertext.
- Snapshots are pruned to the last day of 15-minute ones and a week of
  dailies. They count toward the allowance once, not once per copy.

## Storage allowance

Remote computers and game worlds count toward the member's allowance
(`allowanceGb`), by what their datasets hold (ZFS `used`), not what their
disks may grow to. `usage.nix` counts both on each box; the ledger adds
them up as it does the rest. A member over their allowance can't make or
grow an computer; a running one isn't stopped.

## The API, the page, the CLI

- **Reading**: `compute.<domain>/api/state`, behind the gate, served by
  each host box's `dd-computers` from dd-control: the member's configs
  and computers as agreed, where the plan puts each and its address on
  the Commonty network, and their totals against the limits.
  `computers.nix` puts `compute` in `dd.verify.hosts`, which gives the
  name its `/_dd/verify` and sign-in redirect.
- **Changing**: every change is a signed item put at the gate,
  `PUT /_dd/computers/{kind}`. The page signs with the member's passkey
  (sha256 of `commonty <kind> v2\0` and the body, as the judge checks);
  `dd computer` signs with the device's root key. An account whose root
  is a device's signs from that device only.
- **The page**, `compute.<domain>`: web/ files in the computers server,
  as the Games page is; the app opens it from the box rather than
  carrying a copy. Its passkey signing is the gap
  [app.md](app.md#keys-come-only-from-devices) names for passkey accounts (an assertion over
  bytes a box's page put in front of it), and the box-served page adds
  nothing beyond it. A page that signs with a device's key (power changes
  from the browser's device key, or a device root) must be the app's
  carried copy, never a box's.
- **The CLI**: `dd computer list`, `config <name> ...`, `forget <name>`,
  `make <config> [<name>]` (named as its config when no name is given),
  `start|stop|apply|delete|ssh <name>`.
  It asks the same question before "anyone's".
- **Tested** end to end in `nix/tests/computers.nix`: after the box's side
  with stand-ins, `dd computer config`, `make`, `list`, `ssh`, `stop` and
  `start` as a member, through nginx and the gate to the box's own
  dd-control, which judges, places and leases the computer with the
  relay's witness (`computers/fleet.py`); the guest reaches the web and
  keeps `/home` across a stop.

## The records

Configs and computers are signed items in the control plane's log,
judged by `state/computers`
([computers.go](../../servers/system/control/state/computers/computers.go)) in the same
pass as the identity judge (`state/ident`) and against the identity state
at that same point. The identity judge leaves these kinds alone. Every
box's dd-control serves the result at `127.0.0.1:4195/computers`
(`?owner=<name>` for one member's, `?owners=1` for only who owns what
and what was judged, which the gates read every second; `?since=<changed>`
for 304; a view is made again only when the log or the minute changes), the same
everywhere. No box decides anything about a computer.

| kind | signed by | body (JSON) |
|---|---|---|
| `computer-config` | the owner's root | `owner, name, memory, threads, disk, nix, ports[], players[], keys[], consent, version, gone?` |
| `computer` | the owner's root | `owner, id, name, config, memory, threads, disk, nix, ports[], players[], keys[], consent, anywhere, version` |
| `computer-power` | the owner's root or any of their current devices | `id, up, at` |
| `computer-delete` | the owner's root | `owner, id, at` |

- **In:** a page or `dd computer` puts an item at the gate,
  `PUT /_dd/computers/{kind}` ([computers.rs](../../servers/gate/src/computers.rs)).
  The gate takes it only from its owner, signed in, and only once their
  root's signature checks (or, to start or stop one, a device on their
  entry now; the owner of a start or stop comes from `/computers`, and
  until the gate has read that nothing goes through), then the same
  limits and list of items sent as the directory's PUTs, and hands it to
  dd-control as it came. A forgery never reaches the log; the judge
  decides everything else.
- Sizes: `memory` MiB, `disk` GiB, `threads` a count. `ports` are guest
  ports besides SSH (22 is always there). `consent` is `own` or `any`;
  `anywhere` (true or false) is the owner's yes to the warning: required
  for `any`, refused for `own`. `own` needs at least one box of theirs:
  one they have enlisted, or one the release gives them (`computers.owners`,
  the owners of `rules.computerHosts`). `keys` are OpenSSH public key lines,
  at most 8, each a known type whose key names that type, a comment of
  printable text at most, nothing past one line. `id` is 16 hex, made by the signer; `name` is
  `[a-z0-9][a-z0-9-]{0,31}`, one per owner. `nix` is at most 44 KiB, so a
  whole item fits the gate's 64 KiB with its body in base64. No item of
  these kinds carries a `recovery_sig`: the gate and the judge refuse one. Every field is written,
  empty lists as `[]`: a body that says more or less than its kind is
  refused.
- `version` grows strictly per config and per computer; an item sent
  again changes nothing. `computer-power`'s `at` grows strictly per
  computer and is within 5 minutes ahead / an hour behind the agreed time;
  so is `computer-delete`'s. `computer-config` with `gone` deletes a config.
- The judge refuses an item unless the owner's status is `member` (a
  delete only needs their root); sizes are within `fleet/policy.json`
  `computers` (`memory`, `threads`, `disk`); the owner's computers together,
  running or not and this one as it would be, keep within `memory`, `threads`
  and `disk`; a disk only grows; every player is a member, at most 32,
  none twice. Per member, so nobody fills the state or the pool: at most
  32 configs and 256 computers ever made (deleted ones counted), and 16
  ports across their computers, SSH counted. A deleted computer's id
  never comes back. A guest name past its time counts as no one.
- Ports: a new computer's SSH port, and each forward it adds, is the
  lowest free in the pool (`computers.portBase`, `portCount`), in agreed order,
  so every box agrees; an update keeps the ports it had; a delete frees
  them.
- The view: each computer with its `port`, `forwards`, `asked` (what its
  owner said) and `up` (asked, and its owner still a member: a member who
  leaves or is shut out has theirs stopped, kept until the leaving window,
  never deleted by the judge: the leaving window deletes them, and a delete
  item needs its owner a member, signed in at the gate), and for `own` the owner's `boxes` from the
  identity state. Configs, with the deleted ones left out. The Default
  config is not an item: the page and the CLI offer it to everyone.

## Open

- **The port pool's size** (`fleet/policy.json` `computers.portCount`,
  200 from 27200 for the whole fleet, 16 per member): David's decision,
  not made yet.
- **node1's room**: by the needs' sizes its services would keep about 48
  GiB; measured, they hold about 22 GiB at their peak (14 of it the CI
  runner), so it offers about 27 GiB and 7 threads to computers
  (2026-10-30). A 32 GiB or 8-thread computer does not fit on it.
- **Snapshots and moves** with their disks: not built. A computer whose
  box fails waits (own) or starts afresh elsewhere (any) without its disks.
- **Disks of a deleted computer** stay on the box that ran it; nothing
  deletes them yet.
- **Signing from the page with a device's key** (power changes): not
  built; the passkey signs everything there.
