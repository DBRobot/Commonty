# Files

## What it is

Your own folders and files, kept on the boxes. Names and contents are
locked on your device before they leave it, so no box can read them.

## Using it

- **In the app:** open **Files** from the home page.
- **In a browser:** open Files. The page says it opens through the
  Commonty app; press **Open** and allow the browser in the app
  ([Using a browser](../browser.md)).
- **On a phone:** use the Android app. There is no app for iPhone yet.

Then:

- **Upload:** drop files anywhere on the page, or press **Upload**.
  Large files go up in pieces.
- **Folders:** **New folder**; open one to go into it.
- **Preview:** click a file to see it. Files up to 64 MiB preview.
- **Download:** open a file's menu, or select several and press
  **Download**.
- **Rename and move:** from a file's menu, or **Move** for a selection.
- **Edit and sign a PDF:** open a PDF and press **Edit** or **Sign**.
  You can write on it, highlight, draw, comment, change its own words,
  fill in and add form fields, sign it, redact, find, and rotate, reorder,
  add or split its pages. A scanned page can be read so its words can be
  searched and copied. **Save** replaces it and puts the old one in the
  trash; **Save as a copy** keeps both.
- **Delete:** **Move to trash**. Open **Trash** to see what is there and
  **Restore** it, or **Delete forever** to delete it at once. **Empty
  trash** deletes everything in it at once. Both ask first, since they
  cannot be undone.

Files and [Movies & TV](movies.md) are one library: films and shows are
in its `Movies` and `Shows` folders, and [Chat](chat.md) keeps your
conversations in it too.

## Privacy

| The box | Outside |
| --- | --- |
| 🔒 **Locked** | 🏠 **Stays home** |

- **What the box keeps:** your files, locked. It cannot read their names
  or contents, and it never holds the key.
- **What the box can see:** how many files there are, how big each is,
  when each changed, and when you fetch one. It counts your bytes
  towards your allowance.
- **Outside companies:** none. Your files go between your device and the
  boxes over Commonty's own network.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- **Keys on your devices.** Your library's key is locked to your devices
  and your [recovery key](../recovery-key.md), never to a box or a
  passkey. The app opens it and hands it to the page.
- **The page comes from the app.** Its code is in the app's signed
  release, not sent by a box, so a box cannot change it to take your key.
- **Editing stays on your device.** A PDF is opened, changed and locked
  again in the page; the box only ever gets the locked file.
- **Signing.** Signing with your key makes a signature any PDF reader can
  check: who signed, and that nothing changed afterwards. Your signing key
  is made on each device you sign from and never leaves it. The pictures of
  your signature you keep are in your library, locked like your files.
  Readers say no authority vouches for the name; you do.
- **One door.** Every read and write goes through the gate on the box,
  which lets in only you. Other members, guests and the demo cannot open
  your library. The demo has a library of its own with nothing private
  in it.
- **If a box were taken over,** the attacker would get locked files and
  their sizes and dates, not names or contents.

## Deleting and keeping

- **Trash:** a deleted file stays in the trash **30 days**, then the box
  deletes it. Saving over a file puts the old version in the trash too.
  **Delete forever** and **Empty trash** delete at once, and give the
  room back to your allowance straight away.
- **Backups:** the storage keeps a week of snapshots, so a deleted file
  is gone from every box within about 37 days, and always within
  **90 days**.
- **Leaving:** everything is deleted at once when you leave. If you are
  removed, it is kept, locked, for 30 days, then deleted. See
  [Leaving](../leaving.md).
- **Taking it with you:** download files from the page.
  The Leave tab in Settings downloads Files and Movies & TV as one zip.

## Limits

- Files is for your own use. Sharing a folder with another member is not
  built yet.
- Your files count against your storage allowance (200 GB across every
  service). When it is full, uploads stop; nothing is deleted.
- In a browser on a Mac or iPhone, Files does not open: there is no app
  there yet.
- **Older accounts:** before keys came only from devices, a library key
  was also locked to your passkey. The app removes that lock, but an old
  copy of your account record still holds it, and a box could ask your
  passkey to open it. Making a new passkey and removing the old one, in
  Settings, closes this ([Your devices](../devices.md)).
