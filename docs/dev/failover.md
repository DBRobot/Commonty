# Failover

Status: built and off. One switch turns it on for the whole fleet
(`fleet/policy.json` `failover`), once the steps below are done. A box
that joins later needs nothing done by hand.

When the box running the fleet's services goes away, another box takes
them over. No box holds another's secrets, nothing is shared between
them but ciphertext, and a member signs in again without noticing.

## What moves, and how

The services with one writer move together, as one group. They follow
leases from the [control plane](control-plane.md), and each needs the
box the database is on:

- **Postgres** follows its lease
  ([`postgres-follow.nix`](../../nix/modules/storage/postgres-follow.nix)).
  Its archive goes to the box's own store, encrypted with a key sealed to
  each box able to hold the lease. The next holder restores from there.
- **Photos.** Museum's keys are made on the box. A box taking photos over
  makes its own, and carries every account across: museum keeps each
  account's address beside it, which is only ever
  `<member>@users.<domain>`, a name and not a secret. Under new keys it
  encrypts and hashes those addresses again
  ([`museum-own-keys.patch`](../../servers/photos/museum-own-keys.patch)).
  The photos are in the box's own store, encrypted on members' devices.
- **Passwords.** The vault's sign-in secret is made on the box, for the
  gate on the same box. The key its sessions are signed with stays on the
  box's own disk, never in the bucket. Its files are in the store.
- **Git.** Its repositories and files are copied out of a ZFS snapshot
  every 2 minutes ([`follow.nix`](../../nix/modules/forge/follow.nix)),
  with the list of what the snapshot held. A box taking over restores only
  what the list names, so nothing deleted comes back. Its own config and
  secrets stay its own.
- **The network's control server.** Headscale's database is copied every
  minute, sealed with age to the boxes able to run it
  ([`follow.nix`](../../nix/modules/net/follow.nix)). A box keeping a
  copy that can't run it can't read it. The new holder makes its own
  server key.

A box taking over sets aside what it had before, named for when, and
keeps the two newest of those (a take run again and again filled node2's
disk with 141G of them on 2026-10-25). A forge restore stopped part way
goes on where it was the next time, setting nothing more aside. A forge
copy older than the one before the newest its records name is not taken:
the box refuses the forge and one with the newer copy takes it, or, if
the owner says so (`touch /var/lib/dd-forge/take-older`), it is taken all
the same.

The placement algorithm keeps these together: each depends on the box
postgres is on, and headscale's box is the gateway's.

## The names

The [DNS Worker](../../servers/system/dns/worker.js) reads the agreed
state from the control plane's witness:

- Each service's names point at the box holding its lease: `photos`,
  `api` and the rest at museum's, `vault` at the vault's, `git` and
  `code` at the forge's.
- A placed service with no lease (needs `hosts`) has its names at a box
  the agreed plan gives it: the witness plans on the agreed state with the
  release's placement inputs, as every box does, and the relay hands the
  Worker that beside the leases (`placed`). The gateway's box goes first
  while it is one of them: `llm` at the chat's, `metrics` and `grafana`
  at the fleet view's, `games` at the games', `cache` at harmonia's. A
  box's own word that it runs something (`runs/<service>`) moves no name,
  and a box that joined on its own key gets one only when the plan gives
  it the service, at the address the agreed state has for it. Placed
  nowhere, the name is the gateway's, whose page says one moment.
- A service under a lease follows its holder alone: with the lease lapsed
  its names are the gateway's, whatever the plan says.
- A public name goes to the first of its boxes with a tunnel, else the
  gateway's.
- A service answering under another's name goes where that one does
  (its needs depend on it locally): search under `llm`, the joined
  boxes' metrics under `metrics`, mail's pipe and the transcoder under
  `files`, which is the gateway's. A box that joined on its own key sends
  its metrics to `metrics.<domain>` on the store's port, so they follow
  too.
- `*`, the bare name and the public hosts point at the box holding the
  network's lease.
- Home's tiles are the whole fleet's, whichever box serves the page.
- A public name is a CNAME to that box's own tunnel. Each box makes its
  tunnel's secret itself. It asks the Worker, signed with its TPM key, to
  make the tunnel at Cloudflare under its name, and keeps the secret. The
  Worker keeps nothing.
- Names move within a minute. A box that takes a lease asks at once
  (`/api/converge`).
- With no agreement fresher than two minutes, nothing moves.

## The data

Each box has its own [store](storage.md). The plan says which boxes keep
each bucket, by room. Recent changes go out every 30 seconds, and
everything every half hour, with a copy read back to check it.

A box refuses to start a service it hasn't the memory for, and says so.
The plan then gives the service to another box.

## How long a takeover loses

- **Postgres:** what was not yet archived, at most about 30 seconds
  (`archive_timeout`), plus a copy round.
- **Photos and files:** what was not yet copied, at most about 30 seconds.
- **Git:** at most 2 minutes of pushes. A copy sends what changed since
  the snapshot the last one was taken from, and the rest by size alone,
  reading no other file (with `--checksum` every file was read, a copy of
  node1's 18G took 3 minutes, and no handover's copy finished within the
  lease's time).
- **The network:** devices that joined in the last minute.

The old box's own data is kept, so nothing is lost for good if it comes
back.

## A box that joins

Nothing is set up by hand:

- **Its store**, on a dataset of its pool, as big as the pool allows less
  a fifth kept for its other services, measured every minute.
- **What it carries**: every service its capability lets it run, by its
  own report, the owner's rules (`nix/lib/capability.nix`) and the
  secrets its own file holds, built in by the bundles that bring them
  (`nix/lib/fleet-placement.nix` `carried`). Nothing is listed by hand;
  the control plane decides what runs where.
- **Its tunnel**, which it asks the DNS Worker for with a secret it makes.
- **Its names**: `store.<box>.<domain>` and its certificate.
- **The keys sealed to it** (postgres's archive, the network's database)
  at the holder's next base backup or copy.
- **Copies** come to its store once the plan finds room there.

A secret that only another box's file holds (the demo's password, TMDB's
token) is something it runs without: `dd.hasSecret`.

## Turning it on, once for the fleet

1. **The tunnel token**: `dd dns tunnels`, once. The Worker holds it; no
   box does.
2. **The move off the shared pool**: done; every service uses its box's
   own store (storage.md, [The old pool](storage.md#the-old-pool)).
   Photos and Passwords reach their store only with failover on, and say
   so if it is off.
3. **The switch**: `"failover": true` in `fleet/policy.json`. The release
   that turns it on restarts postgres and what follows it once, on the box
   holding them; they start again as soon as the control plane grants
   their leases.
4. **Room on node2**: a store keeps a fifth of its pool for the box's
   other services, so node2 takes no copies while its half of the old pool
   is on its disk. Once that is deleted, with David's yes
   ([pool-retire.md](pool-retire.md)), the copies come to it by room.

## Known gaps

- The demo's Photos password and the storage ledger's are in node1's sops
  file. On another box the demo gallery and the ledger don't work until
  they're re-made there.
- A member's Ente two-factor secret, if one were set, isn't carried over.
  The fleet's sign-in doesn't use it.
- A box can claim more free room than it has to attract a copy. The read
  back catches a copy that isn't kept, and that copy then counts for
  nothing.
