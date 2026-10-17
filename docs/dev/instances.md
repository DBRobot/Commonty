# Remote computers

Status: design, agreed with David 2026-10-17. Nothing built.

A member keeps a computer of their own on the fleet: a VM with its own
disk, reached over the Commonty network, that reaches the web. They make
it from a config they save (memory, threads, disk, packages, who may reach
it, where it may run), from the browser or the CLI. It runs until they stop
it, keeps their files, and moves to another box only if its box fails and
they said it may.

It is built from what game servers already do
([games](games.md)): one QEMU/KVM guest per instance, a record on the
host, a template unit, a firewall chain refilled from who may reach it.

## What a member decides

- **Memory, threads, disk**, each its own and changeable. A member's
  running instances together stay within 16G and 8 threads
  (`fleet/policy.json` `vms`, for now). Disk counts toward their storage
  allowance, as game worlds do from now on (below).
- **Packages**, in a Nix file: a NixOS module for the guest
  (`environment.systemPackages`, services, users' shells). Saving it
  rebuilds the guest's system. `/home` is a disk of its own and survives
  every rebuild.
- **Who may reach it**: the member's own devices, always, on SSH; and
  more ports for themselves or named friends.
- **Where it may run**: on their own boxes only, or on anyone's. Anyone's
  says first, and has to be confirmed before the instance is made:

  > Whoever has that box in their house can read this computer's disk
  > and memory: your files, and anything you sign in to on it, such as
  > Claude. Run it on someone else's box?

  A member with no box of their own can only choose anyone's. The
  instance can't be made without the yes.

A config is a saved thing apart from the instances made from it: make
three the same, change the config, rebuild one.

## How it fits together

```
browser: compute.<domain>        CLI: dd vm ...
          │                           │
          ▼                           ▼
     gate (who they are) ──► the instance API ──► control plane: the agreed state
                                                    instance records, configs,
                                                    owner, consent, port
                                                          │
                                place.Decide: which box, from each box's
                                vm.prom facts and the owner's consent
                                                          │
                                                          ▼
     the box the plan names ── lease svc/vm-<id> ── dd-vm@<id>.service
                                                   qemu: system.qcow2, home.qcow2
                                                   on tank/enc/vms/<id>
                                                          │
                                   every 15 min: ZFS snapshot → restic → the
                                   instance's bucket → copies on other boxes
```

## The instance

- **One service per instance**, `vm-<id>`, `placed` and `lease = true`,
  so each has its own lease, its own fence and its own move. The unit is
  `dd-vm@<id>` (`placed.nix` learns to map a service to a template
  instance; it refuses `@` today).
- **Its record is agreed state**, not a file on one box: a box that dies
  can't take the only copy of what the instance is with it. It holds the
  owner, the config, the consent, its port, and whether it should be up.
  Nothing secret.
- **Two disks** on a ZFS dataset of its own, `tank/enc/vms/<id>`, its
  quota the disk size: `system.qcow2`, which a reset rebuilds, and
  `home.qcow2`, which nothing but deleting the instance removes.
- **The guest** is the same NixOS base for every instance, built by the
  release. The member's Nix file reaches it on a read-only config disk and
  the guest builds its own system from it (`nixos-rebuild`, nixpkgs pinned
  by the release). The member's Nix is never evaluated or built on the
  host.
- **Stop and start** as games do: QMP `system_powerdown`, a minute, then
  `quit`. A switch never restarts a running instance.

## Network

- **In: the Commonty network only.** Each instance has a port from a
  fleet-wide pool, kept in its record so it stays the same when it moves,
  forwarded to the guest's SSH, and more forwarded ports as its config
  says. A name, `vm-<name>.<owner>.<domain>`, follows the lease like every
  other service's (the DNS Worker). No public name, no tunnel.
- **Who may reach it**: a `dd-vms` chain beside `dd-games`, refilled
  every 15 seconds from the instance API: one rule per (allowed device,
  port), from the identity state's devices.
- **Out: the web.** The guest's traffic leaves through QEMU's user network,
  so on the host it is the `dd-vms` user's own sockets, and the host's
  firewall filters it by owner (`-m owner --uid-owner dd-vms`):
  - refused: port 25; the house's networks (RFC 1918, link-local, CGNAT
    and so the Commonty network); the host's loopback, which QEMU's user
    network otherwise hands the guest at `10.0.2.2`;
  - everything else allowed, and limited per instance (tc on the
    instance's cgroup) so one instance can't take a house's line.

## Placement and moves

- **What a box can host**: each box writes `vm.prom` to its facts, and its
  heartbeat carries it:
  - `dd_vm_max_mem_bytes`: its memory, less what the services it carries
    keep aside, less 2G;
  - `dd_vm_max_threads`: its threads, less one;
  - `dd_vm_max_disk_bytes`: what `tank/enc/vms` has free under its quota.
  Computed on the box every minute (`dd-vm-facts`). A box with no KVM
  writes zeros.
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

- Every 15 minutes, a ZFS snapshot of the instance's dataset, read by
  restic into the instance's bucket, `vm-<id>`, in the box's own store.
  restic's chunks mean a 50G disk with a few changes sends a few changes.
  The store's copies send them on to the boxes the plan picks.
- **The repository's key** is made by the first holder and sealed to every
  box able to run the instance, as postgres's archive key is. A box
  keeping a copy that can't run it holds ciphertext.
- Snapshots are pruned to the last day of 15-minute ones and a week of
  dailies. They count toward the allowance once, not once per copy.

## Storage allowance

Instances and game worlds count toward the member's allowance
(`allowanceGb`), by what their datasets hold (ZFS `used`), not what their
disks may grow to. `usage.nix` counts both on each box; the ledger adds
them up as it does the rest. A member over their allowance can't make or
grow an instance; a running one isn't stopped.

## The API, the page, the CLI

- **One API**, `compute.<domain>/api`, behind the gate: configs (list,
  save, delete), instances (make, start, stop, rebuild, reset, delete),
  the serial console over a websocket.
- **The page**, `compute.<domain>`: the configs, the instances, the
  console, the "anyone's box" confirmation. Same layout as the games page.
- **The CLI**: `dd vm config save <file>`, `dd vm make <config>`,
  `dd vm start|stop|rebuild|delete <name>`, `dd vm ssh <name>`,
  `dd vm console <name>`. The CLI asks the same question before
  "anyone's".

## Build order

1. The guest, the `dd-vm@` template, the two disks, the outbound rules,
   on node1 only, from a record on the box (as games are today).
2. The instance API, the page, the CLI, and the inbound chain.
3. Usage: instances and game worlds into the allowance.
4. Records in the agreed state, `vm.prom`, the consent filter,
   `moves = "on-failure"`, `placed.nix` templates (with the placement
   session).
5. Snapshots, the key, failover restore.

## Open

- Where the instance API runs. It writes agreed state, so it is a client
  of the control plane like the identity judge. One API on every box, or
  on the box holding a lease?
