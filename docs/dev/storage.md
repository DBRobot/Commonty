# Storage

Status: each box's own **store**
([`storage/store.nix`](../../nix/modules/storage/store.nix)), on every box,
holds every bucket. The old shared cluster is gone from the config
(below).

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
- **Copies**: every 30 minutes, `dd-store-copy` copies each bucket from
  the box that writes it (where its service runs) to the other boxes that
  keep it; every 30 seconds, `dd-store-copy-recent` copies what was written
  in the last 15 minutes. That age is the store's own, from the bucket's
  listing (`--use-server-modtime`): rclone's own is a header on each
  object, and reading it was a request for every object on both sides on
  every run. Which boxes keep it is the control plane's: the plan
  (`place.Decide`'s copies) keeps a bucket where it is written, then where
  it is already, then on the boxes whose stores have the most room, and
  only where it fits. Until a plan has been seen, the first boxes by name.
  A copy is added to, never pruned: what is deleted here stays there.
  Deletes reaching the copies is a decision still to make.
- **Whose a bucket is**: most are written where their service runs. A
  box's own (its backup, `backups-<box>`) is written by that box alone,
  wherever it is kept, and only that box copies it out; so are the
  libraries, by the one box built with them. The mail is written where the
  plan runs its service. A copy held elsewhere is never sent back, so what
  the writer deletes stays deleted.
- **Shared buckets**: the metric blocks and the nix cache are kept in one
  box's store, its home: the first store, by name, of a box built with
  and able to run the bucket's service (`homes` in
  `nix/lib/fleet-placement.nix`). The plan keeps it there wherever the
  service runs, and every box writes it there with its own key to that store, handed to it sealed as its
  copies' key is (`dd-store-at`, in `/var/lib/dd-store/at/<bucket>.env`).
  One copy: what is in them can be made again.
- **Room**: each store says how big it is, what it has free and what it
  keeps of each bucket, in its box's heartbeat. It keeps a fifth of its
  pool for the box's other services, measured on the pool, not the
  store's dataset. With a twentieth left it
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

## The old pool

Until 2026-10, the services used a garage cluster node1 and node2 shared
(two copies of everything, one rpc secret). Each bucket was copied into the
store of the box that writes it (`dd-store-migrate`, copying, never
deleting), the services moved, and the cluster was taken out of the
config. Its two halves are deleted by hand, with the owner's yes
([pool-retire.md](pool-retire.md)).

## Turning it on

Every box has a store, sized by its disk (`capacity = "auto"`), with
nothing set up by hand. The move off the shared pool, and what is left to
do once for the fleet: [failover](failover.md#turning-it-on-once-for-the-fleet).
