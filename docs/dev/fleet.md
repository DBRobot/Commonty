# The fleet of boxes

`fleet/` is what the fleet is, as data: the boxes, what each box reported
about its own hardware, what its owner chose for it, who the members are,
and the owner's policy. Nothing in it says what a box runs: a box is built
with what its capability allows
([`nix/lib/fleet-placement.nix`](../../nix/lib/fleet-placement.nix)), and
the control plane decides where each service runs.
[`flake.nix`](../../flake.nix) reads these files and builds one NixOS
system per box from them. This page is how the folder is laid out,
who reads each file, and how to change it.

The folder holds no secret. Every file here is public by design: keys are
public halves, ids are opaque, and the private side (who owns which site,
the cache writer's key) is in `secrets/fleet.yaml`, encrypted with sops.

## Where things are

```
fleet/
  boxes.json              every box: its addresses, its public keys
  boxes/<box>/
    hardware.json         what the box found on itself (signed by its TPM)
    hardware-configuration.nix   nixos-generate-config's output (signed with it)
    report.sig            the TPM's signature over the two files above
    box.json              what the box's owner chose: house, pools, datasets
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

One thing here looks like trust and is not:

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
  garage      on a box built with garage (the storage bundle):
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
flake.nix                nixosConfigurations: one per entry, the bundles its
                         capability allows (fleet-placement.nix carried), and
                         the wiring derived from the list: peers, house peers,
                         garage's peers and zone, where the cache and backups go,
                         the agent's reach list, headscale's owners and box keys
flake.nix (control)      the control plane's boxes: ids 101.., house, key
flake.nix (mail-worker)  BOXES in the Worker's fleet.js: each unlockKey
nix/roles/core.nix       the other boxes' net addresses, and this house's
nix/modules/storage/view.nix   what each box is built with, for who may count what
nix/tests/boxes.nix      checks.boxes: the list against the built systems
nix/lib/fleet-placement.nix    what each box is built with, carries and can run (below)
box-reports/check.py     checks.box-reports: report.sig against unlockKey
entry_point.yml          CI's hosts (every box) and builders (built with the
                         CI runner: the flake's fleetBuilt)
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
  usbNic          the wired link is USB          usbcore.autosuspend=-1
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
first GPU that encodes H.264.

**The owner's choices**: `box.json`. It is written by hand, and it is the
only file in the folder that is.

```
box.json
  owner           a name in policy.json people: whose box this is. Picks
                  the paper key its disk keys are copied to, and the
                  owner checks in capability.nix, headscale and build orders
  house
    ssid          the house's Wi-Fi                     dd.home.ssid
    address       the box's fixed address on the house LAN, with prefix
    household     who lives there (switches its ad blocking); makes it the
                  house's own box, which the house's DNS and Tang ask for
                  (needs requires.place = "house")
  encrypted       datasets under <pool>/enc, unlocked at boot from the TPM
                  and the unlock Worker (disk-unlock.nix)
  extraPools      pools other than tank to import at boot
  pools.bulk      the pool for big, slow data (default tank)
  datasets        dataset -> ZFS properties plus mountpoint; "note" is for
                  people and dropped (dd.zfs.datasets)
  arcMaxBytes     the ARC's cap (zfs_arc_max)
  gamesMemoryMiB  memory all game servers may have together
  neverSleep      no suspend, no lid or power key action
```

`owner`, `house`, `encrypted`, `extraPools`, `neverSleep` and `datasets`
are read without defaults: a box.json without one of them fails to
evaluate. The others are optional.

## What a box is built with

No list says it. [`fleet-placement.nix`](../../nix/lib/fleet-placement.nix)
works it out for each box from its signed report, its `box.json` and the
policy, by the services' needs (`.#needs`,
[nix/lib/needs.nix](../../nix/lib/needs.nix)) and
[`capability.nix`](../../nix/lib/capability.nix):

```
carried    the module bundles it is built with (nix/roles/<bundle>.nix):
           core, and every bundle of which it can run a service that does
           not run everywhere, whose secrets its own sops file holds, and
           whose companions (with) it is built with too
built      every service those bundles bring
carries    of those, the placed services it can run: the plan gives it
           only these (box/placed.nix)
joined     the same for a box that joined on its own key, from its own
           report and box.json, limited to what a member's box may run
           (placement-inputs joined)
```

[`nix/roles/bundles.json`](../../nix/roles/bundles.json) says what each
bundle brings:

```
<bundle>
  services    the needs' service names it brings
  secrets     what its modules read from the box's own sops file whatever
              else it has; a box whose file lacks one is not built with it
  with        bundles that must be built beside it: the code reaches them
              on localhost
  why         one line on why, for people
```

What a box can run is capability's: its `joins` (a TPM 2.0 with P-256,
UEFI, a wired NIC its initrd has a driver for), each service's
`requires` against its report, and its owner's choices: `trust`, a
metered line. `checks.placement` and
`checks.placement-fleet` hold the rules to the fleet and to a dozen
made-up computers.

### Placement

The control plane decides where each placed service runs, from the
release's placement inputs and the agreed state
([control-plane.md](control-plane.md#placement)). A box's own word only
narrows what the release's table says it can run.

## policy.json

The fleet owner's choices. One file, read at evaluation.

```
owner          the fleet owner's name (a key in people)
domain         the fleet's domain                    dd.domain (core.nix)
repo           owner/name of the fleet's repository on its forge
                                                     dd.repo: the agent's
                                                     release url, the demo's
                                                     Code tile
people.<name>
  ssh          their ssh public keys: the admin login on the boxes they own
  paperKey     the ssh-ed25519 public half of their paper recovery key: each
               of their boxes' disk keys gets a copy encrypted to it in
               /boot/dd (dd.unlock.recovery), opened with dd disk recover
allowanceGb    each member's storage across every service and box
                                                     dd.storage.budgetGb
                                                     (storage/view.nix)
```

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
                     Worker is; the gateway bundle reads it
devices.json         systems the fleet builds and signs that run on a
                     household's device, not a box: today home-controller
                     (home-controller.md). CI builds each like a box's
                     system, dd release publish signs it beside them
tang.json            {<box>: {url, thp}}: each house box's Tang and its key's
                     thumbprint (`tang-show-keys 7654` on the box), built
                     into the home controller's image, which takes no other
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
dd member add|remove <name or id>    members.json (a name's id confirmed, or --confirm <id>)
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
checks.box-reports    each report.sig, every folder against boxes.json
checks.boxes          no box names a role, every bundle a box is built with is
                      a file, every box has core and builds, and the derived
                      wiring says what it is built with: peers,
                      dd.box.net, garage's address, zone and peers, Thanos on
                      its own box, the admin's console password, llama-swap
                      shut in
checks.ci             the VM test lists in flake.nix and entry_point.yml agree
```

A change under `fleet/` turns on CI's nix and VM parts
([ci.md](ci.md#how-a-run-fits-together)); every box's system is built,
on every box built with the CI runner.

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
   opaque ids with their names in `secrets/fleet.yaml`) and
   `public: false`. `net` once it has joined the fleet's network
   (`/run/commonty-net-address` on the box).
4. **Its folder.** `scripts/box-report <box> <address>` copies the signed
   report into `fleet/boxes/<box>/`. Write its `box.json` by hand: owner
   (a name in `policy.json` people), house, `encrypted`, `extraPools`,
   `neverSleep`, `datasets`.
5. **Its secrets.** Its TPM recipient (`secretsKey` in the report) in
   `.sops.yaml`, and `secrets/<box>.yaml` with at least `wifi-psk`
   (`host.nix` asks for it), encrypted with `dd secret run`.
6. **What it is built with** follows from its report, its `box.json` and
   its secrets file; nothing is added by hand. `nix build
   .#checks.x86_64-linux.placement` says what it can and cannot run, and
   why. A box built with garage gets a `garage` block; its `id` comes
   after garage first starts there.
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

- **A service on a box** is never put there by hand. If a box should run
  something it does not, `checks.placement` says why it cannot (its
  report, its `box.json`, its secrets file, the policy); fix that, not a
  list.
- **A new bundle**: `nix/roles/<bundle>.nix` and an entry in
  [`nix/roles/bundles.json`](../../nix/roles/bundles.json) with its
  `services`, `secrets`, `with` and `why`; its services' needs say what
  they need of a box ([servers.md](servers.md#adding-a-service)).
- **A new requirement kind** goes in the needs' `requires`
  ([`needs-type.nix`](../../nix/lib/needs-type.nix)) and `lacksIn` in
  [`capability.nix`](../../nix/lib/capability.nix), read from
  `hardware.json`. If the report does not have the fact yet, `detect.sh`
  gets it first, and every box has to report again before a box is
  judged by it.
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
- **The first storage box is special.** A box not built with garage
  takes the nix cache and its backups from the first box built with it in
  the list, over the fleet's network (`cacheOf`, `backupEndpoint` in the
  flake). Reordering `boxes.json` can move that.
- **Opaque ids stay opaque.** Names of places and people beyond
  `policy.json` go in `secrets/fleet.yaml`, not here.
- **`servers/fleet/` is a different folder.** It holds Settings' pages.
