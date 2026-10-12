# A box joining the fleet

Status: a box joins **as a stranger** (built) and builds its own system
from its own report (built). **As a member's box**, set up from the app's
installer window and a USB drive, is being built (below): the fleet's side
(the ticket, whose a box is, the pending backup, Your boxes) and the
drive's installer are built; the app's window is on its own branch.

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
  nowhere else (GET, HEAD, PROPFIND, PUT, MKCOL, and the pieces of an
  upload; never a delete), counted against the member's allowance, and an
  hour's check moves the folder to the images' trash 7 days after its
  first write unless a box joined with its ticket (below).
- **On join** the app, on any of the member's devices, reads which of its
  boxes joined and with which ticket (`GET /_dd/network/joined`), copies
  each one's pending backup into the member's images (`restic copy`) and
  deletes the pending folder with the member's own token (to the images'
  trash, as any delete there).
- **The box** is on the member's Devices tab ("Your boxes",
  `GET /_dd/boxes/mine`), with when it last said how it is, and on their
  Metrics page. Removing it
  puts its ticket id in the member's signed entry (`removed_boxes`), and on
  every box at once it is nobody's.
- **Its metrics** are its member's
  ([`observe/joined-metrics.nix`](../../nix/modules/observe/joined-metrics.nix)).
  It has no key to the metrics bucket the release boxes' sidecars upload
  to, so its prometheus writes to the observe box instead, over the store's
  port (`/_metrics/write`). There it has a token of its own, sealed to its
  secrets key and served beside its store keys (`/_keys/<box>-metrics.age`,
  `dd-metrics-joined`; a box joining under the same name with another
  recipient gets a new one). nginx names the box from the token and
  nothing else (`THANOS-TENANT`), and a Thanos receive keeps each box's
  series apart, labelled `box=<its name>`; any `box` label the box sends is
  dropped. Bounds: a megabyte a write, two writes a second, and while its
  series take more than a gigabyte (`maxBytes`) it is refused; 15 days
  kept; its token goes when it leaves the agreed state. A query of its own
  (`thanos-query-joined`) answers for one box at a time, the one nginx
  names. The Metrics page shows a member their own boxes, and the fleet's
  admin every joined box, as they see every release box; no other member
  sees them. The fleet's view (the bucket) never holds them.
- **Fails closed.** Whose a box is is worked out afresh on every request,
  from an agreed state read in the last five minutes and the entries as
  they are: a box removed, a member who left or is suspended, a device
  taken out of the entry each count at once, and with no fresh state no
  joined box is anyone's. The week's check drops a pending backup only on
  a fresh agreed state it read whole, whose time is within ten minutes of
  the box's own, keeps the backup of any box that came with its ticket
  whoever owns that box now, and moves it to the images' trash (30 days),
  never deletes it outright. A ticket at the images does only what the DAV
  layer would route inside its own folder (`dav::ticket_allows`): no
  delete, move or copy, whatever header comes with it.
- **The disk** is encrypted (the fleet's layout, `encrypted = true`) with
  a key from a half sealed in the box's TPM under PCR 7, as node1's and
  node2's are, but with no second half: the unlock Worker knows nothing of
  a joined box (`dd.unlock.tpmOnly`, box.json `"unlock": "tpm"`).
  - **Its own Secure Boot keys.** The installer needs Secure Boot in setup
    mode (the screen sets it where the firmware lets it, or says where).
    It makes the box's keys on its encrypted disk (`/var/lib/sbctl`), signs
    its boot with them (lanzaboote, which signs every release's boot after,
    the box's own builds included), and enrolls them, with Microsoft's
    beside them as node1 and node2 have: drivers on graphics and network
    cards are signed by Microsoft, and some machines start with no screen
    without them.
  - **Sealed at the first boot.** The installer's boot is setup mode's, so
    the half it makes is held under no PCR (`bootstrap.priv`). The box's
    first boot, with Secure Boot on and its keys, opens the pool with it
    once; `dd-unlock-seal` then seals a new half under that boot's PCR 7,
    re-keys the pool to it (`zfs change-key`) and removes the installer's.
    A stop midway leaves a mark the initrd follows. If Secure Boot is not
    on at that boot, it does not seal, and says so.
  - **The box's own key** (its name, its signatures) is made at that
    first boot too, after the seal, under the same PCR 7, as node1's and
    node2's: nothing that names or signs for the box exists before its
    signed boot does. It was installed under the name `joining`; its agent
    names it by the key (state.JoinName) and builds itself again under
    that name, and only then does it join (dd-control waits for the name).
    The installer restarts straight into that first boot.
  - **What it protects against:** a pulled disk; another system started on
    the box (a USB drive's: Secure Boot refuses it; one Microsoft's keys
    start, through a shim: a different PCR 7, no key); the box's own
    system with a changed command line (its boot is signed and the stub
    takes no other line under Secure Boot); Secure Boot turned off or its
    keys changed (a different PCR 7). A boot the TPM does not recognise gets
    no key at all, never one made from less, and has no shell.
  - **What it does not:** between the install and the first boot (minutes,
    with the person at the machine) the installer's half opens for anyone
    starting a system there; a copy taken then is worth nothing after the
    re-key.
  - **Decided** (David, 2026-10-12): the box enrolls keys of its own, over
    systemd-pcrlock binding PCR 4, which needs no firmware step but moves
    the pool's key into LUKS and predicts every release's boot.
- **Wi-Fi**: the network the box was set up on (the drive's `wifi.json`, or
  one picked on the screen) is sealed to the box's secrets key, which only
  its TPM opens (`/var/lib/dd/wifi.age`, box.json `house.sealedWifi`). The
  box opens it into memory at each boot, before NetworkManager starts
  (`dd-wifi-unseal`); it is never on its disk in the clear.

## The drive

The app's installer window writes `commonty-box-installer-<version>.img.zst`
(`installers.x86_64-linux.box`, [`image.nix`](../../nix/installer/image.nix);
the `box-installer` download). It is a GPT disk: partition 1 the FAT boot
partition (systemd-boot, the installer, and `commonty/`, which the app
fills), partition 2 the installer's system, grown to the drive at its
first boot. It starts to one screen, a browser filling it and nothing to
click away to ([`setup.nix`](../../nix/installer/setup.nix)). The screen's
steps are `dd-box-setup`'s
([`dd-box-setup.rs`](../../servers/system/release/src/bin/dd-box-setup.rs)),
on loopback, which runs the installer's own programs:

```
firmware   UEFI, TPM 2.0, Secure Boot in setup mode, disks seen (not behind RAID/RST),
           virtualisation. What the firmware lets Linux change (Dell, Lenovo,
           HP, no BIOS password) it changes and restarts; the rest it says
           where to change. Power-on after an outage can't be read: a tip.
network    a cable, else the drive's Wi-Fi, else a network picked here
disk       internal, not removable, nothing of it mounted
back up    optional: dd-box-backup (backup.sh)
install    dd-join-install (join-install.sh), with the ticket and the Wi-Fi
done       take the drive out, restart
```

- **The firmware table** is
  [`bios-settings.json`](../../servers/system/release/bios-settings.json):
  per kernel driver, the attribute and value for each setting, and where
  the names come from. A value is written only where the attribute lists
  it among its possible values and no BIOS password is set; a setting not
  in the table is never touched. The names are each maker's own; they have
  not been tried on every model.
- **The backup is file by file** (restic), not block by block: NTFS,
  ext4, XFS, btrfs, FAT and ZFS are mounted read-only and backed up; a
  BitLocker volume can't be read and is said so. Files come back as files
  to open, a backup holds what was used and no free space, and both a
  Windows disk and a Linux one read the same way. It goes to the member's
  pending folder through the gate, on the ticket (rclone's WebDAV, the
  ticket as its bearer token), encrypted on the machine with the drive's
  `backup.key`. A dropped connection is tried again, up to five times.
  The member's free space is not shown yet: there is no route for it on
  the ticket.

