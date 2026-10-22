# Home controller

A household's smart home runs on a Raspberry Pi in their house, the home
controller: Home Assistant behind a door of ours, on NixOS built from this
repository. The boxes carry its traffic sealed, keep its backups locked,
and serve it signed releases. No box can read anything about the home, sign
in to it, or change what it runs. Members see it as **Smart home**.

## The rules

- **The controller is a device, not a box.** It is the household's, like a
  phone. No box holds a key to it, and nothing reaches it but the door.
- **The home stays on the Pi.** Devices, automations and history are Home
  Assistant's state on the Pi's own locked card. A box gets only backups
  the Pi locked first.
- **One system for every house.** The release carries one
  `home-controller` system, the same in every house. Nothing about any home
  is in the repository or a release.
- **The door decides, from keys the controller saved.** Who may open it is
  decided on the Pi, from the root keys of the people it was paired with or
  let in. A box cannot add anyone.
- **Generic.** Any device Home Assistant supports works: Zigbee, Thread,
  Matter, Wi-Fi.

## Where things are

```
servers/smarthome/
  door/            the door (crate smarthome-door): TLS, sign-in, people,
                   the proxy to Home Assistant, pairing, backups, CT watch
  web/             the door's own pages: sign-in, pairing, people
  theme/           Home Assistant's Commonty theme
client/hubwriter/  dd-hub-writer: writes an image to a removable disk
nix/modules/smarthome/
  home-assistant.nix   Home Assistant on loopback, trusted networks, theme
  door.nix             the door's unit
  tang.nix             Tang on the house box (a box module)
nix/controller/
  system.nix       the controller's NixOS system (aarch64)
  image.nix        the card image: /boot, seed, room for the locked partition
  first-boot.nix   the installer that locks the card and installs the seed
```

## Setting one up

### Writing the card

The desktop app's installer, a window of its own (Settings, Network, "Set up
a home controller"; or the downloads page):

```
1 choose     removable disks only, size and model shown; never a fixed disk
2 write      fetch app.json (signed, kind "app"; the card is the download
             commonty-home-controller-<version>.img.zst), check it with the
             release key the app carries, download the image, check its
             sha256, then dd-hub-writer (elevated) unmounts, writes, reads back
3 key        32 random bytes, written as /boot/commonty/pair after the write
             (the FAT partition), kept by the installer
4 start      card into the Pi, cable to the router, power; the installer finds
             it at http://home-controller.local:8080 (mDNS) on the house network
5 pair       below
```

The installer is [`app/src/installer.rs`](../../app/src/installer.rs) and
its page `app/web/installer.html` (capability `app/capabilities/installer.json`:
no commands, only the launch-secret routes `/_dd/app/installer/...`). The
release key and the repository whose releases branch holds `app.json` are
built into the app (`app/build.rs`, from `fleet/release.pub` and
`fleet/policy.json`, or `COMMONTY_RELEASE_KEY` / `COMMONTY_REPO` in a nix
build). The image is kept in the app's data folder (`cache/controller/`)
and used again while the manifest names the same sha256. The pairing key
is the 32 raw bytes, in the installer's memory until pairing has used
them, never on this computer's disk. On Linux the boot partition is found
in `/proc/self/mountinfo`, else mounted with `udisksctl mount`; on macOS
`diskutil mount`; on Windows the letter Windows gives it.

`dd-hub-writer` ([`client/hubwriter`](../../client/hubwriter)) is the only
part that runs with admin rights, and the app carries it beside itself
(a bundle resource, built in `nix/app-release/linux.nix` and
`windows.nix`). Elevation is the platform's own prompt, asked by the app
([`app/src/writer.rs`](../../app/src/writer.rs)): `pkexec` on Linux, its
progress on a pipe (from an AppImage, a copy in a folder only the user can
open, since root cannot read the user's FUSE mount); `osascript` "with
administrator privileges" on macOS and PowerShell's `Start-Process -Verb
RunAs` on Windows, whose elevated program gets no pipes, so its lines go
to a file the app reads as they come. The writer has no manifest asking
for admin rights: `list` needs none, and one would prompt for it.

```
dd-hub-writer list
  stdout: [{id, name, model, bus, size_bytes, removable, mounted: [..]}]
  removable disks only (USB, SD, or the OS's removable flag), never the
  disk holding the running system; needs no admin rights
dd-hub-writer write <image> <device-id> <sha256-hex> [--progress]
  1 refuse a disk that is not listed, not removable, the system's, or
    smaller than the image
  2 hash the whole image file (raw, or zstd by its magic) and refuse a
    mismatch; nothing is opened for writing before this
  3 unmount every partition, open the disk exclusively, write in aligned
    4 MiB chunks, sync, read back and compare the sha256 with the
    decompressed stream's
  4 have the OS read the new partition table, so the app can write
    /boot/commonty/pair
  --progress: JSON lines {phase: "hash"|"write"|"verify", done, total,
  bytes_per_sec} on stdout, at most four a second plus the one that ends
  each phase
  exit 0 written, 2 refused (nothing written), 3 write or verify failed,
  1 bad arguments or an unreadable image; the reason on stderr
```

Its disk handling is ported from USBImager (MIT) and Raspberry Pi Imager
(Apache-2.0): `/sys/block`, `/proc/self/mountinfo` and `umount2` on Linux,
written through `O_DIRECT | O_EXCL`; `diskutil list/info -plist`,
`unmountDisk` and the raw `/dev/rdiskN` on macOS; on Windows
`IOCTL_STORAGE_QUERY_PROPERTY` for the bus, `FSCTL_LOCK_VOLUME` and
`FSCTL_DISMOUNT_VOLUME` on each volume of the disk, then
`IOCTL_DISK_DELETE_DRIVE_LAYOUT` before writing `\\.\PhysicalDriveN`
unbuffered. Its tests write to a plain file; a real disk on Linux, as root,
without a card at hand:

```
modprobe scsi_debug dev_size_mb=256 removable=1   # a removable /dev/sdX
dd-hub-writer list
dd-hub-writer write image.img.zst sdX $(sha256sum image.img.zst | cut -d' ' -f1) --progress
modprobe -r scsi_debug
```

A card written another way (Raspberry Pi Imager with our image list) has no
pairing key; the installer adds one to it (below).

### First start

```
initrd  find the house box's Tang on the LAN (Tang's port, 7654); its
        advertisement must be signed by a key whose thumbprint is in the
        image (the release lists every house box's Tang thumbprint)
        make the locked partition: LUKS2, a random key, Clevis bound to Tang
        nixos-install the seed's system into it; reboot
after   unlock through Tang; make the controller key (ed25519, on the locked
        partition); the door starts in pairing mode on :8080, LAN only
```

### Pairing

Over the house network, before the controller has a name or a certificate,
so every message, each way, carries a mac in `x-commonty-mac`:
HMAC-SHA256 with the pairing key over `commonty controller pair v1\0` and
the exact bytes of the body (never a message read and written again, so a
client in any language makes the same one):

```
app ─▶ door   POST :8080/pair {entry: SignedEntry, at}
              the door checks the mac, identity::verify(entry), and saves
              entry.root as the household's first person
door ─▶ app   {controller: <ed25519 public, b64>, label: <16 base32, from the key>}
app           saves the controller key (it pins it from now on)
              signs the name with the account's root:
              commonty controller name v1\0<label>.home.<domain>\0<controller>
              asks the gate for a network key (POST /_dd/network/controller,
              its device token): a single-use tag:controller key
app ─▶ door   POST :8080/pair/finish {network_key, name_sig, domain}
door          joins the Commonty network, registers its name (below),
              gets its certificate, removes /boot/commonty/pair, stops :8080
```

#### The root's word, either kind of root

`identity::controller_name_signed(entry, name, controller, sig, origins)`
is the one check of "the account's root gave this controller this name",
used by the door, the gate and the DNS Worker alike. It makes the message
itself (`controller_name_message`), so no other signature of the root's
can pass for one:

```
a device-key root   sig = base64 ed25519 over message
a passkey root      sig = base64 of the JSON Assertion (as an entry's own
                    signature is), the root passkey answering the challenge
                    sha256(message), on one of origins (https://home.<domain>)
```

`identity::controller_name_message(name, controller)` is the name message
above, for whatever signs it. A passkey root signs it in a browser. The app
first asks the gate for it (`PUT /_dd/controller-name/<label>` {controller},
its device token; the label must be that key's), then opens
`https://home.<domain>/_dd/controller-name#<label>` in the system browser.
That page (the gate's) reads what the signed-in member's own app asked
(`GET /_dd/controller-name/<label>/ask`), never a key from the link, so a
link someone sends a member cannot get their passkey to name another
controller. It asks the passkey and leaves the answer at the gate (`POST
/_dd/controller-name/<label>` {sig}, the member signed in), which keeps it
only if it is that member's root's word for the asked key; the app collects it (`GET /_dd/controller-name/<label>`, its
device token, the same member; given once, kept 10 minutes at most, by
member and label, so nobody else's answer can stand in for it). The signature
is no secret: it names the controller, it opens nothing. The app asks
every 2 seconds for up to 10 minutes: `404` until the browser has answered,
then `{sig}`, which the app checks with `identity::controller_name_signed`
before /pair/finish (a box could hand back anything). The installer says
to answer the passkey in the browser while it waits. A device-key root
that is not on this device cannot sign, and the installer says so before
anything is written.

#### Without a pairing key

A card written another way (Raspberry Pi Imager) has no pairing key. Its
door says so (`GET :8080/pair/status` → `{pairing: false, needs_key: true}`),
pairs with nobody, and its page says to add the key from the app. In the
installer that is "My card was written another way" on the first step:
the person picks the card from the same list of removable disks, and the
app writes only `/boot/commonty/pair` (32 random bytes) to its boot
partition, mounted as the user the same way as after writing an image
(`udisksctl`, `diskutil`, the drive letter). No image is written and no
admin rights are asked. The key stays in the installer's memory; the
person puts the card back in the Pi and powers it, and Start the Pi and
Pair follow as for a card the app wrote, restoring a backup included.
The door reads the key when it starts.

Why not compare digits on the door's page instead: that page is plain
http, found by its mDNS name, before the controller has a name or a
certificate. A man in the middle on the house network can show the same
digits in both places, be the controller the app pins, and be sent the
network key and a restored backup. A key carried on the card itself never
crosses the network.

What it does not stop: someone with the card in their hands before it is
paired; whoever pairs is the first person in, on the door's people page
for everyone to see.

```
GET :8080/pair/status   {pairing, needs_key, paired, finished, restore}
                        pairing    a key is on the card and pairing is not finished
                        restore    null, or {state: working|done|failed, error?}
```

### Remembering it

The app keeps the controller as `{label, name, controller}` in the
member's list of home controllers: JSON, locked with their own library key
(as the Devices tab locks device names), kept at the gate as bytes
(`GET`/`PUT /_dd/controllers`, a member's own, by session or device token,
64 KiB at most; gone when they leave). No box can tell from it whether a
household has a controller. The app reads it with the device's keys
(`/_dd/app/controllers`, [`app/src/controllers.rs`](../../app/src/controllers.rs));
a browser opens it with the passkey in Settings, Network and keeps the
opened list in its own storage. The home page's Smart home tile is drawn
from it by `shell.js`, never from `/_dd/me`.

## The door

The only thing listening on the network: `:443` on `commonty0`. It ends TLS
itself.

### Signing in

The door keeps, per person: the newest `SignedEntry` it has accepted, held
to the root it saved, by `identity::accept` (an older or forged entry never
replaces a newer one). It refreshes them every hour from the gate's
`GET /_dd/controller/directory/<name>` (below, "The gate, for a
controller") when it can; a box can withhold or replay an entry, never
forge one.

```
GET  /_door/challenge                 {nonce} (32 random bytes, b64url, 2 minutes)
POST /_door/signin                    one of:
  passkey  {name, id, assertion, nonce}
           identity::check_login(entry, id, assertion,
             sha256("commonty door v1\0" host "\0" nonce), ["https://" host])
  device   {name, entry, device, sig, nonce}
           device's key in the entry's devices; sig = ed25519 over
           "commonty door v1\0" host "\0" nonce "\0" name; the entry may be newer
           than the door's and replaces it if identity::accept allows
→ Set-Cookie: door=<random>; Secure; HttpOnly; SameSite=Strict; Path=/; 8 hours
```

The passkey's relying party is the bare domain, which the door's name is
under, so the members' existing passkeys work there. A passkey listed in
the entries of two people let in signs nobody in: nobody in can copy
another's into their own entry and be taken for them. Someone only asking
does not count, so they cannot shut anyone out that way.

### Members only

Only a member of the fleet gets through: the release's member list
(`fleet/members.json`) comes with the controller's system
([members.rs](../../servers/smarthome/door/src/members.rs)), and the door
checks it at pairing, at every sign-in, and on every request of a session.
Only the root the list names as a person's current one counts
(`identity::anchored`), as for the DNS Worker naming a controller: an entry
from before a recovery may be signed by a key in a thief's hands, and
someone let in only by an invite gets in once a release lists them. So nobody can take a member's name here, or keep them from
asking, and a member the release revokes is shut out by the release that
revokes them. Two limits, said here: a household that holds its updates
also holds the member list where it was; and a member who left by their
own statement (leaving.md) is let through until the release removes them,
since the door has no box to ask.

### Who is who in Home Assistant

```
browser or app ─TLS─▶ door ─ person P ─▶ bind 127.0.1.<n> ─▶ 127.0.0.1:8123
```

Each person is one Home Assistant user, made by the door (over Home
Assistant's websocket api, as its own admin) when they are let in. The
door writes `127.0.1.<n>: <user id>` into `/var/lib/hass/commonty-users.yaml`
and restarts Home Assistant (and again, later, if that restart failed).
Home Assistant saves a new user a little after making it, so a restart can
lose one: 30 seconds after each restart the door lists Home Assistant's
users (`config/auth/list`) and makes again anyone's that is gone. Home
Assistant's trusted networks provider signs each address in as its user,
without a page
(`nix/modules/smarthome/home-assistant.nix`). The door never sends
`X-Forwarded-For`, and strips its own cookie before passing a request on.
It adds the Commonty bar to Home Assistant's pages.

### People

```
GET  /_door/people        who is in, who asked
POST /_door/ask           a signed-in member of the fleet who is not in asks
POST /_door/let-in        {name}  a person already in approves
POST /_door/remove        {name}  drops their root, their sessions, their
                                  HA user's address
```

## The name and its certificate

`<label>.home.<domain>`, `label` made from the controller's key
(`identity::controller_label`: base32 of the first 10 bytes of
sha256("commonty controller label v1\0" ‖ key), lowercase). It looks random,
so the name says nothing about the house, and only the holder of that key
can claim it: the door, the gate and the DNS Worker all refuse a name whose
label is not its controller's own. Labels are public (DNS, CT logs), so a
random label anyone could name would let one member squat another
household's label at the gate before its controller first backs up. Three
layers:

- **A public certificate.** The DNS Worker answers the controller's ACME
  challenges only for a name a member's root signed for that controller's
  key. It checks the member's entry the way the mail Worker does (pinned,
  `admit` in `client/mail`), so no box can get a certificate for the name,
  and sets its A record to the controller's network address.

  ```
  POST /api/controller {label, controller, entry, sig, address, at, own}
       register, and every 15 minutes after, where it is now; own by the
       controller key over commonty controller address v1\0<label>\0<address>\0<at>
  POST /api/challenge  {controller, at, op, name, value, sig, key, entry, name_sig}
       as for a box, sig by the controller key over
       commonty acme v1\0<controller>\0at\0op\0name\0value
  ```

  Both carry the member's word for the controller (their entry and their
  root's signature of its name), checked each time against this release's
  members. There is no other way to move a name: a controller whose member
  has left moves nothing and gets no new certificate, and its name lapses
  with the one it has.
- **The app pins the controller's key** from pairing and refuses any other,
  whatever the certificate says. The certificate is ACME's and cannot carry
  an ed25519 key, so the door vouches for it:

  ```
  GET /_door/tls   {sig}   the controller key over
                   commonty door tls v1\0<host>\0<hex sha256 of the leaf certificate>
  ```

  The app asks it on a connection that carries nothing, checks the
  signature against the key from pairing and the certificate that
  connection showed, and from then on its handshake to that name takes
  that certificate alone ([`app/src/smarthome.rs`](../../app/src/smarthome.rs)).
  Smart home in the app goes through the app's own server on
  `127.0.0.1:41663` (the launch-secret cookie, as Photos), which signs in
  with the device key, keeps the door's cookie itself, passes WebSocket
  upgrades, and lets Home Assistant's pages and scripts through: the Pi is
  the household's own device, not a box. Desktop and Android alike: a
  webview cannot be held to a key, so the window never talks to the door
  itself. A browser opening the name directly has the public certificate
  and the CT watch, not the pin.
- **The door watches Certificate Transparency** (crt.sh) for its name every
  hour and tells the household on its pages if a certificate appears that
  it did not ask for.

## The network

The controller joins with a single-use key the gate mints in the `boxes`
user, tagged `tag:controller`. The policy lets any member's device reach a
controller on 443 alone (`nix/modules/net/headscale.nix`); the door turns
away everyone outside its household.

## Unlocking the card

Clevis bound to Tang on the house box (`nix/modules/smarthome/tang.nix`).
The box does one blinded key operation and never learns the disk key. Tang
answers on the house LAN only, never on the Commonty network, so a Pi
taken out of the house cannot unlock. The Pi is wired because the unlock
happens before the system is up.

What it does not stop: root on the house box together with the card itself.
A household device as a second share (`sss`) closes that later.

## The gate, for a controller

A controller is no member and holds no device token; it asks the gate
(the one holding the libraries bucket, `files.<domain>`) with its own key
and its pairing person's word for it:

```
x-dd-controller: base64url (unpadded) of JSON
  {member, label, controller, name_sig, at, proof}
  member    the pairing person's name; label, controller, name_sig as registered
  name_sig  the member's root over controller_name_message(<label>.home.<domain>, controller)
  at        unix seconds, a number
  proof     base64 ed25519 by the controller's key over
            "commonty controller request v1\0" METHOD "\0" path "\0" at
            (path as sent, the name in it percent-escaped; at in decimal)
```

The door's client is [gate.rs](../../servers/smarthome/door/src/gate.rs):
the gate is `DOOR_GATE` (`https://files.<domain>`), and the registration
(`registration.json`, written at /pair/finish) gives the rest.

The gate checks the member is a member (its own directory), the name
signature by their root (`identity::controller_name_signed`), the proof, and `at`
within 5 minutes. Then:

```
GET  /_dd/controller/directory/<name>   one person's entry (the door's refresh)
POST /_dd/controller/backup             {put, get}: presigned urls, 10 minutes, for
                                        controllers/<label>/backup in the libraries bucket
GET  /_dd/controller/backup/<label>     a member (session or device token): {get}, to restore
```

Base64 is standard in the JSON's fields, `at` unix seconds, `path` the
request's path without its query. A request that fails a check is 401; a
member the release does not let in, 403; a gate without the bucket answers
the backup routes 404. `GET /_dd/controller/backup/<label>` is 404 too for a
label another member vouched for, or one with nothing put yet
([controller.rs](../../servers/gate/src/controller.rs)).

The gate keeps which member vouched for which label, and for which
controller key (`controller-backups.json` in its state, written whole): the
first word for a label holds it, and another member or another key naming
the same label is refused (409), so nobody reaches another household's
backup by its label. A label is its key's, so no other controller can be
named with it at all (401). A member who leaves takes their controllers' backups
with them (leaving.md).

## Backups

Every night (the first an hour after the door starts, then every 24
hours), and when someone is let in or removed, the door asks Home
Assistant for a backup over its websocket (`backup/generate` {agent_ids:
["backup.local"], include_homeassistant, include_database}, then
`backup/info` every 2 seconds until its `state` is `idle`; a
`last_action_event` that failed is a failed backup), takes the newest
`.tar` in Home Assistant's backups directory (`DOOR_HA_BACKUPS`,
`/var/lib/hass/backups`), and puts it, with its own state (who is in,
their entries, Home Assistant's admin; never sessions), in one file,
locked ([backup.rs](../../servers/smarthome/door/src/backup.rs)):

```
"CBK1" | u32 BE header length | header JSON | secretbox(K, nonce, tar)
header  {nonce: <24 bytes, base64url>, sealed: [{to: <ed25519 public key, as the
        entry lists it>, key: <library::seal_to(to, K)>}], made: <unix seconds>}
tar     ustar, two files: home-assistant.tar (Home Assistant's backup, as it
        wrote it) and door-state.json (the door's state.json, sessions left out)
```

K is the backup key: 32 random bytes the door made once, kept on the
locked card (`backup.key` beside the state, 0600). It is sealed
(library::seal_to) to every device key in the entries of the people in
(a passkey holds no keys: keys come only from devices); so any of their
devices can open it, and no box can. The file (512 MiB at most) goes up through the gate's presigned url
(`POST /_dd/controller/backup`, then `PUT` to its `put`): the box keeps
ciphertext, its size, and when it changed. Home Assistant's tar is then
removed from the card, sent or not, so the card does not fill. How the
last one went is in `/_door/status` as `backup: {at, ok, error?}` (`at` in
unix seconds) and on the people page.

Restoring is part of setting up a new card: the installer asks the gate
for the member's controllers' backups, the member picks one, the app opens
K with its own key, and after pairing sends the file and K, sealed to the
new controller's key:

```
app ─▶ door   POST :8080/pair/restore  body: the backup file (512 MiB at most)
              x-commonty-mac over it as for /pair; header x-commonty-backup-key:
              K sealed to the controller's key (library::seal_to)
door          checks the mac, opens K and the file, reads both parts and
              checks every person's entry by its root; any of it wrong: 400
door ─▶ app   {"restoring": true}, with x-commonty-mac over its bytes
door, after   writes Home Assistant's tar to its backups directory as
              commonty-restore-<backup.json's slug>.tar, restarts Home Assistant
              (it reads that directory when it starts), asks backup/restore
              {backup_id, agent_id: "backup.local", restore_database,
              restore_homeassistant}, and waits for it to come back; then
              takes the people (and their entries), their Home Assistant
              users and addresses, and Home Assistant's admin from the saved
              state (never sessions, the controller key or the label; the
              person who paired stays in), and K as its own backup key
```

after /pair and before /pair/finish. Its answer carries the mac as /pair's does, and the restore runs on after it: `/pair/status`'s `restore` says how it went. The
installer offers "Start from a backup" once a card is chosen, listing the
member's controllers (one without a backup is listed but cannot be
picked); it downloads the file and opens K before /pair, so nothing is
paired if the backup cannot be had, then sends it after /pair. Both show bytes and time
left. The entry in `sealed` whose `to` is this device's public key is
opened with `library::open_with`; a backup made before the device was
admitted has none, and the installer says to restore from a device that
was. The new controller gets a new label and name; the app replaces the
old one in the member's list.

Settings, Network shows a controller's "Last backup" from the door's
`/_door/status` field `backup: {at, ok, error?}` (`at` unix seconds),
when the door gives it.

## Building and releasing

- **CI builds it.** `nixosConfigurations.home-controller` is
  `aarch64-linux`. Our programs in it are cross-compiled on the x86
  runners; everything else comes from cache.nixos.org; the runners run the
  few remaining aarch64 steps under qemu (`boot.binfmt.emulatedSystems` on
  the runner bundle). The card's image steps (the ext4, the partitions,
  zstd) only copy files, so `cardImage` (nix/controller/image.nix) runs
  them with x86 tools rather than nixpkgs' `sdImage` under qemu. `build_host` builds it on every runner and
  `same_build` holds them to one path, as for a box.
- **Tests.** The door's and the writer's crate tests run with the rest.
  The `home-controller` VM test runs the same modules on x86: pairing,
  sign-in, refusal, the users file, Tang unlock. Nothing boots the real
  card image in CI yet; it is first tried on a Pi.
- **Release.** `dd release publish` signs the `home-controller` system
  into the release beside the boxes' (fleet/devices.json lists it); the
  card is one of the app's downloads (`dd release app`, built by two
  runners, which emulate aarch64), so `app.json` signs its sha256. The
  controller's `dd-agent` follows its entry in the release and, as a
  box's does, moves only to a closure two of the fleet's boxes vouched for
  (`attesters`, their ssh host keys), fetching closures through the gate's
  read-only cache door
  (`https://cache.<domain>`, on the network only, tagged controllers only).
  Updates are automatic; a household can hold them from the door's pages.

## What a box can and cannot do

```
can     see that the controller exists, when it and the household's devices
        connect, how big its backups are
        cut it off from outside the house
cannot  read anything in the home, its backups or its traffic
        sign in to it, add a person, change what it runs, or get a
        certificate for its name
```

## Open

- A household device as a second unlock share.
- Following a person whose root changed (recovery): the door needs the
  previous entry, which it keeps, and `identity::accept` does the rest.
