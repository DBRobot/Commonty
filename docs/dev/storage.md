# Storage

Status: two kinds side by side. The services still use the garage
**cluster** that node1 and node2 share. Each box's own **store**
([`storage/store.nix`](../../nix/modules/storage/store.nix)) runs on every
box beside it, and the buckets are copied into it; the services read it
once failover is on (below).

## Why each box gets its own

The cluster's rpc secret is shared by every box in it. Holding it makes a
box the cluster's admin: it can read and delete every object. That can't
hold once a box sits in someone else's house, and "no box holds another
box's secrets" rules it out anyway.

With two boxes the cluster also refuses every write while either one is
away. Its two copies or nothing is its consistency rule. So a service that
fails over to the box still up can read everything, but write nothing,
until the other is back.

## The store

- **A garage of one** per box, with a secret the box makes on its first
  start and shares with nobody. Its rpc is on loopback. Its S3 is on
  `127.0.0.1:3910` for the box's own services, and at
  `https://store.<box>.<domain>` for the other boxes, a name that exists
  on the fleet's network alone.
- **A key per bucket** for the service that uses it, made on the box, in
  `/var/lib/dd-store/keys/<bucket>.env`. Only that service reads it.
- **A key per other box**, for the copies it sends here, allowed only on
  the buckets it copies. It is sealed to that box's secrets key (its TPM)
  and served at `https://store.<box>.<domain>/_keys/<other>.age`: public,
  and only the box it is for can open it.
- **Copies**: every 5 minutes, `dd-store-copy` copies each bucket from the
  box that writes it (where its service runs) to the other boxes that keep
  it. Which boxes keep it is the control plane's: the plan
  (`place.Decide`'s copies) keeps a bucket where it is written, then where
  it is already, then on the boxes whose stores have the most room, and
  only where it fits. Until a plan has been seen, the first boxes by name.
  A copy is added to, never pruned: what is deleted here stays there.
  Deletes reaching the copies is a decision still to make.
- **Room**: each store says how big it is, what it has free and what it
  keeps of each bucket, in its box's heartbeat. With a twentieth left it
  refuses what other boxes send (`507`), and still serves what it keeps.
  Its dataset's quota is its capacity, so its own services can't fill the
  pool either. A service isn't placed where its buckets wouldn't fit, and
  a box refuses to start one it hasn't the memory for; it says so, and the
  plan gives the service to another box.
- **What is stored is ciphertext**: photos, files and films are encrypted
  on members' devices, backups by restic, the postgres archive by wal-g.
  A box holding a copy holds nothing it can read.
- **Failover**: a bucket's copies carry the same name, so a service that
  moves to a box holding a copy reads it there at once, and writes there.
  The writes copy back out the same way.

## Turning it on

Every box has a store, sized by its disk (`capacity = "auto"`), with
nothing set up by hand. The move off the shared pool, and what is left to
do once for the fleet: [failover](failover.md#turning-it-on-once-for-the-fleet).
