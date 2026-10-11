# Hosting a box

A box is a computer in a member's home that runs Commonty for everyone.
Hosting one is one of the two ways to become a member ([Joining
Commonty](joining.md)).

## What counts as real hardware

A box has to add real storage or computing power to the network. A
device that adds next to nothing, a Raspberry Pi for example, does not
count.

Today a box also needs:

- a 64-bit Intel or AMD PC (x86-64), with UEFI;
- a TPM 2.0 security chip, which locks and unlocks its disks;
- an SSD to start from, plus whatever storage it adds;
- a wired network connection at home. Some built-in network cards are
  not supported yet, so a box may need a supported USB network adapter;
- power and internet, all the time. A box never sleeps.

Setting a box up is done with David for now.

## What you agree to

From the [Terms of use](../policies/terms.md#hosting-a-box). If you host a
box, you agree to:

- keep it running and up to date;
- never tamper with it, or with the software Commonty runs on it;
- never try to read or recover anything it holds.

A box that breaks these rules is removed from the network.

## What the box can and cannot see

A box never holds your keys, or anyone else's. Keys are only on members'
devices. What a box keeps for others falls into three kinds, using the
[Privacy policy's](../policies/privacy.md) ratings:

- 🔒 **Locked:** Files, Movies & TV, disk images, chat history and device
  names. The box keeps them but cannot read them, not even file names.
- 🔏 **Locked, labelled:** Passwords and Photos. The contents are locked,
  but the box sees who they belong to, when they changed and how big
  they are.
- 👁 **Open to the box:** accounts (names and public keys, nothing
  secret), the network's list of devices, storage counts, metrics, game
  worlds and logs, Git repositories (public anyway), a chat message while it is answered, a web search, ad
  blocking's look-ups, a film being played, and the front door's request
  logs.

Whoever looks after a box, or breaks into it, can see what the box can
see. That includes you, so the terms ask you never to look.

## What protects it

- **Locked disks.** A box's disks are encrypted and unlocked by its own
  security chip. A box unlocks by itself only when it starts at home; one
  that starts somewhere new waits for you to say so in Settings, Network,
  **Moved or stolen?**. A disk pulled out of a box is unreadable.
- **Signed releases.** A box runs only releases built from Commonty's
  public code and signed. Two boxes must build each release to the same
  result before it ships. You do not install software on it yourself.
- **No trusted box.** No box is trusted with anyone's data, including
  yours. A box that is stolen or misused can hand over only locked data,
  plus what is open to it as listed above.

See the [Security policy](../policies/security.md).

## Limits

- Only x86-64 PCs are supported today.
- Some network cards cannot unlock the disks at start-up, as above.
