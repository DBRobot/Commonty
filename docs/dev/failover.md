# Failover

Status: built and off. One switch turns it on for the whole fleet
(`fleet/policy.json` `failover`), once the steps at the end are done.

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
never deletes it.

The placement algorithm keeps these together: each depends on the box
postgres is on, and headscale's box is the gateway's.

## The names

The [DNS Worker](../../servers/system/dns/worker.js) reads the agreed
state from the control plane's witness:

- Each service's names point at the box holding its lease: `photos`,
  `api` and the rest at museum's, `vault` at the vault's, `git` and
  `code` at the forge's.
- `*`, the bare name and the public hosts point at the box holding the
  network's lease.
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
- **Git:** at most 2 minutes of pushes.
- **The network:** devices that joined in the last minute.

The old box's own data is kept, so nothing is lost for good if it comes
back.

## Turning it on

Each needs David:

1. **A store per box:** a `store` block in each box's
   `fleet/boxes/<box>/box.json` (`capacity`, `dataset`, `dir`), with the
   dataset in `datasets`.
2. **The roles:** every box to fail over to carries the same roles as
   node1's photos, vault, forge and gateway (`fleet/boxes.json`).
3. **The tunnel token:** `dd dns tunnels`, once (Cloudflare Tunnel Edit on
   the account). The Worker holds it; no box does.
4. **Moving the buckets** from the shared cluster into the stores, one at
   a time ([storage](storage.md#turning-it-on)). Each delete from the
   cluster needs David's yes.
5. **The switch:** `"failover": true` in `fleet/policy.json`.

The release that turns it on restarts postgres and the services that
follow it once on the box holding them. They start again as soon as the
control plane grants their leases, in seconds.

## Known gaps

- The demo's Photos password and the storage ledger's are in node1's sops
  file. On another box the demo gallery and the ledger don't work until
  they're re-made there.
- A member's Ente two-factor secret, if one were set, isn't carried over.
  The fleet's sign-in doesn't use it.
- A box can claim more free room than it has to attract a copy. The read
  back catches a copy that isn't kept, and that copy then counts for
  nothing.
