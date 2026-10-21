# Encrypted libraries

A member's files, films and shows are their library: one folder tree,
named and encrypted on their own device, stored on the boxes as
ciphertext. The Files page, Movies & TV, `dd library`, `dd media` and the
app all read the same library, in the same format, through the same door.
This page is how that fits together, and how to change it.

## The rules

- **Ciphertext at rest, made on the device.** Names and bytes are
  encrypted before they leave the member's machine or browser tab. A box
  moves and stores what it cannot read.
- **No format of ours.** A library is an rclone crypt remote, byte for
  byte. `rclone mount` opens it on any desktop with the key and the id,
  and nothing here has to exist for that to keep working.
- **The key lives in the identity.** A library's key is sealed to the
  member's own keys and kept in their signed directory entry. Never on a
  box, never in a file.
- **Keys come only from devices.** The keys it is sealed to are ones a
  device holds: the app's and dd's device keys, a root kept on one of
  them, and the paper recovery key. Never a passkey: a passkey answers its
  PRF to any page of the domain, so any box could ask for it. A browser
  gets a library key only from the member's app, which opens it with its
  own keys and serves the page that uses it.
- **One door.** Every read and write goes through the gate, which decides
  owner, reader or nobody. No S3 key leaves a box.
- **A delete goes to the trash first.** A delete is a move into the
  library's trash, which the box empties 30 days on; a write over a name
  already taken puts what was there into the trash first. Only the owner,
  from the trash itself, deletes for good at once: **Delete forever** on
  one thing, **Empty trash** on all of it.
- **Plaintext only in memory, only while working.** A box that plays a
  film for someone decrypts it in RAM for that one viewing and forgets it,
  with keys that open that film and nothing else of the library.

## How the bytes move

```
the member's device                      the box (files.<domain>)                 the storage pool
───────────────────                      ────────────────────────                 ────────────────
browser tab (Files, Movies & TV)
  wasm: names + blocks sealed here ─┐
the app (same pages, device token) ─┤    nginx  /_dd/ ──► gate (dd-verify)
dd library / dd media (rclone) ─────┘      │              │ allowed(): token or cookie,
                                           │              │ member list, owner/reader
        WebDAV under /_dd/dav/<lib>/...    │              ▼
        bearer token or session cookie ────┘     library-gate (servers/library)
                                                   presigns one object, 10 minutes ──► garage, bucket "libraries"
                                                   streams the bytes through             <lib>/<encrypted path>
                                                   lists, copies, moves to trash/        <lib>/trash/<when>/...
```

- A device never holds a bucket key. The gate holds one (`LIBRARY_ID`,
  `LIBRARY_SECRET`) and signs a url for exactly one object, good for
  [ten minutes](../../servers/library/src/lib.rs) (`URL_SECS`), then
  fetches or sends the bytes itself and streams them on.
- One bucket holds every library, a prefix per library id. Which prefix a
  request may touch is the gate's decision from the directory, not
  garage's.
- `s3.<domain>` is garage's public face, on the gateway
  ([nix/roles/gateway.nix](../../nix/roles/gateway.nix)). The gate on the
  library box reaches the bucket there
  ([nix/modules/library/libraries.nix](../../nix/modules/library/libraries.nix)).

## How a film plays

```
browser tab                         gate (files.<domain>)               dd-transcode (same box)
───────────                         ─────────────────────               ───────────────────────
GET /_dd/dav/<lib>/<enc>  ────────► presign GET, 6 hours
    x-dd-presign: 1       ◄──────── { url }
GET /_dd/dav/<lib>/<enc>  ────────► the header, 32 bytes
    range: bytes=0-31
GET /_dd/transcode/key ───────────────────── auth_request ────────────► the box's key, made at start
(the tab makes the file's block keys from the header and seals them to that key)
POST /_dd/transcode/session ──────────────── auth_request, X-DD-User ─► check url prefix, open the keys
  { url, keys: the file's block                                         fetch header (ranged GET)
    keys sealed to the box,                                             ffprobe, ffmpeg ──┐
    size, from }                                                                         │ reads plain bytes
                          ◄──────────────────────────────────────────── { id, playlist,  │ from 127.0.0.1:4191
                                                                          from, duration }│ /plain/<id>
GET /_dd/transcode/session/<id>/index.m3u8, 00042.ts ...  (no gate) ──► segments, from tmpfs
DELETE /_dd/transcode/session/<id> ───────────────────────────────────► ffmpeg killed, folder wiped

                       dd-transcode ── ranged GET, 4 MiB at a time ──► s3.<domain>/libraries/<lib>/<enc>
                                       decrypt in memory, 256 MiB kept per session
```

## The encryption

[client/library/src/crypt.rs](../../client/library/src/crypt.rs) is
rclone's crypt format, checked byte for byte against rclone itself
([client/library/tests/rclone.rs](../../client/library/tests/rclone.rs),
which needs `rclone` on PATH and skips without it).

```
keys    scrypt(N=2^14, r=8, p=1) over password and salt -> 80 bytes:
          32 data key | 32 name key | 16 name tweak
        password = the library key, base64; salt = the library id
names   each path segment: AES-256-EME, PKCS#7, lowercase base32hex
files   "RCLONE\0\0" | 24-byte nonce | blocks of 64 KiB, each NaCl secretbox
        (16-byte tag + bytes) under the nonce plus the block number
```

Every block has its own nonce, so a file can be sealed or opened a piece
at a time, in any order, and the result is the same as doing it whole.
The browser, the app, `dd` and the transcoder all rely on that.

Every block also has its own key, which is what a transcoder gets instead
of the data key (`block_keys`, `BlockOpener` in crypt.rs). Secretbox's
XSalsa20 is two steps: HSalsa20 of the key over the nonce's first 16
bytes makes a subkey, and Salsa20 runs under that subkey and the nonce's
last 8 bytes (its first 32 bytes of stream are the Poly1305 key). rclone
adds the block number to the nonce from its first byte up, so the first
16 bytes, and the subkey, differ for every block:

```
block key n   HSalsa20(data key, (file nonce + n)[0..16])     32 bytes
open block n  Salsa20(block key n, (file nonce + n)[16..24]) + Poly1305
```

HSalsa20 is a PRF: a block key gives back neither the data key nor any
other block's key, and another file's blocks (another random nonce) need
keys it does not have.

### Where the keys live

A library is a record in its owner's signed entry
([client/identity/src/lib.rs](../../client/identity/src/lib.rs),
`Library`): an id (32 hex characters), the key sealed once for each key
that may open it, a list of readers, and when it was made.
[`seal_for_entry`](../../client/library/src/lib.rs) seals it to:

```
device:<fingerprint>   each device in the entry
root                   the root key, unless the root is a passkey
recovery               the paper recovery key, if there is one
```

Sealing is a libsodium sealed box to the x25519 twin of an ed25519 key,
so the keys already in the directory are the ones that open it. A device
admitted to the entry gets the libraries sealed to it in the same signed
step ([client/account/src/lib.rs](../../client/account/src/lib.rs),
`admit`). A passkey admitted gets nothing: it signs in. Recovering an
account opens each library with the paper key and seals it afresh to the
new keys; the root it replaces goes into the entry's `recovered_from`, so
the member stays a member ([gate.md](gate.md#roles)).

A browser has no device key, and gets none from a passkey. Its pages that
open a library are the app's ([app.md](app.md)): the app opens the key
with its own device key, out of the entry it checks under the root it
holds or pinned (`account::own_entry`, `Opener::anchor`; a first look is
held to the release's signed member list), and hands it to
the page through `/_dd/app/library`, to its window or to a browser the
member let in from it. A box never serves a member a page that holds a
library key; the demo's library holds nothing private and is the one
exception.

The other secrets a member's devices share go the same way
(`account::seal_for_keys`): the archive password for Backups
(`dd-devices.json`, written by `dd image push` and `dd image devices`) and
the Photos password (`dd-photos.json`, written by the app), beside the
member's disk images. Each is sealed to the same keys as a library and
signed by the device that made it; a copy is opened only when a device of
the root-signed entry signed it, for that purpose. Sealing needs only a
public key, so a box could seal a secret of its own to a device; the
signature is what refuses it. For an account whose root is a passkey
(signed by an assertion a box's page could have asked for) a copy is
sealed to the device that made it alone, and only that device's own copy
is believed.

### Moving off passkey keys

Entries made before this sealed library keys to `passkey:<id>`, and a
passkey carried the public half of a key its PRF made (`library_key`).
The move:

- The app, at each launch where the root is on its device, runs
  `account::reseal`: every library key is opened with the root and sealed
  again to the devices, the root and the paper key alone, and the passkey
  seals and `library_key`s go, in one signed version. `dd library reseal`
  does the same by hand. Every other entry update drops them too
  (`identity::drop_passkey_keys`).
- The directory refuses a new version that still carries one
  (`directory::no_passkey_keys`); older versions on file still read and
  still move between boxes.
- **What this does not undo.** The library key stays what it was, and
  entries are public: anyone who kept an old version holds the passkey
  seal. A box that can get the old passkey's PRF (by asking during any
  sign-in) can still open that seal, for as long as that passkey exists.
  Closing it takes one of: each member replacing their passkeys (a new
  one made, the old one removed from the entry and deleted from their
  password manager), or rotating the library key (re-encrypting the
  library, names and all, on a device). Neither is automatic yet. The same
  holds for an archive password once sealed to a passkey
  (`dd-passkeys.json`, deleted by `dd image push` or `dd image devices`) and
  for the Photos account's old keys at museum (a box that kept them).

Readers are in the format and the gate already honours them (a reader's
keys are sealed under the owner's entry, `readers[].keys`), but nothing
writes a reader yet: no command or page shares a library. A device takes
a library shared with it only from an owner whose entry the release's
signed member list anchors (`media::gate::openable`,
`directory::resolve_released`): any entry would otherwise do, and a box
can make one, sealed to the member's device, and read what goes in.

### What a box can and cannot read

```
                          names   contents   sizes, counts, who fetches what, when
the gate's box            no      no         yes
garage, every box in it   no      no         yes
a transcoding box         no      one film   yes
  (while a session lives: it holds that film's block keys, sealed to a
   key it made at start, and fetches the one file it was given a url to)
```

The box never gets the data key. It gets one key per 64 KiB block of the
one film, made in the member's tab from the film's header, and they open
that film and nothing else: not another file of the library, even if the
box could fetch it, and not the data key. It is given one presigned url
and no bucket key, and the keys go when the session ends or the service
restarts.

### The demo's library

The demo account is one account every visitor shares, so it cannot hold a
passkey. The box keeps a library for it whose id and key are plain in the
configuration (`dd.verify.demoLibrary`,
[libraries.nix](../../nix/modules/library/libraries.nix)) and hands the
key to the demo's pages in `/_dd/config`. The gate lets the demo read that
one library and nothing else, and refuses that id to anyone else's entry.
Nothing private is in it.

## The gate

Two halves.
[servers/gate/src/library.rs](../../servers/gate/src/library.rs) decides
who may; [servers/library/](../../servers/library/) (crate
`library-gate`) is the storage and WebDAV behind it.

### Who may

`allowed()` answers every request except OPTIONS:

1. The id must be 32 hex characters.
2. The caller is a device token (a biscuit, signed by the device, for this
   host) or the box's own session cookie. `dd`, rclone and the app bring
   the first; the pages in a browser the second.
3. The demo reads the demo library and nothing else.
4. Anyone else must be on the release's member list. An entry is a
   document a person writes about themselves, so naming a library id in
   it proves nothing alone.
5. The owner is the member whose entry holds the library. A reader is a
   member some owner's entry names in that library's `readers`; one named
   there with `writes: true` is a writer.

An owner may write; a reader gets 403 for PUT, POST, MKCOL, MOVE, COPY,
DELETE and PROPPATCH. A writer (a calendar two people keep) may do all of
those but delete for good: a DELETE in the trash, or of the trash, is the
owner's alone. What a writer saves counts against the owner's allowance.

### WebDAV

[servers/library/src/dav.rs](../../servers/library/src/dav.rs), routed at
`/_dd/dav/{lib}`, `/_dd/dav/{lib}/` and `/_dd/dav/{lib}/{*path}`
([servers/gate/src/lib.rs](../../servers/gate/src/lib.rs)):

```
OPTIONS                 answers before anyone is asked who they are
PROPFIND  depth 0 or 1  a file's size, date and etag, or a folder's level;
                        the root hides trash/
GET, HEAD               streamed through, Range passed on
GET on a folder         the root, or a path ending /: its listing as JSON,
                        one level, nginx autoindex's shape
                        ([{name, type, mtime, size}]); nothing under it is
                        404; the root hides trash/
GET + x-dd-presign: 1   { url }: a presigned GET good for 6 hours, for a
                        transcoder (HANDOFF_SECS)
PUT                     streamed through; 201 with the new ETag. What had
                        the name is copied into the trash first
MKCOL                   a folder is a zero-byte "<path>/" marker object
MOVE, COPY              Destination must be in the same library, not the
                        source or inside it; with Overwrite: F, 412 if the
                        name is taken, otherwise what was there goes to the
                        trash first; a folder moves object by object (copy,
                        then delete)
DELETE                  moves the file or folder to trash/<unix seconds>/<path>,
                        a second later while that place is taken
DELETE in the trash     trash/<stamp>/...: that thing, gone for good at once;
                        trash itself: all of it (Empty trash). The owner's
                        only, like every write; 404 for what is not there
PROPPATCH               refused (403 in a multistatus)

If-Match, If-None-Match on PUT, DELETE and an upload's last piece: made only from
                        that version (an etag, or * for anything / nothing);
                        otherwise 412 and nothing changes

in pieces, for a file too large for a tab (header x-dd-upload):
POST   x-dd-upload: start              -> { upload: id }   S3 multipart
PUT    x-dd-upload: <id>, x-dd-part: n -> { etag }         n is 1..10000
POST   x-dd-upload: <id>, { parts: [etag, ...] }           finished
DELETE x-dd-upload: <id>                                   abandoned parts dropped
```

Paths are cleaned before use: no empty segments, no `.` or `..`, no
backslash. The trash keeps its own names in the clear and the library's
encrypted names under them. Nothing may be written, moved or copied into
the trash (`trash` and `trash/...` exactly: an encrypted name may begin
with those letters and is no part of it); the one move out of it is a
restore, `trash/<stamp>/...` back into the library.

A PUT or DELETE that carries `If-Match` or `If-None-Match` is made from
a version: the gate looks at the object's etag and writes under that
object's lock, so two devices saving one calendar event from the same
version cannot both win; the second gets 412, reads the new one and
merges. A write without either header is the last one wins, as before.
The lock is in the gate's memory, so it holds for writes that reach the
same box.

A write over a name keeps every version: the one before goes to the
trash for 30 days. So what is rewritten often does not live in the
library: Movies & TV's place in each film and its shelf are notes the
gate keeps beside it ([Reading a library](#in-a-browser)).

The gate empties the trash itself, once a day (an hour after a failure,
as at boot before the bucket is up): every `trash/<stamp>/` older than 30
days is deleted, for every library (`purge_trash`). A delete for good
(`delete_for_good`) lists what is there and deletes it under that
library's trash lock (the one a move into the trash takes too), and
reports the bytes it took out, so two deletes of one thing count it once;
one library's lock never holds up another's.

### Allowance

Every write by an owner is held to their one allowance across every
service ([servers/gate/src/library.rs](../../servers/gate/src/library.rs),
`metered`; the numbers in
[servers/gate/src/storage.rs](../../servers/gate/src/storage.rs)). Room is
set aside first: the size the request declares, or all that is left when
it declares none. The body is counted as it streams and cut off past that
room with 507, so the bucket never keeps the object. A COPY is measured in
the bucket first and holds that much. A piece of an upload in pieces
(`x-dd-upload`) is metered like a PUT, whatever its method. What lands is
counted until the fleet's count next includes it, and what a delete for
good took out of the trash is room again at once (`Ledger::freed`). See
[Storage](#storage).

## Reading a library

### In a browser

The pages are plain files: [servers/files/web/](../../servers/files/web/)
(Files) and [servers/media/web/](../../servers/media/web/) (Movies & TV).
Both import [servers/home/web/library.js](../../servers/home/web/library.js),
which opens the library and speaks to the gate, and the wasm in
[client/web/src/library.rs](../../client/web/src/library.rs), which does
the crypto with the same `library` crate as everything else.

Files' work on many files at once is
[servers/files/web/files-more.js](../../servers/files/web/files-more.js):

- **A folder or a selection as one zip:** each file read by ranges and
  opened (`pieces`), streamed through `zip.js` (the Leave tab's writer,
  stored and Zip64) to a file the person picks, or a Blob where the
  browser has no picker.
- **Folders in:** a drop's folder entries (`webkitGetAsEntry`) or an
  `<input webkitdirectory>`, put with their folders made on the way;
  `..` and `.` parts of a name are dropped, so nothing lands outside the
  folder it was dropped in.
- **Search:** every name under `Files/`, walked once and kept in the tab's
  memory alone (names are private, so never in its storage), found by
  every word typed; this folder's matches first, the library's below, and
  in the bar's search (`searchFrom`). `/_dd/files#<path>` opens a place.
- **Folder sizes:** everything under a folder counted, three folders at a
  time, remembered for the visit. Any change Files makes (an upload, a
  rename, a move, the trash, a new folder) forgets the sizes above it and
  the search's names, so both are worked out again when next shown.
- **A zip without a save picker** (Firefox, Safari, the app's webviews) is
  gathered in the tab before it is saved, so it is refused past 2 GB, and
  the person is told to download in parts or use a browser that writes
  straight to disk.
- A save in flight (the PDF editor's) writes beside the file as
  `.<name>.saving-<time>` first; Files does not list names of that shape,
  so a file of the member's own named so would not show either.
- **Scan to PDF** is [scan.js](../../servers/files/web/scan.js): pictures
  turned and cropped in a canvas, made a PDF by MuPDF (`Pdf.fromImages`),
  read by Tesseract into hidden text when asked (as the PDF editor's
  Recognise text), and sealed into the folder open. Up to 40 pages a scan,
  each kept as a picture until the PDF is made.

`checks.files-page` drives all of it in a browser against the Mail test's
made-up gate, and looks at what the gate holds (sealed bytes under
encrypted names) and at what it was asked: typing a search asks nothing,
and no request carries a word searched for.

The gate serves both pages at `/_dd/files` and `/_dd/media` to the demo
when it has a library; a member's are the app's (below). They belong on `files.<domain>`: that
host is the only one that serves `/_dd/transcode`.

Opening (`unlock`):

```
the app         GET /_dd/app/library: the app opens it with the device's
                keys and hands the page { id, key, reader }: its window,
                or a browser it let in (the page on s-<session>.localhost)
the demo        /_dd/config carries demoLibrary { id, key }; reader
anything else   { app: true }: the page says it opens through the app; a
                box serves a member the page that hands over to it instead
```

Then every name is encrypted to ask the gate and decrypted from its
answer in the tab. scrypt runs once per library per tab: the wasm keeps
the last four ciphers.

- **Small files** (up to 96 blocks, 6 MiB) are sealed whole and PUT.
  Larger ones are read, sealed and sent a piece of 96 blocks at a time as
  a multipart upload: over S3's 5 MiB smallest part, under nginx's 10 MB
  body. Downloads come back by ranges, straight to a file the person picks
  where the browser allows it.
- **Files** shows the `Files/` folder: folders, a preview (whole files up
  to 64 MiB), uploads, move and rename (MOVE with `Overwrite: F`), and the
  trash with Restore, Delete forever and Empty trash (each asks first and
  says it cannot be undone). A reader sees no write controls.
- **Movies & TV** shows `Movies/` (a film is a file) and `Shows/` (a show
  is a folder of episodes, or of season folders). What it knows beyond
  names is sealed with the library's key like the rest
  ([servers/media/web/shelf.js](../../servers/media/web/shelf.js)). The
  pictures are files in the library under `.commonty/art/`. The shelf
  (year, a line about each title, its pictures' names) and where each
  viewer stopped, and on which device, are notes the gate keeps beside
  the library, replaced whole on each save, never trashed:
  `/_dd/library/<lib>/notes/shelf`, the owner's, which readers read, and
  `/_dd/library/<lib>/notes/progress`, each viewer's own
  ([gate](gate.md#libraries-and-storage-libraryrs-storagers)). An older
  page kept them in the library as `.commonty/shelf.json` and
  `.commonty/progress.json` (a reader's place in their browser): the
  first visit after moves them to the gate and deletes the old copy. With
  the gate out of reach the page still shows the shelf, and keeps the
  place in the browser until it answers.
  Posters come from TMDB, asked from the device with only the title and
  year, when the box has a key for it (`dd.verify.tmdbKeyFile`); a title
  TMDB does not know gets a still from its own frames. TMDB's art is asked
  again after five months and dropped after six.

### Editing a PDF

Files opens a PDF for editing and signing in the same tab, on MuPDF in
WebAssembly, and saves it back sealed like any upload. How it works, what
it loads and how its signatures are made is [its own page](pdf-editor.md).

### Players

[servers/media/web/media.js](../../servers/media/web/media.js) asks the box
first for every film:

1. `transcode()` in library.js: presigned url, the file's header, the
   box's key, the file's block keys sealed to it
   (`library_blocks_for_box`), POST the session.
2. The playlist plays in hls.js, pinned in the flake and served at
   `/_dd/web/hls.js`, loaded the first time a film opens. Only where hls.js
   cannot run (an iPhone) does the browser's own player take the playlist.
3. The box cannot: a file up to 256 MiB is opened whole in the tab and
   played from a Blob. Larger, the page says what the box said and offers
   to save it.

The first step starts when a film's band opens, not on Play: the band
asks the box for a session where Play would start (where it stopped, or
the beginning) and loads hls.js, so the seconds the box takes to read the
film and pack its first piece are spent while the member looks at the
band. Play takes that session, or waits on it if it is still starting;
"Play from the beginning", an episode other than the next, or a session
older than five minutes asks for a fresh one, and the band's is stopped.
Closing the band stops it too, and the box allows one film per member, so
browsing leaves at most one running. A reader (the demo) does not start
one: its visitors share the box's four places. `nix/tests/media-page`
holds it to that, against a box that takes two seconds.

The playlist grows while the box works, so the video element knows only
the part packed so far. The page's bar counts in the film's time instead:
the length the box read from the file, plus where the session began. A
jump inside what the box still has just seeks; anywhere else ends the
session and starts one there (quick presses add up to one jump). Progress
is saved every half minute and when the player stops; past 95%, or within
two minutes of the end, the film is done.

### From a terminal

[client/media/src/gate.rs](../../client/media/src/gate.rs) is the gate
for `dd` and the app: the key out of the entry (`Opener`, the device key
or the root), every library this machine can open (`openable`: its own,
then any another member's entry shares with it, each entry verified), and
the WebDAV calls with names and blocks done here.

```
dd library new             a key sealed to every key of yours, in your entry,
                           with Files, Movies and Shows made in it (needs the root)
dd library list            the libraries this machine can open
dd library key <lib>       the key as rclone's password (the id is password2)
dd library ls <lib>        everything in it
dd library put <lib> <file> [--as name]   - reads stdin, whole
dd library get <lib> <name> <out>
dd library mkdir <lib> <path>
dd library trash <lib> <name>
dd library share <lib> <member> [--write]
                           sealed to their devices, in your entry (needs the root);
                           --write makes them a writer
dd library unshare <lib> <member>
dd media [--at dir]        every openable library mounted with rclone under
                           ~/Commonty/<first 12 of the id>, until ctrl-c
```

[client/cli/src/librarycmd.rs](../../client/cli/src/librarycmd.rs),
[client/cli/src/mediacmd.rs](../../client/cli/src/mediacmd.rs). `dd
media` runs `rclone mount` ([client/media/src/mount.rs](../../client/media/src/mount.rs))
with two remotes in the environment and nothing on disk: `dav` (WebDAV at
`files.<domain>/_dd/dav/<lib>`, a device token as bearer) and `lib`
(crypt over it, the key and id obscured through rclone's stdin). The token
is good for an hour and comes from `dd media --token`, which rclone runs as
its `bearer_token_command` at start and again whenever the gate answers
401, so a mount lasts as long as it is left running.

### In the app

The app carries the Files and Movies & TV pages and library.js
([app/carried](../../app/carried)). `/_dd/app/library`
([app/src/site.rs](../../app/src/site.rs)) gives its window the member's
own library alone: one shared with them is another member's, and never
stands in as their Files. Every other `/_dd/`
request the pages make is proxied to `files.<domain>` with the device's
token instead of a cookie, `x-dd-presign` and the upload headers among
those passed on.

The same pages, and Chat, are what the app serves the member's browser
once they let it in ([app.md](app.md#the-sites-key-pages-photos-and-passwords-in-a-browser)),
from `app/src/local.rs` on `s-<session>.localhost:41660`: the same
`/_dd/app/library`, behind that browser's session cookie, and every call
sent on with the device's token. The gate hands a member's browser at
`/_dd/files`, `/_dd/media` or Chat to the app (the use-app page).

## Transcoding

[servers/transcode/src/main.rs](../../servers/transcode/src/main.rs)
(`dd-transcode`): one file, one viewer, one job.

```
GET    /key                    the box's public x25519 key, made fresh at every start
POST   /session                { url, keys, size, from } -> { id, playlist, from, duration }
GET    /session/{id}/{file}    index.m3u8 and its segments
DELETE /session/{id}           over: ffmpeg killed, folder removed
GET    /plain/{id}             second listener, ffmpeg only: the plain file, with ranges
```

A session:

1. `url` must start with `TRANSCODE_SOURCE` (the libraries bucket), and
   redirects are not followed: otherwise a member could point the box at
   anything on its network.
2. The block keys are opened with the box's secret, and there must be
   one for every block of `size`; the file's header is fetched for its
   nonce, from which each block's follows.
3. ffprobe reads the length and the streams, and at the same time the
   picture's packets over 16 seconds from where it starts (`KEYS_LOOK`:
   their keyframe flags and sizes, nothing decoded); ffmpeg reads the
   plain file from `127.0.0.1:4191/plain/<id>` and writes HLS (4-second
   segments) into `/run/dd-transcode/<id>`, a tmpfs.
4. The answer waits for the first segment, up to 30 seconds, so the page
   never asks for a playlist that is not there yet.

The plain bytes come from the bucket by ranges of 64 blocks (4 MiB),
decrypted in memory; 64 such pieces are kept per session for ffmpeg's
seeks back (256 MiB), the furthest from the current one dropped first.

**Only the work a browser needs, in segments a phone can hold.** A
copied picture can only be cut at the film's own keyframes, so its
segments are as long as those are far apart. The demo's Sintel has them
10.4 s apart: copied, it made 11 MB segments, and Chrome on a small
Android phone stopped on the third (2026-10-09). So `choose` copies the
picture (`-c:v copy`) only when all of these hold, and otherwise encodes
it with a keyframe every four seconds:

```
H.264 in 4:2:0                 what every browser plays
keyframes <= 5 s apart         COPY_KEYS_SECS, the most seen in the 16 s read
a segment <= 6 MiB             COPY_SEGMENT_BYTES, the picture's bytes a second
                               times the segment that keyframe spacing makes
the packets could be read      unread counts as far apart
```

This costs encoding for every film with sparse keyframes, which is most
x264 encodes (x264's default is one every 250 frames): on the GPU where
the box has one, on the cores (`libx264 veryfast`) where not. Small
segments come first; a box that cannot keep up shows as a slow start, not
a player that stops. Stereo AAC is copied; other sound is made stereo AAC.
Only the first picture and sound tracks go in; subtitles are dropped.
Resuming at `from` starts on the keyframe before it when the picture is
copied, and says so in `from`, so picture and sound start together.

**What it logs.** One line when a session starts, once its first segment
is there: the start of its id, where it began and how long the start
took, copy or encode (and where) and why, the picture's codec, pixel
format and size, its keyframe spacing, what a copied segment would weigh,
and the sound's codec and channels. One line when it ends: why (closed,
idle, another film, a failure), how many segments, the longest in
seconds and the largest in bytes. ffmpeg's own errors (`-loglevel
error`, up to 40 lines a session) go to the journal marked with the
session, the id cut short in them. Never a file name, a url, a key or a
whole session id, which opens the segments.

```
transcode: k3x9ab: started at 0s in 2.1s: picture encoded on the cores (keyframes far apart; h264 yuv420p 1920x818, keyframes 10.4s apart, a copied segment ~11.2 MB), sound made stereo AAC (ac3 6ch)
transcode: k3x9ab: ended (closed): 37 segments, longest 4.0s, largest 2.9 MB
```

**The player's side.** media.js runs hls.js with limits that fit a
phone: 20 MB and 20 s ahead (`maxBufferSize`, `maxBufferLength`), at
most 60 s (`maxMaxBufferLength`), 30 s kept behind
(`backBufferLength`). A fatal media error (a buffer that will not take a
segment among them) gets one `recoverMediaError()` before the page says
the player stopped, in words a member can act on. Every fatal error, the
first of a run of buffer errors and every tenth after, and the video
element's own errors (the only word from Safari's player) go to the
gate's `POST /_dd/player-error`, which logs one line: the host, the
browser family, the error, and the kind of film (its size, container
from its ending, codecs, picture size, the last segment's bytes and
seconds, seconds buffered). No name, path or library: the gate keeps each
field only in the shape the player gives it ([gate.md](gate.md#routes)).
`journalctl -u dd-verify | grep player-error` on the box serving `files.<domain>`.

**GPU encoders.** When the box's hardware file names a render node that
encodes H.264 (`dd.box.vaapi`, set from the hardware report in
[nix/modules/box/host.nix](../../nix/modules/box/host.nix)), encoding is
`h264_vaapi` on it, decoding on the chip where it can, 10-bit brought down
to 8. If ffmpeg on the GPU stops before the first segment, the session
starts again on the cores (`libx264 veryfast`).

**The six-minute window.** Segments are plaintext, so they stay in memory
and a whole film would not fit. Every second each session is held to a
window around the furthest segment the player has asked for: more than
90 segments (six minutes) behind it are deleted, and when ffmpeg is more
than 90 ahead it is paused with SIGSTOP, and continued (SIGCONT) once it
is within 45. The window is also held to 384 MiB (`WINDOW_BYTES`): the
oldest segments behind the player go while it is larger, and ffmpeg is
paused while more than half of that is ahead, until it is under a quarter.
A copied picture is the file's own bitrate, and six minutes each way of a
4K remux would be gigabytes. A jump outside the window starts a new
session there.

**Limits.**

```
MAX_SESSIONS   4 at once on a box; past that, 503 (a place is taken
               before a start waits on anything, so starts at once
               cannot all see room)
one per member starting a film ends that member's other session
               (the demo is many visitors under one name: not ended)
IDLE           10 minutes without a fetch, and the session is wiped
MemoryMax      4G for the unit (nix/modules/library/transcode.nix):
               4 x (256 MiB of pieces + 384 MiB of segments + the
               block keys), and ffmpeg
block keys     32 bytes a 64 KiB block, all of them at the start: 1 MiB
               for a 2 GB film, 25 MiB for 50 GB, and a start's body is
               a third more in base64. MAX_BLOCK_KEYS is 128 GiB of film
               (64 MiB of keys); START_BODY and nginx's
               client_max_body_size are 96 MiB to fit it
```

The keys all go at the start because the box has no way to ask for more:
the page is a player fetching segments, and a browser's own player
(Safari, an iPhone) fetches them with no page code in the way, while
ffmpeg seeks wherever the container sends it and would stall on a key
the page had not sent yet.

**Kept to itself.** The `/plain` listener answers only connections whose
socket belongs to the service's own user, read from `/proc/net/tcp`: the
session id is on ffmpeg's command line, where anyone on the box can read
it. ffmpeg and ffprobe may open only the expected containers
(`-format_whitelist`) over http and tcp. The unit runs as a dynamic user
with no core dumps, a system call filter, and only the render node as a
device.

### Behind nginx

[nix/modules/library/transcode.nix](../../nix/modules/library/transcode.nix),
on `files.<domain>`:

```
/_dd/transcode/            auth_request; X-DD-User from the gate; no Authorization passed on
= /_dd/transcode/session   the same, exactly (a slash-ended location would redirect the POST)
/_dd/transcode/session/    no gate: the id is the credential, like a presigned url,
                           since a player element cannot carry a token; no buffering
```

The demo may start a film: the gate counts a POST to
`/_dd/transcode/session` as reading
([servers/gate/src/lib.rs](../../servers/gate/src/lib.rs), `demo_allows`).

## Storage

- **garage** ([nix/modules/storage/garage.nix](../../nix/modules/storage/garage.nix))
  holds the bucket. `dd.libraries` asks for a bucket `libraries` with a key
  of its own (`dd-library`, read and write) imported from the env file,
  and no CORS: no page reaches the bucket itself.
- **The storage pool.** The garage dataset is capped at garage's capacity
  (`dd.zfs.atMountpoint`, never below what it already holds). Its blocks
  keep 15-minute, hourly and daily snapshots, so a week, never weekly or
  monthly, like every dataset on every box
  ([nix/modules/storage/snapshots.nix](../../nix/modules/storage/snapshots.nix)).
  Garage's metadata snapshots are kept 7 days
  ([nix/modules/storage/backup.nix](../../nix/modules/storage/backup.nix)).
  What each place keeps, and the check that holds the sum to 90 days, is in
  [servers.md](servers.md#retention).
- **What each box says.** Each box puts `usage/<service>/<member>` facts in
  its heartbeat; dd-control reads them from
  `/var/lib/dd-facts/usage-*.facts`, one `usage/<service>/<member> <bytes>`
  a line
  ([client/cli/src/storage_ledger.rs](../../client/cli/src/storage_ledger.rs)).
  They are not `.prom` files: node-exporter never serves them, so no
  member's figures are in Prometheus or Thanos (`checks.usage-facts`).
  - The forge counts `code` and the vault counts `passwords`
    ([nix/modules/storage/usage.nix](../../nix/modules/storage/usage.nix)).
  - The Photos box runs `dd storage-ledger`
    ([nix/modules/storage/ledger.nix](../../nix/modules/storage/ledger.nix):
    two minutes after boot, then hourly) for `photos` and `libraries`.
    Libraries and disk images come from the gate's
    `/internal/storage/members`, trash included.
- **The fleet's count.** Every box runs `dd storage-view`
  ([nix/modules/storage/view.nix](../../nix/modules/storage/view.nix),
  every 10 minutes), which adds up the control plane's agreed state into
  `/var/lib/dd-storage/ledger.json` for its own gate.
  - A box's count is believed only for services the release's
    `fleet/boxes.json` roles say it runs: photos and libraries need the
    photos role, code needs forge, passwords needs vault.
  - Counts saturate rather than overflow.
- **The counts never fail a switch.** dd-storage-view and
  dd-usage-<service> (hourly) first run 2 minutes and 90 seconds after
  their timers start (`OnActiveSec`), not after boot; a switch never runs
  them (`restartIfChanged = false`); and the control plane or postgres not
  answering is "next time" (exit 0), with the last count standing
  ([ci.md](ci.md#when-a-release-does-not-land)).
- **Photos limits.** The Photos box sets each member's Photos limit from
  the fleet's count, and only from one under an hour old. Without a fresh
  count it leaves the limits as they are.
- **The allowance** is `allowanceGb` in
  [fleet/policy.json](../../fleet/policy.json) (200), read into
  `dd.storage.budgetGb`. It is what a member stores, not the copies the
  pool keeps. The gate holds uploads to it and shows the numbers in
  Settings, Storage (`/_dd/storage/mine`). An upload of unknown length is
  given all that is left and cut off past it.

## Disk images

Disk images (`dd image`, restic's repository format, through
`rustic_core` in [client/archive/](../../client/archive/), encrypted on
the member's machine) are in the `libraries` bucket under the prefix
`images-<member>`.

- nginx sends `files.<domain>/images/` to the gate's `/_dd/images`
  ([nix/modules/media/webdav-media.nix](../../nix/modules/media/webdav-media.nix)),
  which serves them with the same WebDAV code (`images` in
  [library.rs](../../servers/gate/src/library.rs)). A GET on a folder
  returns its JSON listing.
- Members only, and only their own images.
- A browser changes nothing there without a passkey shown in the last
  five minutes. A request carrying `x-dd-upload` counts as a change,
  whatever its method. A device token needs no passkey.
- DELETE moves into the trash, which goes after 30 days like a library's;
  a DELETE in it, or of it, deletes for good. Backups has **Empty trash**,
  and its delete a box to delete for good at once (the trash with it:
  restic's packs are not one image each).
- Images count against the allowance.
- The archive password is sealed to the member's devices beside the
  archives (`dd-devices.json`, [Where the keys live](#where-the-keys-live)),
  so Backups in Settings opens them where the app serves Settings
  ([client/web/src/archive.rs](../../client/web/src/archive.rs)).
- `/srv/images` and the old nginx WebDAV are gone: the copy into the
  bucket was checked member by member, and the dataset was destroyed.

## Configuration and state

```
nix/roles/library.nix          a library box: the bucket key from sops, dd.libraries, dd.transcode
nix/roles/media.nix            the Files and Movies & TV tiles; files.<domain>, whose /images/ goes to the gate
dd.libraries.enable, keyFile   the bucket, its key (LIBRARY_ID, LIBRARY_SECRET), the gate's config
dd.verify.library              { s3, bucket, keyFile } -> VERIFY_LIBRARY_S3, _BUCKET, the env file
dd.verify.demoLibrary          { id, key } -> VERIFY_DEMO_LIBRARY_ID, _KEY
dd.verify.tmdbKeyFile          posters on Movies & TV
dd.transcode.enable, source    the service; the only url prefix a session may fetch
dd.box.vaapi                   the GPU's render node, from the hardware report
dd.storage.budgetGb            the allowance, from fleet/policy.json allowanceGb;
                               null: no view, no ledger, no limit
```

`dd-transcode`'s environment: `TRANSCODE_BIND` (127.0.0.1:4190),
`TRANSCODE_PLAIN_BIND` (:4191), `TRANSCODE_DIR` (/run/dd-transcode),
`TRANSCODE_FFMPEG`, `TRANSCODE_SOURCE`, `TRANSCODE_VAAPI`.

State on a box is the bucket (libraries and disk images) and the fleet's
count (`/var/lib/dd-storage/ledger.json`). The gate keeps no state about
libraries: the keys are in entries, the objects in garage. The transcoder
keeps nothing past a session or a restart.

What a library box needs (memory, network, privacy labels) is
[nix/modules/library/needs.nix](../../nix/modules/library/needs.nix); see
[What each service needs from a box](hardware.md).

## Where things are

```
client/library/            the format (crypt.rs), sealing, a library's keys
client/media/              gate.rs: the gate from a device; mount.rs: rclone mount
client/web/src/library.rs  the browser's crypto, as wasm
client/cli/src/            librarycmd.rs, mediacmd.rs, storage_ledger.rs
servers/library/           the WebDAV gate's storage half: sigv4, presigning, dav.rs
servers/gate/src/          library.rs (who may), storage.rs (the allowance)
servers/home/web/library.js  opening, listing, carrying, transcode calls: shared by both pages
servers/files/web/         Files
servers/media/web/         Movies & TV, shelf.js
servers/transcode/         dd-transcode
client/archive/            dd image's restic repository, over the gate's /_dd/images
nix/modules/library/       libraries.nix, transcode.nix, needs.nix
nix/modules/media/         webdav-media.nix: files.<domain>: a redirect to Files,
                           and /images/ sent to the gate
nix/modules/storage/       garage.nix, ledger.nix (the Photos box's count),
                           view.nix (the fleet's count, every box), usage.nix
                           (forge and vault counts), cache-gc.sh
```

## Tests

- **Unit tests**: the format against rclone, both ways
  ([client/library/tests/rclone.rs](../../client/library/tests/rclone.rs));
  sealing to devices, root and boxes
  ([client/library/src/lib.rs](../../client/library/src/lib.rs)); the
  pieces and one file's block keys for a box; that a passkey is never
  sealed to; the shared secrets' signatures, a copy a box made refused
  ([client/account/src/lib.rs](../../client/account/src/lib.rs));
  block keys against NaCl's HSalsa20 vector, against the data key's own
  opening, and against a file rclone wrote
  ([client/web/src/library.rs](../../client/web/src/library.rs)); paths,
  dates and decoding in dav.rs, and against a bucket of its own there, two
  deletes for good of one thing counting it once and one library's trash
  never waiting on another's; the signing in lib.rs; the allowance in
  storage.rs, with what a delete for good frees.
- **`library`** VM test ([nix/tests/library.nix](../../nix/tests/library.nix),
  [library.py](../../nix/tests/library.py)): one box with garage and the
  full gate. A member made with `dd`, `dd library new` with its three
  folders, rclone as a crypt remote over the gate copying in and reading
  back, `dd` and rclone reading each other's files, nothing plain in the
  bucket or on disk, delete into the trash, the trash emptied after 30
  days, one thing deleted for good and then the whole trash emptied (and
  the library root refused), an upload in pieces, the demo's cookie reading its own library
  and nothing else, a stranger's token, no token, and a library id
  claimed in someone else's entry. Disk images under `/_dd/images`: put
  and read, the JSON listing, 404 for an empty folder, another member
  refused, delete into the trash and the trash emptied, no token 401; a browser without a fresh
  passkey refused a write, including a write dressed as a read with
  `x-dd-upload`. The allowance: a declared size too big refused before a
  byte moves, a chunked upload cut off at the room and not kept, what fits
  landing and then counting, a COPY that would not fit refused.
- **`transcode`** VM test ([nix/tests/transcode.nix](../../nix/tests/transcode.nix),
  [transcode.py](../../nix/tests/transcode.py)): a two-minute clip that
  needs encoding, written by rclone, served with ranges, its block keys
  made and sealed by [transcode-chunks.py](../../nix/tests/transcode-chunks.py).
  The session's length and first segment, the plain listener refusing
  root, urls outside the source refused, keys for another box refused,
  keys that do not cover every block refused,
  resuming and resuming past the end, DELETE really ending ffmpeg and its
  folder, one film per member, and the demo's sessions left alone. Two
  H.264 films with silent 5.1 AC-3: keyframes 10 s apart (encoded) and
  2 s apart (copied, as the log line says), each with every segment 5 s
  or less and under 2 MB, and nothing of the film or a whole session id
  in the journal.

Both are in `vmTests` in [flake.nix](../../flake.nix) and run in CI's
`vm_tests` matrix ([ci.md](ci.md)). Neither covers the window (pausing
ffmpeg), the GPU path, or the pages themselves: no VM test runs a
browser, so hls.js's limits and the player-error report are not played
in CI.

## Changing it

- **The format is rclone's.** A change in crypt.rs keeps the rclone tests
  passing both ways; a library someone mounts with plain rclone must keep
  opening.
- **The gate moves ciphertext.** Nothing on a box decrypts a name. A new
  method in dav.rs keeps to the library's prefix, refuses readers if it
  writes, and never deletes outright outside the trash: a delete there is
  a move into the trash, and only a delete in the trash is for good.
- **A new endpoint needs the gate in front.** Under `/_dd/dav/` it is
  already behind `allowed()`. Anything else on `files.<domain>` gets
  `auth_request`, as `/_dd/transcode/` does; the session paths are the one
  exception, and their id is the credential.
- **Plaintext stays in memory.** The transcoder writes only to its tmpfs,
  and a change to how much it keeps (KEEP, BEHIND, AHEAD, MAX_SESSIONS,
  WINDOW_BYTES) is checked against `MemoryMax` and the `requires.memory` in needs.nix.
- **Page changes** go to library.js, files.js, media.js or shelf.js and
  their copies under `app/web/_dd/` in the same pull request; the
  [UI guidelines](ui.md) hold.
- **The VM tests are the contract.** A change to the gate's rules, the
  WebDAV methods or the transcoder's API adds its case to
  `library.py` or `transcode.py`.
