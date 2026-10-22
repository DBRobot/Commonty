# Retiring the old pool

The garage cluster node1 and node2 shared (`1ed04b4160b765e2` and
`eca3acbf439670ef`, two copies of everything, on :3900) is replaced by each
box's own store ([storage](storage.md)). This is the order it goes in, and
the deleting, which the owner does by hand. Nothing here is done by a
release or an agent: a release never deletes a dataset.

## What was where (2026-10-21)

```
bucket          old pool   moves to                       how
ente            72.6 GiB   node1's store                  copied; already all there
vaultwarden     1.6 KiB    node1's store                  copied; already all there
libraries       81.7 GiB   node1's store (built with it)  copied hourly by dd-store-migrate
mail            642 B      node1's store                  copied hourly
metrics         1.0 GiB    node1's store (its home)       copied hourly
nix-cache       49.2 GiB   node1's store (its home)       copied hourly; could start empty
backups-node1   97.8 GiB   not moved                      a new repository in node1's store
backups-node2   5.0 GiB    not moved                      a new repository in node2's store
media           0          nothing                        empty, nothing writes it
```

On disk the old pool is `vault/enc/photos` on node1 (397G) and
`tank/enc/garage` on node2 (312G), each with a week of snapshots, plus
garage's metadata in `/var/lib/garage` on each (about 800M).

The old backup repositories are not copied: node1's newest snapshot is
7.2 GiB of its 97.6 GiB repository, and node2's 0.6 GiB of 4.7. The rest of
node1's is `/srv/images` from before 2026-10-11, kept because forget
grouped by paths (fixed: by host). Deleting the old pool deletes those
repositories and their history: up to 35 snapshots on node1 and 17 on node2.
To keep them, copy each into its box's store before step 6 (it fits on
node1; node2's 5 GiB fits once its half is gone) and say so first.

## The order

1. **Copying** (branch poolmove): every bucket gets a place in a store and
   dd-store-migrate copies it there every hour. On node1, wait until every
   bucket says all here:

   ```
   ssh admin@100.64.0.2 cat /var/lib/dd-facts/store-migrate.prom
   ```

   every line ending in ` 1`. The first copy of the libraries takes a while;
   it is node1's disk to node1's disk.

2. **The switch** (branch poolswitch): backups, the libraries, the mail and
   the metrics use the stores. Each bucket is copied once more after it,
   then never again. The next night's backup (03:00) is each box's first
   snapshot in its new repository.

3. **The cache** (branch poolcache): the nix cache is in node1's store, and
   `dd release publish` reads node1's key to it over ssh.

4. **Failover on** (fleet/policy.json, its own branch): Photos and Passwords
   write the store; ente and vaultwarden are copied once more.

5. **The check**, from the checkout, while the old pool still runs:

   ```
   scripts/pool-check
   ```

   Every line `ok` (or `note` for metrics and nix-cache, which can be made
   again), and `== complete` at the end. Before it is run, on node1
   `store-migrate.prom` shows `once="true"} 1` for every bucket: each was
   copied once more after its switch. A bucket `FAIL`ing with keys
   `missing`: if the service has run on the store for a while, a member
   may have deleted them since (the libraries' trash, a photo); look at the
   names before going on. Anything else: stop.

6. **The config** (branch pooldrop): no box runs the old pool's garage, and
   nothing points at it. Its data is untouched. After the release lands on
   both boxes, check:

   ```
   ssh admin@100.64.0.2 'systemctl is-active garage; systemctl --failed'
   ssh admin@100.64.0.5 'systemctl is-active garage; systemctl --failed'
   ```

   `inactive` (or `unknown`) for garage, and nothing failed. Photos, Files,
   Passwords and the metrics pages open; `dd release publish --dry-run`
   finds the cache.

## Deleting node2's half

Only after step 6. node1 still holds a whole copy of the old pool, and its
store holds every bucket.

```
ssh admin@100.64.0.5
systemctl is-active garage                      # must not say active
findmnt /srv/garage                             # tank/enc/garage, nothing else
sudo find /proc/*/fd -lname '/srv/garage/*' 2>/dev/null | head   # nothing has it open
zfs list -r -t all -o name,used tank/enc/garage # the dataset and its snapshots
sudo zfs destroy -r -n -v tank/enc/garage       # dry run: what would go
sudo zfs destroy -r tank/enc/garage             # no snapshot is kept
sudo rm -rf /var/lib/garage                     # its metadata, ~770M
zfs list -o name,used,avail tank                # about 360G more available
```

Then node2's store says it has room (a fifth of the pool is kept back for
the box), and the plan sends it copies by room. Watch them arrive:

```
ssh admin@100.64.0.5 cat /var/lib/dd-facts/store.prom
scripts/pool-check copies
```

ente (73 GiB), the libraries (82 GiB) and node1's backups land first; about
170 GiB in all, a few hours over the house's network.

## Deleting node1's half

Only once `scripts/pool-check copies` says `ok` for every bucket the plan
keeps on both boxes: until then node1's store is the only copy of some of
them, and `vault/enc/photos` the second.

```
ssh admin@100.64.0.2
systemctl is-active garage                       # must not say active
findmnt /vault/photos                            # vault/enc/photos, nothing else
sudo find /proc/*/fd -lname '/vault/photos/*' 2>/dev/null | head  # nothing has it open
zfs list -r -t all -o name,used vault/enc/photos # the dataset and its snapshots
sudo zfs destroy -r -n -v vault/enc/photos       # dry run
sudo zfs destroy -r vault/enc/photos             # no snapshot is kept
sudo rm -rf /var/lib/garage                      # its metadata, ~810M
zfs list -o name,used,avail vault
```

## After

- The sops keys nothing reads any more, in the box files and fleet.yaml:
  `garage-rpc-secret`, `garage-backup-key-id`, `garage-backup-key-secret`,
  `thanos-key-id`, `thanos-key-secret`, `cache-key-id`, `cache-key-secret`,
  `library-key-id`, `library-key-secret`, and (with failover on)
  `garage-key-id`, `garage-key-secret`, `vault-key-id`, `vault-key-secret`;
  in `secrets/fleet.yaml`, `cache.keyId` and `cache.keySecret`. Leaving them
  costs nothing; they open nothing that still exists.
- `scripts/pool-check` and the old pool's half of this page can go.
