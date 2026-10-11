# Your home network and Wi-Fi

Commonty has a private network of its own, joining every member's
devices and every box. The boxes in your house are on your house's
Wi-Fi or cable too. This page covers both.

## Commonty's network

### What it does

The Commonty app joins your device to Commonty's network when it starts.
Your device then reaches the boxes directly by name, wherever you are,
without going through the public front door. Game servers, Git, Metrics
and the services' data all go this way.

- **On a computer,** the app points your browser at Commonty's network
  for Commonty's own names only. Everything else your browser does goes
  the usual way.
- **On an Android phone,** the app asks once to set up a VPN. The phone's
  browser and other apps then reach Commonty's names. The VPN carries
  only Commonty's addresses, and the phone's name lookups.

It is built on Tailscale's networking engine, with the control server on
one of Commonty's boxes.

### Privacy

| The box | Outside |
| --- | --- |
| 👁 **Open to the box** | ✉️ **Envelope seen** |

- **What the box running the network keeps, and can read:** which
  devices you have, each one's name on the network (your computer's own
  name, or `phone-xxxx`), its network address, and when it was last
  online.
- **Traffic is sealed end to end** between your device and the box. When
  the two cannot reach each other directly, Tailscale's public relays
  carry it, sealed. They see your devices' internet addresses talking,
  not what is said.
- **Joining and connecting** go through Commonty's front door, which
  passes through Cloudflare. What passes is sealed by the network's own
  encryption, but Cloudflare sees when your devices connect.
- **Name lookups on a phone:** with the VPN on, the phone's lookups for
  other sites go to Quad9, unfiltered, through the network's resolver,
  encrypted on the way. Quad9 sees every name looked up.

Every service's ratings are in the [Privacy policy](../policies/privacy.md); how Commonty is protected is in [Security](../policies/security.md).

### Security

- A device joins with a single-use key the gate gives only to a device
  admitted to your account.
- When you leave or are removed, your devices are taken off the network
  at once.
- Your devices reach only what is meant for members: a game server's
  ports open only to the devices its owner invited.
- **If the box running the network were taken over,** the attacker could
  see your devices and when they were online, and could add a device of
  its own to the network. It still could not sign in as you or open your
  locked data.

## Your house's boxes and Wi-Fi

Open Settings, then **Network**.

- **Your home's boxes** lists each box in the house: online or offline,
  whether it has a cable to the router or is on Wi-Fi (with its signal),
  whether it is up to date, and when it last backed up.
- **Wi-Fi** shows the network name the boxes use.

### Changing the Wi-Fi

Changing your Wi-Fi password on the router? Put the new details in here
**first**. A box on Wi-Fi cannot hear about the change once it is cut
off.

1. Press **Change Wi-Fi details**.
2. Type the network name and password.
3. Press **Save with passkey** and confirm.
4. Each box tries the new details. The page says when every box has
   joined. A box where they do not work goes back to the old Wi-Fi, and
   the page names it.

Each box checks that the change was signed by a member's own passkey,
so no box can change another's Wi-Fi. The password waits on each box
only as long as it takes to apply it. The house's network name is shown
only to members.

### A box moved or stolen

Each box unlocks its disks by itself when it starts at home. A box that
starts somewhere new waits until a member approves it with their passkey.
**Moved or stolen?** opens that page, where you can let a moved box in or
mark a box as stolen. A box marked stolen is never let in.

## Adding a device

Settings, **Devices**, **Add a device** shows a QR code and a code. Scan
it with the new device, or type the code into the app's **Have a code?**,
check the six digits match, and press **Approve**. It joins Commonty's
network by itself. See [Your devices](devices.md).

## Ad blocking

If your house has a box with ad blocking, its switch is here too. See
[Ad blocking](services/ad-blocking.md).

## Limits

- There is no app for Mac or iPhone yet, so those cannot join Commonty's
  network.
- Relays are Tailscale's until Commonty has a public box of its own.
- Only boxes in your own house appear under **Network**.
