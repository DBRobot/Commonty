# The fleet of boxes

`fleet/` is what the fleet is, as data: the boxes and the roles each one
runs, what each box reported about its own hardware, what its owner chose
for it, who the members are, and the owner's policy. No Nix in it says what
a box runs; [`flake.nix`](../../flake.nix) reads these files and builds one
NixOS system per box from them. This page is how the folder is laid out,
who reads each file, and how to change it.

The folder holds no secret. Every file here is public by design: keys are
public halves, ids are opaque, and the private side (who owns which site,
the cache writer's key) is in `secrets/fleet.yaml`, encrypted with sops.

## Where things are

```
fleet/
  boxes.json              every box: its roles, its addresses, its public keys
  boxes/<box>/
    hardware.json         what the box found on itself (signed by its TPM)
    hardware-configuration.nix   nixos-generate-config's output (signed with it)
    report.sig            the TPM's signature over the two files above
    box.json              what the box's owner chose: house, pools, datasets
  roles.json              what each role needs of a box
  policy.json             the fleet owner's choices: domain, repo, people, allowance
  members.json            who may use the services, as hashes of their roots
  release.pub             the release key's public half
  cache-keys.pub          the keys that sign closures in the nix cache
  control.json            the control plane's voters and rules
  control-worker.json     where the control relay Worker is (dd control setup)
  mail.json               where the mail Worker is (dd mail setup)
  dns-worker.json         where the DNS Worker is (dd dns setup); with it,
                          the zone is the Worker's: names and certificates
  id-worker.json          where the id Worker is (dd id setup); with it,
                          sign-in is there, once for every box
  layout.nix              the one disk layout every box gets
```

Not to be confused with [`servers/fleet/`](../../servers/fleet), which is
Settings' Backups, Devices and Network tabs ([servers.md](servers.md#the-services)).

## The rule: no box is trusted

Nothing in this folder marks a box as trusted, and nothing should. There is
no `trusted`, `safe` or `privileged` field on a box, and a change that adds
one is the wrong change. Every box is assumed to be readable by whoever
holds it.

Where a service does read people's plaintext on the box that runs it, that
is a fact to see, not a permission to grant. Its module says so with
`dd.box.plaintext = [ "<service> (what it reads)" ];`
([`nix/modules/box/box.nix`](../../nix/modules/box/box.nix)); the box
publishes the list as a metric. Nothing refuses it, and
`checks.plaintext-label` ([nix/tests/plaintext-label.nix](../../nix/tests/plaintext-label.nix))
checks that labelling is all that happens: a label is a fix still owed,
not a gate. The fix is to make the service stop reading plaintext.

Two things here look like trust and are not:

- **`ownHardware` in `policy.json`** keeps a service on boxes whose
  `box.json` owner is the fleet's owner. It is about whose hands the
  hardware is in, for services that hand people the code that handles
  their secrets (a page a master password is typed into). It says nothing
  about the box being safe.
- **`public` in `boxes.json`** says whether the box's gate answers at the
  fleet's public names. It changes how other boxes reach its directory,
  not what they believe from it.

## boxes.json

One entry per box, by name. The name is the box's hostname and its folder
under `fleet/boxes/`.

```
<box>
  hostKey     the box's ssh host key (ed25519). scripts/install-box checks it
              before wiping anything; dd-attest signs with it; release
              publish checks those signatures against it
  net         the box's address on the fleet's own network (headscale,
              100.64.0.0/10). Headscale keeps a node's address for good; a
              box that joins again as a new node gets a new one
  public      true where the box's gate answers at the public names: other
              boxes pull its directory at https://files.<domain>/_dd/directory,
              else at http://<net>:4181/_dd/directory
  siteId      an opaque id for the house the box is in (s-xxxxxxxx)
  regionId    an opaque id for its region (r-xxxxxxxx); garage's zone
  roles       the roles it runs: one file each in nix/roles/<role>.nix;
              core always
  garage      on a box with the storage role:
    id          its garage node id, once garage has started there once
    capacity    the space it offers the cluster ("3T", "400G")
    dataDir     where its blocks live (a dataset in its box.json)
  unlockKey   the P-256 key its TPM signs with, base64 SPKI
              (dd-unlock-enrol keys prints it)
```

The names behind `siteId` and `regionId`, and each box's owner and
contents, are in `secrets/fleet.yaml`; `dd box list --private` shows them
to whoever holds the release signer's sops key.

### Who reads it

```
flake.nix                nixosConfigurations: one per entry, its roles' files, and
                         the wiring derived from the list: peers, house peers,
                         garage's peers and zone, where the cache and backups go,
                         the agent's reach list, headscale's owners and box keys
flake.nix (control)      the control plane's boxes: ids 101.., house, key
flake.nix (mail-worker)  BOXES in the Worker's fleet.js: each unlockKey
nix/roles/core.nix       the other boxes' net addresses, and this house's
nix/modules/storage/view.nix   each box's roles, for who may count what
nix/tests/boxes.nix      checks.boxes: the list against the built systems
nix/tests/role-fit.nix   checks.role-fit (below)
box-reports/check.py     checks.box-reports: report.sig against unlockKey
entry_point.yml          CI's hosts (every box) and builders (boxes with runner)
dd release               which boxes to build, ask to vouch, and nudge
dd box list | public     what the list says, and what the box says
scripts/install-box      hostKey and net, to find and check the box
scripts/box-report       net, to reach the box
```

## fleet/boxes/\<box\>/

Two kinds of file, with different owners.

**The box's report**: `hardware.json`, `hardware-configuration.nix` and
`report.sig`. The box makes them at every boot
([`nix/modules/box/report.nix`](../../nix/modules/box/report.nix),
[`report/detect.sh`](../../nix/modules/box/report/detect.sh)) in
`/var/lib/dd/report/`, and signs the pair with its TPM-held key.
`scripts/box-report <box>` copies them here. Nobody edits them by hand:
`checks.box-reports` ([nix/tests/box-reports/check.py](../../nix/tests/box-reports/check.py))
verifies `report.sig` over

```
"commonty box report v1\0" <box> "\0" sha256(hardware.json) "\0" sha256(hardware-configuration.nix)
```

against the box's `unlockKey`, and fails for a folder with no entry in
`boxes.json`, an entry with no folder, or a missing signature. The report
is only made on a box with `encrypted` set, since that is what enrols the
TPM key.

```
hardware.json
  hostId          ZFS's host id                 networking.hostId
  bootDisk        /dev/disk/by-id/...           layout.nix's device
  wired, wifi     the wired NIC's MAC, the wifi interface   dd.home
  usbNic          the wired link is USB          (nothing now: every box has
                                                 USB power saving off)
  secretsKey      age1tpm1...: the TPM key secrets/<box>.yaml is
                  encrypted to; also headscale's recipient for the box
  firmware        uefi, secureBoot, setupMode, tpm {version, p256, ppi},
                  settingsFromLinux, windowsKey
  cpu             arch, vendor, model, threads, flags
  memoryBytes
  virt            kvm, nested
  gpus[]          render, driver, vendor, encode (VAAPI profiles)
  disks[]         id, kind, usb, bytes            smartd (observe/disks.nix)
  pools[]         name, vdevs, bytes, encrypted, redundancy
  nics[]          name, mac, driver, wireless, usb, initrd
  power           battery, mains
  gpu             the first GPU only: older reports; nothing reads it
```

[`nix/modules/box/host.nix`](../../nix/modules/box/host.nix) turns the
report into the box's NixOS settings: the hardware configuration, the
disk layout on `bootDisk`, `dd.box.threads`, `memoryBytes`, `disks` and
`gpus`, graphics drivers for what was found, and `dd.box.vaapi` for the
first GPU that encodes H.264. Every box has USB power saving off,
whatever it reported: `usbcore.autosuspend=-1`, and a udev rule that keeps
each USB device's `power/control` on and its USB3 link power states (U1,
U2) off where the kernel offers them.

A box with both `wired` and `wifi` has both house profiles up, the cable at
route metric 10 and the wifi at 20
([home-network.nix](../../nix/modules/box/home-network.nix)). A USB adapter
can come back from a reset with its carrier up and nothing getting
through; NetworkManager then sees nothing wrong and the wifi waits behind a
dead route (node2, 2026-10-11). `dd-wired-watch`
([wired-watch.sh](../../nix/modules/box/wired-watch.sh)) asks the router
over the cable every 30 s (`ping -I`). After 90 s without an answer it
resets the adapter if it is USB (its `authorized` 0 then 1), and if the
cable still does not answer while the router answers over the wifi, it
takes `house-wired` down so the wifi carries everything (with the wifi
down too, or a router that answers neither, the cable stays). While on wifi it listens for the router on the cable with an
ARP probe (no address, so no route moves), and every 10 minutes brings the
profile up to try it outright; `house-wired` comes back the moment the
router answers over it. It never reboots, and writes `dd_wired_state`
(0 ok, 1 failing, 2 on wifi), `dd_wired_since_seconds`, `dd_wired_resets`
and `dd_wired_last_reset_seconds` to the box's facts.

**The owner's choices**: `box.json`. It is written by hand, and it is the
only file in the folder that is.

```
box.json
  owner           a name in policy.json people: whose box this is. Picks
                  the paper key its disk keys are copied to, and the
                  owner checks in role-fit, headscale and build orders
  house
    ssid          the house's Wi-Fi                     dd.home.ssid
    address       the box's fixed address on the house LAN, with prefix
    household     who lives there (switches its ad blocking); a role with
                  "box": ["household"] needs it
  encrypted       datasets under <pool>/enc, unlocked at boot from the TPM
                  and the unlock Worker (disk-unlock.nix)
  extraPools      pools other than tank to import at boot
  pools.bulk      the pool for big, slow data (default tank)
  datasets        dataset -> ZFS properties plus mountpoint; "note" is for
                  people and dropped (dd.zfs.datasets)
  arcMaxBytes     the ARC's cap (zfs_arc_max)
  runnerCapacity  CI jobs at once on this box's runner
  gamesMemoryMiB  memory all game servers may have together
  neverSleep      no suspend, no lid or power key action
```

`owner`, `house`, `encrypted`, `extraPools`, `neverSleep` and `datasets`
are read without defaults: a box.json without one of them fails to
evaluate. The others are optional.

## roles.json

What each role needs of a box. The keys are role names, which are also
the file names in [`nix/roles/`](../../nix/roles); `_` is a note.

```
<role>
  requires    against the box's hardware.json:
                arch, uefi, tpmP256, wiredInitrd, memoryGiB, threads,
                cpuFlags, kvm, nestedKvm
  box         fields its box.json house must have (["household"])
  with        roles that must be on the same box: the code reaches them
              on localhost
  perFleet    exactly this many boxes run it (gateway: 1)
  services    the needs' service names it runs, for policy.json ownHardware
  prefers     what makes it faster ({"gpuEncode": "H264"}); a note today,
              nothing checks it
  why         one line on why, for people
```

### Role-fit

[`nix/lib/fit.nix`](../../nix/lib/fit.nix) takes `roles.json`,
`boxes.json`, the `fleet/boxes/` folder and `policy.json`, and returns the
problems as sentences. `checks.role-fit`
([nix/tests/role-fit.nix](../../nix/tests/role-fit.nix)) fails with them;
`checks.box-reports` builds it too. A box fails when:

- it does not run `core`;
- it names a role `roles.json` does not have;
- its report misses a `requires` (an unknown requirement is a problem
  too, so a typo cannot pass);
- a `with` role is not on it, or a `box` field is not in its house;
- it runs a role whose `services` include one in `ownHardware`, and its
  `box.json` owner is not the policy's `owner`.

And the fleet fails when a `perFleet` role runs on another number of
boxes.

### Placement

Placement is `boxes.json`, by hand. Nothing chooses roles for boxes yet.
Role-fit is the check that keeps the hand-written list honest; placement
will read the same rules, with each service's needs (`.#needs`,
[nix/lib/needs.nix](../../nix/lib/needs.nix)), when it exists
([servers.md](servers.md#needs-and-placement), [hardware.md](hardware.md)).

## policy.json

The fleet owner's choices. One file, read at evaluation.

```
owner          the fleet owner's name (a key in people)
domain         the fleet's domain                    dd.domain (core.nix)
repo           owner/name of the fleet's repository on its forge
                                                     dd.repo: the agent's
                                                     release url, the demo's
                                                     Code tile, forgeMirrors
people.<name>
  ssh          their ssh public keys: the admin login on the boxes they own
  paperKey     the ssh-ed25519 public half of their paper recovery key: each
               of their boxes' disk keys gets a copy encrypted to it in
               /boot/dd (dd.unlock.recovery), opened with dd disk recover
allowanceGb    each member's storage across every service and box
                                                     dd.storage.budgetGb
                                                     (storage/view.nix)
ownHardware    services that run only on the owner's own boxes (role-fit)
forgeMirrors   the forge's push mirrors: [{to, user, repo?}]; repo defaults
               to the fleet's own (roles/forge.nix); the token is the sops
               secret github-mirror-token
```

The one mirror listed is the GitHub copy. Its account is suspended, so
its pushes fail and nothing depends on them ([mirror.md](mirror.md)).

## The other files

```
members.json         {"members": [...], "revoked": [...]}: sha256 of each
                     member's root (identity::member_id), no names. dd member
                     add|remove writes it; the gate reads it (dd.members) and
                     the mail Worker builds it in. An id on revoked is shut
                     out even with a redeemed invite (gate.md#roles)
release.pub          the release key's public half, base64. dd release init
                     writes it. Built into the agent, the gate (invites, the
                     download page) and the mail Worker (releases.md#the-keys)
cache-keys.pub       one name:key per line: the laptop's and each box's nix
                     cache keys. A box's nix trusts only the laptop's
                     (storage/cache.nix)
control.json         validators [{id, name, key}]: each voting box's
                     dd-control key, then the Cloudflare witness; rules
                     {goneAfterMs, leaseTtlMs, leases}. Built into the relay
                     Worker's config and dd-control's (control-plane.md)
control-worker.json  {domain, account, zone, script}: where the relay is
mail.json            {domain, account, zone, kv, script}: where the mail
                     Worker is; the gateway role reads it
layout.nix           the disk: a 1G ESP, the rest one ZFS pool (tank) with
                     system/root (reserved 2G), system/nix, system/var and
                     home; under <pool>/enc when encrypted
```

The Cloudflare ids in `control-worker.json`, `mail.json`,
`dns-worker.json` and `id-worker.json` are not
secret: they name where a Worker is, and the token that can change it
lives on the laptop's keyring ([client.md](client.md#keys-on-a-device)).

## Commands that write here

```
dd member add|remove <name or id>    members.json
dd release init                      release.pub (refuses if a key exists)
dd control setup --validator n=k...  control.json validators, control-worker.json
dd mail setup                        mail.json
dd dns setup                         dns-worker.json
dd id setup                          id-worker.json
scripts/box-report <box> [addr]      boxes/<box>/hardware.json,
                                     hardware-configuration.nix, report.sig
dd box list [--private]              reads boxes.json (and secrets/fleet.yaml)
dd box public <box>                  reads boxes.json, asks the box over ssh
```

Every change here reaches the boxes the same way: commit, merge, and the
next release ([releases.md](releases.md)). A box never reads this folder
from the forge; it is built into the system the release names.

## Checks

```
checks.role-fit       fit.nix over the four files
checks.box-reports    each report.sig, every folder against boxes.json; and role-fit
checks.boxes          every role is a file, every box has core and builds, and
                      the derived wiring says what the list says: peers,
                      dd.box.net, garage's address, zone and peers, Thanos on
                      its own box, the admin's console password, llama-swap
                      shut in
checks.ci             the VM test lists in flake.nix and entry_point.yml agree
```

A change under `fleet/` turns on CI's nix and VM parts
([ci.md](ci.md#how-a-run-fits-together)); every box's system is built,
on every box with the runner role.

## Adding a box

There is no script for a first install yet: `scripts/install-box`
reinstalls a box that is already in the list. The flake builds a box only
from its folder (`host.nix` reads `hardware.json` and `box.json`), and the
signed report in that folder is made by the box itself, so a new box's
first report has to come from the box before its entry can build. What
has to exist, in the order each part depends on the last:

1. **The machine.** UEFI, a TPM 2.0 with P-256, a wired NIC whose driver
   is in the initrd, Secure Boot on with our keys
   ([hardware.md](hardware.md#booting-on-a-stock-pc-decided-2026-10-04)).
2. **Its TPM key.** As root, `dd-unlock-enrol keys` prints the
   `unlockKey`. The report (`dd-box-report.service`) and the secrets key
   (`dd-secrets-key.service`) are made at every boot of a box with
   `encrypted` set.
3. **Its entry in `boxes.json`.** `hostKey` (its
   `/etc/ssh/ssh_host_ed25519_key.pub`, type and key, no comment),
   `unlockKey`, `siteId` and `regionId` (an existing house's, or new
   opaque ids with their names in `secrets/fleet.yaml`), `public: false`,
   and `roles: ["core"]` to start. `net` once it has joined the fleet's
   network (`/run/commonty-net-address` on the box).
4. **Its folder.** `scripts/box-report <box> <address>` copies the signed
   report into `fleet/boxes/<box>/`. Write its `box.json` by hand: owner
   (a name in `policy.json` people), house, `encrypted`, `extraPools`,
   `neverSleep`, `datasets`.
5. **Its secrets.** Its TPM recipient (`secretsKey` in the report) in
   `.sops.yaml`, and `secrets/<box>.yaml` with at least `wifi-psk`
   (`host.nix` asks for it), encrypted with `dd secret run`.
6. **Its roles.** Add them to its list. `nix build
   .#checks.x86_64-linux.role-fit` and `.#checks.x86_64-linux.box-reports`
   say what it cannot carry. A storage box gets a `garage` block; its `id`
   comes after garage first starts there.
7. **A release.** The box is built, and asked to vouch, from then on
   ([releases.md](releases.md#changing-it)). Then `dd-unlock-enrol share`
   on the box hands the unlock Worker its half of the disk key
   ([disk-unlock.nix](../../nix/modules/box/disk-unlock.nix)).

To reinstall a box that is already in the list, keeping its identity, use
`DD_LAN=<its LAN address> scripts/install-box <box> [branch]`
([scripts/install-box](../../scripts/install-box)). It builds from git,
takes the box's ssh host key, headscale and garage state, verifier state,
certificates, secrets key and vote key, checks the host key against
`boxes.json` before wiping anything, installs with nixos-anywhere, and
checks the result: same host key, root on ZFS, the built system, healthy
pools, the same `net`, no failed units.

## Changing it

- **A role for a box**: add it to the box's `roles`, run role-fit, and
  read what it says. A role that needs another on the same box says so in
  `with`; adding the role means adding both.
- **A new role**: `nix/roles/<role>.nix`, an entry in `roles.json` with
  its `why` and what it `requires`, then a box's list
  ([servers.md](servers.md#adding-a-service)).
- **A new requirement kind** goes in `need` in `fit.nix`, read from
  `hardware.json`. If the report does not have the fact yet, `detect.sh`
  gets it first, and every box has to report again before the check can
  pass.
- **A new box.json field** is read in `host.nix`. Read it with a default
  (`box.x or ...`) unless every box's file gets it in the same change.
- **A new person** in `people` needs `ssh` and `paperKey` before a box
  names them as owner: `core.nix` reads both for every box they own.
- **The domain** changes in `policy.json`, and also in the places that
  still say `commonty.org` by themselves: the cli's defaults
  ([client.md](client.md#pitfalls)), `dd mail setup` and `dd control
  setup`'s `--domain`.

## Pitfalls

- **A report you edit is refused.** Fix the cause on the box (or in
  `detect.sh`), reboot or restart `dd-box-report`, and copy it again.
- **`net` changes when a box joins as a new node.** Update `boxes.json`
  in the same change as anything else that follows from it; a stale
  `net` makes the box's peers and the agent's reach check wrong.
- **The first storage box is special.** A box without the storage role
  takes the nix cache and its backups from the first storage box in the
  list, over the fleet's network (`cacheOf`, `backupEndpoint` in the
  flake). Reordering `boxes.json` can move that.
- **Opaque ids stay opaque.** Names of places and people beyond
  `policy.json` go in `secrets/fleet.yaml`, not here.
- **`servers/fleet/` is a different folder.** It holds Settings' pages.
