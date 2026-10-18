# Remote computers

Status: being built, in this order, through the control plane:

1. **The records** (this branch, inst-1017): configs and computers as
   signed items, judged in `state/computers` beside identity, served at
   `4195/computers`. Below, under The records.
2. **Placement** (the control plane's): each computer is a placed
   service with a lease, fitted to the boxes' `vm.prom`, never moved while
   its box is alive.
3. **The follower** on each box: runs the computers whose lease the box
   holds; the guest, the page, `dd vm`, the outbound rules and the
   allowance are built (branch vms-1017) and move onto it.
4. **Snapshots**, so a computer can come back on another box.

A box carries the compute role when its own report says it has KVM; no
list of boxes names it.

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
  running computers together stay within 16G and 8 threads
  (`fleet/policy.json` `computers`, for now). Disk counts toward their storage
  allowance, as game worlds do from now on (below).
- **Packages**, in a Nix file: a NixOS module for the guest
  (`environment.systemPackages`, services, users' shells). Saving it
  rebuilds the guest's system. `/home` is a disk of its own and survives
  every rebuild.
- **Who may reach it**: the member's own devices, always, on SSH, with
  the computer's own key (below); and more ports for themselves or named
  friends.
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

Nobody types an SSH key. Each remote computer makes its own key pair when
it is made: the public half is let in, the private half is kept beside
its disks on the box (`computers/<id>/ssh/key`, out of the guest's
shares, so it moves and goes with them) and handed only to its owner:
`dd vm ssh <name>` fetches it into `~/.ssh/commonty-<name>` and uses it,
and the page offers it as a download. "New key" makes another and the
old one stops working. The box already runs the computer, so holding its
key tells it nothing new.

## How it fits together

```
browser: compute.<domain>        CLI: dd vm ...
          │                           │
          ▼                           ▼
     gate (who they are) ──► the computers API ──► control plane: the agreed state
                                                    computer records, configs,
                                                    owner, consent, port
                                                          │
                                place.Decide: which box, from each box's
                                vm.prom facts and the owner's consent
                                                          │
                                                          ▼
     the box the plan names ── lease svc/vm-<id> ── dd-computer@<id>.service
                                                   qemu: system.qcow2, home.qcow2
                                                   on tank/enc/computers/<id>
                                                          │
                                   every 15 min: ZFS snapshot → restic → the
                                   computer's bucket → copies on other boxes
```

## The computer

- **One service per computer**, `computer-<id>`, `placed` and `lease = true`,
  so each has its own lease, its own fence and its own move. The unit is
  `dd-computer@<id>` (`placed.nix` learns to map a service to a template
  computer; it refuses `@` today).
- **Its record is agreed state**, not a file on one box: a box that dies
  can't take the only copy of what the computer is with it. It holds the
  owner, the config, the consent, its port, and whether it should be up.
  Nothing secret.
- **Two disks** on a ZFS dataset of its own, `tank/enc/computers/<id>`, its
  quota the disk size: `system.qcow2`, which a reset rebuilds, and
  `home.qcow2`, which nothing but deleting the computer removes.
- **The guest** is the same NixOS base for every computer, built by the
  release. The member's Nix file reaches it on a read-only config disk and
  the guest builds its own system from it (`nixos-rebuild`, nixpkgs pinned
  by the release). The member's Nix is never evaluated or built on the
  host.
- **Stop and start** as games do: QMP `system_powerdown`, a minute, then
  `quit`. A switch never restarts a running computer.

## Network

- **In: the Commonty network only.** Each computer has a port from a
  fleet-wide pool, kept in its record so it stays the same when it moves,
  forwarded to the guest's SSH, and more forwarded ports as its config
  says. A name, `vm-<name>.<owner>.<domain>`, follows the lease like every
  other service's (the DNS Worker). No public name, no tunnel.
- **Who may reach it**: a `dd-computers` chain beside `dd-games`, refilled
  every 15 seconds from the computers API: one rule per (allowed device,
  port), from the identity state's devices.
- **Out: the web.** The guest's traffic leaves through QEMU's user network,
  so on the host it is the `dd-computers` user's own sockets, and the host's
  firewall filters it by owner (`-m owner --gid-owner dd-guests`):
  - refused: port 25; the house's networks (RFC 1918, link-local, CGNAT
    and so the Commonty network); the host's loopback, which QEMU's user
    network otherwise hands the guest at `10.0.2.2`;
  - everything else allowed, and limited per computer (tc on the
    computer's cgroup) so one computer can't take a house's line.

## Placement and moves

- **What a box can host**: each box writes `vm.prom` to its facts, and its
  heartbeat carries it:
  - `dd_vm_max_mem_bytes`: its memory, less what the services it carries
    keep aside, less 2G;
  - `dd_vm_max_threads`: its threads, less one;
  - `dd_vm_max_disk_bytes`: what `tank/enc/computers` has free under its quota.
  Computed on the box every minute (`dd-vm-facts`). A box with no KVM, or
  one draining or failing, writes zeros.
- **Where it may run**: the boxes whose facts fit it and, for "my boxes
  only", the owner's boxes in the identity state (`members.<name>.boxes`).
  The consent is a placement input, in the needs type's "who may run it".
- **When it moves**: only when its box fails. Its needs say
  `moves = "on-failure"`; `place.Decide` keeps it with its lease holder
  while that box is alive and it fits, however much better another box
  scores. With "my boxes only" and one box, it waits for that box.
- **What it loses**: a move restores the newest snapshot, so up to 15
  minutes of changes and a copy round (5 minutes). The old box's dataset
  is kept, named for when, and never deleted without the owner's yes.

## Snapshots

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

- **One API**, `compute.<domain>/api`, behind the gate: configs (list,
  save, delete), computers (make, start, stop, rebuild, reset, delete),
  the serial console over a websocket.
- **The page**, `compute.<domain>`: the configs, the computers, the
  console, the "anyone's box" confirmation. Same layout as the games page.
- **The CLI**: `dd vm config save <file>`, `dd vm make <config>`,
  `dd vm start|stop|rebuild|delete <name>`, `dd vm ssh <name>`,
  `dd vm console <name>`. The CLI asks the same question before
  "anyone's".

## The records

Configs and computers are signed items in the control plane's log,
judged by `state/computers`
([computers.go](../../servers/system/control/state/computers/computers.go)) in the same
pass as the identity judge (`state/ident`) and against the identity state
at that same point. The identity judge leaves these kinds alone. Every
box's dd-control serves the result at `127.0.0.1:4195/computers`
(`?owner=<name>` for one member's, `?since=<changed>` for 304), the same
everywhere. No box decides anything about a computer.

| kind | signed by | body (JSON) |
|---|---|---|
| `computer-config` | the owner's root | `owner, name, memory, threads, disk, nix, ports[], players[], consent, version, gone?` |
| `computer` | the owner's root | `owner, id, name, config, memory, threads, disk, nix, ports[], players[], consent, anywhere, version` |
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
  for `any`, refused for `own`. `own` needs at least one box of theirs. `id` is 16 hex, made by the signer; `name` is
  `[a-z0-9][a-z0-9-]{0,31}`, one per owner. Every field is written,
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
  never deleted by the judge), and for `own` the owner's `boxes` from the
  identity state. Configs, with the deleted ones left out. The Default
  config is not an item: the page and the CLI offer it to everyone.

## Open

- Signing from the page: power changes with the browser's device key,
  everything else with the passkey (root). Not built yet.
