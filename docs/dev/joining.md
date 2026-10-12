# A box joining the fleet

Status: a box joins **as a stranger** (built) and builds its own system
from its own report (built). **As a member's box**, set up from the app's
installer window and a USB drive, is being built (below): the fleet's side
(the ticket, whose a box is, the pending backup, Your boxes) is built; the
app's window and the drive's installer are next.

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
  backup: the old disk, encrypted  control plane: JoinReq{..., Ticket},
  install: TPM-sealed disk               carried as Joined.Ticket
  join: with the ticket            gates: whose each joined box is
    (/var/lib/dd/join-ticket)      app: on join, the pending backup moves
                                        into the member's images
```

- **The join ticket** is a device biscuit the member's device mints
  (`auth::device::mint_join_ticket`): `user(<member>)`, `ticket(<16 random
  bytes, hex>)`, `purpose("join")`, and checks that it is for `join` alone
  and good for 7 days. Like any purpose-built token it opens nothing else:
  not the member's files, pages or directory entry.
- **Whose a box is, is the gates'**, as the directory is. The box's
  dd-control sends the ticket with its join (`-join-ticket-file`); the
  validators carry it in `Joined.Ticket` and judge it no more than they
  judge the directory (they refuse only one too big, or one that came with
  another box already). Every gate checks it the same way
  ([`member_boxes.rs`](../../servers/gate/src/member_boxes.rs)): signed by
  a device in the member's entry, good at the agreed time the box joined,
  the first box to come with that ticket id, and not one the member has
  removed. So every box agrees whose it is, and none vouches for it alone.
  A ticket no gate takes leaves the box a stranger, and a member's box can
  do no more on the fleet than a stranger's.
- **The pending backup** is a restic repository of its own, at
  `images-<member>/pending/<ticket id>/`, under a password made for it
  alone: the drive never holds what opens the member's other images. The
  app seals that password to the member's devices, beside the ticket, so
  any of them can open the backup. The gate takes the ticket there and
  nowhere else (GET, HEAD, PROPFIND, PUT, MKCOL; never a delete), counted
  against the member's allowance, and an hour's check drops the folder 7
  days after its first write unless a box joined with its ticket. With no
  read of the agreed state it drops nothing.
- **On join** the app, on any of the member's devices, copies the pending
  backup into the member's images (`restic copy`) and deletes the pending
  repository.
- **The box** is on the member's Devices tab ("Your boxes",
  `GET /_dd/boxes/mine`), with when it last said how it is. Removing it
  puts its ticket id in the member's signed entry (`removed_boxes`), and on
  every box at once it is nobody's.
- **The disk** is encrypted with a key sealed to the box's TPM (PCR 7):
  a pulled disk reads as nothing, and the box boots on its own.
- **BIOS**: the installer checks UEFI, TPM 2.0, Secure Boot (off), AHCI and
  virtualisation, and sets what the firmware lets it through
  `/sys/class/firmware-attributes` (Dell, Lenovo, HP, with no BIOS
  password), restarting itself; the rest it shows, where to change.
