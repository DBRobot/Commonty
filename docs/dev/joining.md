# A box joining the fleet

Status: a box joins **as a stranger** (built) and builds its own system
from its own report (built). **As a member's box**, set up from the app's
installer window and a USB drive, is being built (below).

A box the release does not list needs no one to add it. It joins the
control plane's agreed state on its own word, and joining gives it nothing
anyone needs to trust it with.

## As a stranger

- **Its name comes from its key.** `dd-control -join-key` takes the box's
  TPM-held key and joins under `j` and ten hex of the key's hash
  ([`state/join.go`](../../servers/system/control/state/join.go)). No
  other key can take that name, a box of the release's can't join a second
  time, and the relay refuses a hello for that address signed by any other
  key.
- **It holds no lease**, so it runs no service. The plan only gives it
  **copies**, where its store has room. A bucket is always kept where its
  service runs, on a box of the release, and a copy goes first to a box
  able to take the service over: a stranger's copy is one more place the
  ciphertext is, never the only one. A stranger saying it keeps a copy
  never pushes out a box of the release.
- **The network**: the gate hands it a key tagged `tag:joined`. A joined box
  reaches the other boxes' stores on port 3443 and nothing else. The
  boxes' gates, services and pages, all on 443, are out of its reach.
- **Its address** on the network is the one headscale gave it, said into
  the agreed state by the box running headscale (`netaddr`), never by the
  joined box itself. The DNS Worker points `*.<its name>.<domain>` there,
  and only at an address on the fleet's network.
- **Its store** gets a key at each other box's store, sealed to the age
  recipient it joined with (`dd-store-joined`). When it is gone from the
  agreed state, so is its key; what it copied there stays.
- **Bounds:** at most 32 strangers at once, 4 joins an hour across the
  fleet, 8 joining at once at the relay. One unheard for 30 days is
  dropped from the agreed state.

## What a stranger can do

Hold ciphertext it can't read, and fail to keep it: the box writing a
bucket reads a copy back, and a copy found missing counts for nothing.
Waste a copy slot that a box with more room would have had.

What it can't do: run anything, see any plaintext, reach any service, take
another box's name, or change what the plan does with the release's boxes.

## As a member's box (being built)

A member sets the box up from the app, signed in. Three parts, and what
passes between them:

```
app ("Set up a box" window)        USB drive (FAT partition, commonty/)
  writes the box installer   ───▶  ticket       a join ticket (biscuit)
  mints a join ticket              wifi.json    {ssid, psk} or absent
  makes a backup password          backup.key   the pending repo's password
  copies this computer's Wi-Fi     version      the installer's release

box installer (full screen)        fleet
  BIOS: check, set what it can     gate: PUT /_dd/images/pending/<ticket id>/...
  network: wired, else wifi.json         with the ticket alone, 7 days
  backup: the old disk, encrypted  control plane: JoinReq{..., Ticket}
  install: TPM-sealed disk               Joined.Owner = the ticket's member
  join: with the ticket            app: on join, the pending backup moves
                                        into the member's images
```

- **The join ticket** is a device biscuit the member's device mints
  (`client/auth` `mint_for`, purpose `join`): `user(<member>)`, `ticket(<16
  random bytes, hex>)`, and a check on time, 7 days. It is good for one
  join and the uploads of that one backup. The control plane checks it
  against the member's signed entry in the directory, and records the
  ticket as used once a box joins with it.
- **The pending backup** is a restic repository of its own, at
  `images-<member>/pending/<ticket id>/`, under a password made for it
  alone: the drive never holds what opens the member's other images. The
  app seals that password to the member's devices, beside the ticket, so
  any of them can open the backup. The gate takes writes there on the
  ticket and nowhere else, and drops the repository 7 days on unless a box
  joined with its ticket.
- **On join** the app, on any of the member's devices, copies the pending
  backup into the member's images (`restic copy`) and deletes the pending
  repository.
- **The box** joins as `Joined{Owner: <member>}`: on the member's Devices
  tab, where the member can remove it, and the member sees its metrics.
- **The disk** is encrypted with a key sealed to the box's TPM (PCR 7):
  a pulled disk reads as nothing, and the box boots on its own.
- **BIOS**: the installer checks UEFI, TPM 2.0, Secure Boot (off), AHCI and
  virtualisation, and sets what the firmware lets it through
  `/sys/class/firmware-attributes` (Dell, Lenovo, HP, with no BIOS
  password), restarting itself; the rest it shows, where to change.
