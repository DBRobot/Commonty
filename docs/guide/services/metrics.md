# Metrics

## What it is

How the boxes are doing: whether each is up, how busy and how hot it
is, its drives, its storage and its backups, and anything that needs
looking at. It holds figures about the boxes, not about your data.

## Using it

Open **Metrics** from the home page, in the app or in a browser. It works
in a plain browser: it holds no keys. You need to be on Commonty's
network.

- **The fleet:** every box, with its processor, memory, temperature,
  network and power, and whether it is answering.
- **A box:** click its name for its charts over time, what runs on it,
  and the health of its drives.
- **Alerts:** what needs looking at now, what happened in the last 7
  days, and what is watched.
- **Storage:** where the space goes on every box, the backups, and the
  shared storage.
- **Explore:** ask any figure the boxes record.

## Privacy

| The box | Outside |
| --- | --- |
| 👁 **Open to the box** | 📖 **Read outside** |

- **What the boxes keep:** figures about themselves: load, memory,
  temperatures, disks, network traffic, which services are running, and
  when each last backed up. They are not locked; every member can read
  them here.
- **What is not in it:** your files, photos or messages, and how much
  each member stores. Your storage figures are in Settings, **Storage**,
  for you alone.
- **Where a box is** shows only as random-looking ids. To work them out,
  each box asks ipinfo.io every hour, which sees the house's internet
  address.
- **Outside companies:** none for the figures, and nothing about any
  member. When a drive or a storage pool is in trouble, the box emails
  its owner through Cloudflare, which can read the email: it names the
  box and the problem. ipinfo.io sees each box's internet address, as
  above.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- **Who can see it:** members. Not guests, and not the demo: the
  fleet's figures are members' business.
- **Read only.** The page can only ask for figures; nothing on it changes
  a box.
- **If a box were taken over,** the attacker could read and change the
  figures, which would hide its own trouble. Your data is not in them.

## Deleting and keeping

Each box keeps its own figures 30 days. The history is also kept in the
shared storage: every figure for 180 days, then in less detail for up to
five years. Nothing in it belongs to
one member, so nothing is deleted when you leave.

## Limits

- Only drive and pool alarms send mail today; the rest show here when
  the page is open.
- A box that is off or cut off shows greyed out, with no figures, until
  it is back.
