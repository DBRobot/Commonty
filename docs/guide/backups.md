# Backups of your computers

Two kinds of backup keep things safe in Commonty: **disk images** you
make of your own computers, and the **box backups** that protect
everyone's data on the boxes.

## Disk images

A disk image is a full copy of a computer's disk, such as an old laptop
before you wipe it. It is locked on that computer before it is sent,
and kept on the boxes.

### Making one

Disk images are made with Commonty's command-line client on the
computer whose disk you copy (see the
[developer pages](../dev/client.md)). Saving the same disk again sends
only what changed. The app cannot make a disk image yet.

### Seeing, downloading and deleting

Open Settings, then **Backups**, in the app, or in a browser you allowed
from the app ([Using a browser](browser.md)). Settings opened straight
from a box cannot open your images.

- **Your disk images** lists each one with its size and when it was
  saved.
- **Download** saves one to this computer, with progress shown.
- **Delete** asks you to confirm, then removes the image and frees its
  space.

If the list says your images do not open on this device yet, save one
again from the computer that made them.

### Privacy

| The box | Outside |
| --- | --- |
| 🔒 **Locked** | 🏠 **Stays home** |

- **What the box keeps:** your images, locked on the computer that made
  them. It cannot read the files in them, or their names.
- **What the box can see:** how big the pieces are and when they were
  sent.
- **Outside companies:** none. Images go between your devices and the
  boxes over Commonty's own network.

Every service's ratings are in the [Privacy policy](../policies/privacy.md); how Commonty is protected is in [Security](../policies/security.md).

### Security

- **The key stays on your devices.** The images' password is locked to
  your devices and your [recovery key](recovery-key.md), and is believed
  only when one of your own devices signed it. The app opens it for the
  Backups page.
- **Only you.** Other members, guests and the demo cannot reach your
  images.
- **From a browser,** nothing can be changed without the app or a
  passkey shown in the last five minutes.
- **If a box were taken over,** the attacker would get locked images,
  their sizes and dates.

### Deleting and keeping

- A deleted image's pieces go to a trash for **30 days**, then they are
  deleted, and they are gone from every box within **90 days**.
- Images count against your storage allowance.
- **Leaving:** your images are kept 30 days in case you come back, then
  deleted ([Leaving](leaving.md)).
  The Leave tab in Settings lists every disk image, each with its own
  Download.

## Box backups

These protect what is kept on the boxes. You do not have to do
anything, and you cannot restore from them yourself.

- **Every night**, each box backs up its own important state: the
  account directory (which holds your locked keys), the databases of
  Photos, Passwords and Git, Git's repositories, game servers and
  worlds, and Commonty's network.
- **Locked by the box** with its own password before the backup leaves
  it, and kept in the shared storage, which holds two copies on
  different boxes. No other box can read another's backup.
- **Your files, photos and disk images** are not in these backups. The
  shared storage keeps two copies of them on different boxes, and a
  week of snapshots.
- **How long:** a backup is kept under five weeks, and its snapshots a
  week more. That is why something you delete is gone from every box
  within 90 days.
- **Outside companies:** none. The backups stay on Commonty's boxes.
- **When a backup fails,** the box's owner is emailed, and Settings,
  **Network** shows when each box last backed up.

## Limits

- Making a disk image needs the command-line client; the app cannot do
  it yet. There is no app for Mac or iPhone.
  <!-- coming: no Mac or iPhone app yet -->
- Restoring a disk image onto a computer is done with the command-line
  client too.
- Today the boxes are in one house, so a fire or flood there could take
  every copy at once. Copies in other houses need more boxes.
