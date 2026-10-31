# A box joining the fleet

Status: a box joins **as a stranger** (built) and builds its own system
from its own report (built). **As a member's box**, set up from the app's
installer window and a USB drive, is built: the ticket, the enlistment the
member's root signs, whose a box is, the access a box gives, the pending
backup, Your boxes. Proof that the box is real hardware (the TPM's
attestation, docs/dev/attestation.md) is on its own branch and not yet
asked for.

A box the release does not list needs no one to add it. It joins the
control plane's agreed state on its own word, and joining gives it nothing
anyone needs to trust it with.

## As a stranger

- **It meets the hardware minimum** ([Hardware](#hardware)), or it can't
  join at all: the validators refuse the join with what is missing, and
  the box says it in its log and in `/var/lib/dd-control/join-refused`.
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
  backup: the old disk, encrypted  control plane: JoinReq{..., Facts, Claim},
  install: TPM-sealed disk               its id kept as Joined.Ticket
  join: with the ticket's claim    gates: whose each joined box is
    (the ticket stays in           app: on join, the pending backup moves
     /var/lib/dd/join-ticket)
                                        into the member's images
```

- **The join ticket** is a device biscuit the member's device mints
  (`auth::device::mint_join_ticket`): `user(<member>)`, `ticket(<16 random
  bytes, hex>)`, `purpose("join")`, and checks that it is for `join` alone
  and good for 7 days. Like any purpose-built token it opens nothing else:
  not the member's files, pages or directory entry.
- **The ticket never leaves the box and the app.** It is a bearer token
  (it writes the member's pending backup), and everything a join carries
  is in the agreed state for every box to read. So the box's dd-control
  (`-join-ticket-file`) joins with the ticket's **claim**
  ([`ticket.go`](../../servers/system/control/state/ident/ticket.go)
  `MakeClaim`): its id, its signed authority block (what the member's
  device signed, without the proof key that makes the biscuit usable), and
  the box's own key signed with that proof key. The validators hold the
  claim to its block's id and to that signature (only a holder of the whole
  ticket can make it), refuse a second box with an id already joined, and
  keep the id (`Joined.Ticket`).
- **Whose a box is, is the agreed identity state's**: the join puts the
  box's key, the ticket's id and its signed block in the directory log
  (`joined`,
  [`state/join.go`](../../servers/system/control/state/join.go)). Then
  the member's root signs an `enlist` naming the box and the ticket's id,
  and the judge
  ([`state/ident`](../../servers/system/control/state/ident/judge.go))
  takes it only if the box's ticket is the member's: signed by a device in
  their entry, for them, not expired, never used. Once enlisted, the
  ticket is used for good. A box is one member's at a time. Every gate
  reads whose each box is from the agreed state's `enlisted`
  ([`member_boxes.rs`](../../servers/gate/src/member_boxes.rs)), with the
  ticket it was enlisted with; a ticket alone makes a box no one's. A box
  no member has enlisted stays a stranger.
- **Ticket to enlistment**: after the join, the app that wrote the drive
  (or any of the member's devices with Backups open) sees the box waiting
  (`GET /_dd/network/joined`, `enlisted: false`, from the identity view's
  `waiting`: the judge's word that the block is the member's) and, where the account's
  main key is on that device, signs the `enlist` and puts it to the door
  (`PUT /_dd/boxes/enlist`). An account whose main key is a passkey adds it
  on Settings > Devices ("waiting for you to add it", Add with passkey).
- **The pending backup** is a restic repository of its own, at
  `images-<member>/pending/<ticket id>/`, under a password made for it
  alone: the drive never holds what opens the member's other images. The
  app seals that password to the member's devices, beside the ticket, so
  any of them can open the backup. The gate takes the ticket there and
  nowhere else (GET, HEAD, PROPFIND, PUT, MKCOL, and the pieces of an
  upload; never a delete), counted against the member's allowance, from
  anyone signed up and not shut out (the backup is made before the box
  gives them access), and only until a box is enlisted with it: a used
  ticket opens nothing. The gate checks the whole biscuit, proof and all
  (`verify_ticket`); what the agreed state holds of it (the id, the signed
  block) opens nothing, nor does a biscuit rebuilt around the block. An hour's check moves the folder to the images'
  trash 7 days after its first write unless a box was enlisted with its
  ticket (the agreed state's `enlisted`, never the joined box's word).
- **On join** the app, on any of the member's devices, reads which of its
  boxes joined and with which ticket (`GET /_dd/network/joined`), copies
  each one's pending backup into the member's images (`restic copy`) and
  deletes the pending folder with the member's own token (to the images'
  trash, as any delete there).
- **The box** is on the member's Devices tab ("Your boxes",
  `GET /_dd/boxes/mine`), with when it last said how it is, and on their
  Metrics page. Removing it is the root's `unenlist`
  (`PUT /_dd/boxes/unenlist`, signed by the passkey on the tab, or by the
  app where the main key is a device key). An entry's `removed_boxes`, from
  before, is still read and signed as it is, and decides nothing.
- **The door's PUTs** (`/_dd/boxes/enlist`, `unenlist`, `extend`) are the
  directory's, step for step: the one per-visitor limiter first, a body of
  64 KiB at most, the kind the path names, signed in, the item theirs (an
  enlistment's `member` is the signed-in member's root; an extension is
  the admin's), its signature checked against the root the agreed identity
  state has for them now, the name's and key's allowances counted only
  after that, duplicates by the shared item hash, and 503 while the
  identity state is not read. The judge decides; the door only refuses
  what can't be theirs.
- **Its metrics** are its member's
  ([`observe/joined-metrics.nix`](../../nix/modules/observe/joined-metrics.nix)).
  It has no key to the metrics bucket the release boxes' sidecars upload
  to, so its prometheus writes to whichever box the plan gives
  joined-metrics instead, at `metrics.<domain>` over the store's port
  (`/_metrics/write`); the name follows that box. There it has a token of its own, sealed to its
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

## Access

Signing up gives nothing. The fleet's services open to a member by an
**invite** (the release's member list, or an invite the release key
signed: both David's) or by a **box** of theirs that counts. The judge's
view says which (`access`: `invite`, `box` or `none`,
[`state/ident/access.go`](../../servers/system/control/state/ident/access.go)),
and the gates open the services by it alone, from the agreed state, and
to nobody while it is not read (a gate in `agreed` mode; one still
replaying the directory serves its replay, which knows invites only).

- **Everywhere, as an invite does**: every check of who is a member
  counts a box's member as an invited one, on a gate in `agreed` mode: the
  services' doors, the directory's reads, a house's Wi-Fi on a
  directory-only box, a sign-up let straight into the directory. The
  remote computers judge replays the log item by item, so each item of a
  computer's carries the joined boxes as the agreed state had them when it
  was ordered (`DirItem.Boxes`, the facts access reads and no other); an
  item from before carries none, and no box counts for it. Whether a
  computer runs, and its lease, are read with the boxes as they are now.
  A home controller holds someone its release's list does not name to the
  gate's word (`/_dd/controller/standing/<name>`, asked at sign-in, at
  each refresh, and at the end of pairing). A client (`dd git signers`,
  `dd policy announce`, a shared library, a device's first look at its
  own entry) holds them to every directory it asks agreeing on their root
  and saying the agreed state gives it the services
  (`/_dd/directory/<name>/standing`, `directory::resolve_member`): the
  boxes' word, where the release's list is the release key's. Only a gate
  in `agreed` mode answers either standing; one replaying, or in shadow,
  says nothing, and the list alone decides as before.
- **What qualifies**: the [Hardware](#hardware) section's qualifying
  level. Wired or Wi-Fi alike.
- **Many boxes**: access stands while any qualifying box of theirs is
  enlisted and not gone 30 days. Removing one of several costs nothing
  while another counts.
- **Losing it**: only by removing every box, or every box gone 30 days
  (unheard by the agreed state) while healthy.
- **Our fault, for 90 days**: a box whose last word was that its agent's
  last run did not end well (`agent_ok` 0: the release was refused or
  rolled back on it; held, none and absent say nothing) on a release
  older than the newest the release's own boxes run is not gone for 90
  days from its last word (`OurFaultFor`), and the state machine keeps
  its place that long rather than forgetting it after 30 days as it does
  a stranger. Its word alone is not enough: with no newer release out,
  nothing of ours failed on it. And since that word (its release and how
  it went) is the box's own, it has an end: past 90 days the box is gone
  like any other, and only the admin's extension keeps the member's
  access, for a release of ours that really kept a box down. That is
  the signal the agreed state has; there is no fleet-wide "bad release"
  mark in it, so a box that dies on a release without saying so first
  counts as gone after 30 days like any other. The box's word itself is unattested
  until TPM attestation is wired: a box that lies about its release is
  not caught by this, only bounded by the 90 days.
- **Grace**: 30 days. Only the admin (the release's `identity.admin`,
  fleet/policy.json `owner`) extends it for a member, with a root-signed
  `extend` naming them and an `until`. It keeps their access while they
  still have a box enlisted, gone however long; it brings back nobody who
  removed their boxes.
- **A ban is a ban**: a member left, suspended or removed is `none`
  whatever box or invite they hold, and enlists nothing; only David taking
  them off the release's `revoked` lets them back.
- **A home controller**: any member with access sets one up, however they
  came in (an invite or a box); a ban shuts it as it shuts the rest.
- **Anyone signed up and not shut out** may keep a pending backup and see
  and remove their boxes, whatever their access: that is how a box comes
  to give them access.
- **Placement**: an enlisted box of a member with access is theirs
  (`place.Owners`, from the agreed state alone, read alike by every box and
  the witness). A copy it holds is kept as one on a box of the release is,
  and a stranger's word never pushes it out. Once joined boxes are attested
  (`attested`, off today) it may run what reads nobody's data (`joined`
  `services`) and its owner's own (`own`, trust `own`), one for them on their
  boxes, never the instance the fleet shares and never a fleet name; a box
  the fleet's owner enlisted is theirs like a box of the release, leases
  aside. A stranger runs nothing. A ban, leaving or an unenlist takes it all
  out of the next plan.
- **What it is built with**: its dd-control writes whose it is, as the agreed
  state says, to `/var/lib/dd/box/enlisted.json`, and its agent's next run
  builds from that (`nix/lib/joined-own.nix`, never box.json's own word), so a
  gain arrives within a run or two. The file decides only what is built: the
  plan reads the agreed state, so a box lying in it builds more and is given
  no more, and a loss needs no rebuild to take effect. It gives nobody a login
  (`dd.box.owner` stays empty on a joined box).
- **Which house**: boxes of one owner set up on one Wi-Fi say one house (a
  hash of its name, in the agreed state); a box set up by cable names none and
  is a house of its own.

## Hardware

Two levels, all of the numbers in one place
([`state/ident/hardware.go`](../../servers/system/control/state/ident/hardware.go)),
for David to tune:

```
MINIMUM (ident.Minimum)       below it the join is refused, with what is missing
  TPM 2.0, UEFI, firmware that can do Secure Boot, x86_64 or aarch64
  MinThreads     4            logical threads
  MinMemTotal    7 GiB        MemTotal ("8 GiB")
  MinSystemDisk  128 GB       the disk the system is on (1e9 bytes a GB)
  a network link              one NIC, wired or Wi-Fi

QUALIFYING (ident.Qualify)    the minimum and any one of
  compute  ComputeThreads   8        threads, and
           ComputeMemTotal  14.5 GiB MemTotal ("16 GiB"); never a Raspberry Pi
  storage  StorageBytes     4 TB     usable store (4e12 bytes)
  gpu      GPUVRAM          8 GiB    VRAM on an nvidia or amdgpu card,
           or an encoder the transcoder can use (H.264 through VAAPI);
           integrated graphics count only by their encoder
```

- **Why these.** The minimum is what a box needs to build and run its own
  system: boxes don't share builds, rustc peaks near 1 GB a job, and the
  closure is 5 to 11 GB, with generations and its share of the store
  beside it. Compute is what hosting a service or a member's computer
  needs: 8 threads and 16 GiB (a member's computers may take 32 GiB
  together). Storage is 4 TB because copies are kept twice.
- **A box at the minimum and nothing more** ("just enough to run") joins as
  a stranger and holds copies, but gives its member no access. A weak CPU
  with 4 TB of store qualifies, and so does one with a GPU.
- **Read from the box's own signed report** as its heartbeat carries it
  (dd-control `reportFacts`, from `/var/lib/dd/report/hardware.json`,
  [`detect.sh`](../../nix/modules/box/report/detect.sh)): `tpm_major`,
  `uefi`, `secure_boot_capable` (its firmware has the SecureBoot
  variable), `threads` (`cpu.threads`), `mem_total_bytes` (`memoryBytes`,
  MemTotal), `system_disk_bytes` (the boot disk's size), `nics`,
  `gpu_vram_bytes` (the largest nvidia or amdgpu card's `vram`: amdgpu's
  sysfs, nvidia-smi), `video_encoders` (render nodes that encode H.264),
  `raspberry_pi` (its device tree); `arch_x86_64` or `arch_aarch64` from
  the agent itself; `store_size_bytes` from its store. A join carries the
  same facts, and the validators hold them to the minimum.
- **The box's word**: until TPM attestation is asked for, all of it is
  what the box says of itself; a box that lies about its hardware is not
  caught by this.

## The drive

The app's installer window writes `commonty-box-installer-<version>.img.zst`
(`installers.x86_64-linux.box`, [`image.nix`](../../nix/installer/image.nix);
the `box-installer` download). It is a GPT disk: partition 1 the FAT boot
partition (systemd-boot, the installer, and `commonty/`, which the app
fills), partition 2 the installer's system, grown to the drive at its
first boot. Two boxes build it and must make the same bytes (dd-attest),
so partition 2's ext4 is made by `image.nix` itself, not by
systemd-repart: repart sized it by asking the builder's file system where
a trial image's holes are (ZFS answers per 128K record, ext4 per block)
and copied the store with the builder's hard links, xattrs and reflinks.
`image.nix` copies plainly, sizes by the files' own lengths, and mkfs adds
them sorted, with a fixed UUID and hash seed and every time
SOURCE_DATE_EPOCH; repart only places it (`CopyBlocks=`). CI builds it on
each runner box and once more with `--rebuild` (`build_installer`). It starts to one screen, a browser filling it and nothing to
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

## How it is tested

Two VM tests, split where the box first starts:

- **`join-install`**: the installer, through its screen's API: the
  refusals (a disk in use, two unnamed, an old or forged release, a page
  that is not the screen's), the backup to a stand-in gate on the ticket
  alone, the encrypted install, its own Secure Boot keys enrolled; then
  the installed disk's first boot with Secure Boot on, which seals the
  disk's key and makes the box's key under PCR 7, a boot after it that
  opens the disk with nobody there, and a changed PCR 7 that gets no key
  and no signature. The system installed is a stand-in with the boot chain
  of a joined box (lanzaboote, the disk unlock), not the whole of one.
- **`joined-box`**: the real joined box (`lib.joinedBoxModules`), booted
  under its install name with a test key in its TPM (so its name is known
  to build for): its agent names it by the key, takes the signed commit,
  builds and switches to that name's system; dd-control, held until then,
  is refused below the hardware minimum (the VM is small, and says what
  it lacks), then, its report written to meet it and to qualify (a TPM
  2.0, UEFI with Secure Boot, 4 threads, 8 GiB, a 256 GB system disk, a
  GPU with 8 GiB of VRAM), joins a control plane of three validators and
  the witness with the member's ticket's claim; the agreed state holds the
  ticket's id and never the ticket. Signing up opened nothing (`/verify`
  403) and the box waits for her;
  an enlist naming another ticket is refused; her root's enlist on its
  ticket, through her gate's PUT, makes it hers (`/_dd/boxes/mine`) and
  opens the services (`access: box`, `/verify` 200): signed in, every
  service's door (Files, Mail, Photos, Passwords, the forge, the games),
  the directory, a key to the network, a friend link for a guest, and a
  remote computer the judge takes on the box state its item carried, all
  on her gate in agreed mode; her unenlist makes it nobody's and shuts
  each of them again, and the used ticket enlists nothing.

Not covered by either: a real machine's firmware (the vendor table, Setup
Mode set from Linux, an unsigned system refused by Secure Boot, which stops
before anything could be checked), a build of the box's system from
source inside the VM (the agent's build there is the release's flake
handing back one built ahead), and the app's side (its own branch).

