# Smart home

## What it is

Your home's lights, sensors, switches and plugs, run by a small computer
in your house: the **home controller**. It is a Raspberry Pi running
[Home Assistant](https://www.home-assistant.io/), set up and kept up to
date by Commonty. Your devices, automations and history stay on the Pi.
Commonty's boxes carry its traffic locked and keep its backup locked, and
cannot read either.

Any device Home Assistant supports works: Wi-Fi, Zigbee, Thread and
Matter. Zigbee and Thread need a radio stick plugged into the Pi.

## What you need

- A **Raspberry Pi 4 or 5**, with its power supply.
- A **microSD card** for it. Everything on the card is erased.
- A **network cable** from the Pi to your router. The Pi has to be wired,
  not on Wi-Fi.
- **One of Commonty's boxes on the same home network.** The Pi's card
  unlocks only while that box answers, so a Pi carried out of your house
  stays locked.
- A computer with the **Commonty app** (Windows, macOS or Linux) and a
  way to put the card in it.

## Setting it up

In the Commonty app on your computer, open **Settings**, then **Network**,
and under **Home controller** press **Set up a home controller**. A
setup window opens and takes you through it:

1. **Choose a card.** Put the microSD card in your computer and pick it.
   Only removable cards and USB disks are offered.
2. **Write the card.** The app downloads the home controller, checks it
   against the release's signature, writes it, and reads it back to make
   sure. This takes a few minutes.
3. **Start the Pi.** Put the card in the Pi, plug in the network cable,
   then the power. On its first start the Pi locks its card to your home
   and installs itself. This takes several minutes, and it restarts once.
4. **Pair it with your account.** The app finds the Pi on your home
   network by itself. Press **Pair**. If you sign in to Commonty with a
   passkey, your browser opens and asks for it.
5. **Open Smart home.** Your home controller is set up. Add your devices
   to it in Home Assistant.

If the card was written another way, such as with Raspberry Pi Imager,
choose **My card was written another way**: the app adds the key it
pairs with to the card, and nothing else, then you put the card back in
the Pi.

To move to a new card or a new Pi, write a new card and choose **Start
from a backup** when pairing: your home comes back as it was at its last
backup.

## Using it

- **In the app:** open **Smart home** from the home page. The app signs
  you in with your device's own key, with nothing to type.
- **On a phone:** use the Android app. There is no app for iPhone yet.
- **In a browser:** while your device is on Commonty's network, open your
  home's address and sign in with your passkey.

Then use it as Home Assistant: add devices, make automations and scenes,
and look at their history. Home Assistant's own phone apps are not
supported; use Commonty's.

**Who is in.** The person who paired the controller is in. Anyone else
with a Commonty account signs in to it and presses **Ask to be let in**;
anyone already in sees them under **People** and can let them in or turn
them down, and can remove someone later. Each person is their own user
in Home Assistant. People outside Commonty cannot get in.

**Updates** install on their own. Under **People**, **Hold** keeps your
home on the release it runs until you let updates go again.

## Privacy

| The box | Outside |
| --- | --- |
| 🔒 **Locked** | 🏠 **Stays home** |

- **What the box keeps:** one backup of your home, locked on the Pi
  before it leaves, so only the devices of the people in can open it.
  The box cannot read it.
- **What the box can see:** that your account has a home controller, its
  random name, how big its backup is and when it changes. Your devices,
  automations and history are never on a box.
- **Its traffic:** between your devices and the Pi it goes over
  Commonty's own network, locked end to end. The boxes pass it on and
  cannot read it.
- **Its name:** the Pi gets a certificate for its random name (such as
  `abcd….home.<domain>`) from Let's Encrypt, and certificates are public.
  The name says nothing about you or your home.
- **Devices you add:** a device or service you connect in Home Assistant
  may talk to its maker, as it would anywhere. That is between you and
  them.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- **The Pi is yours.** No box holds a key to it or can sign in to it.
  The Pi decides itself who may open it, from the keys of the people it
  was paired with or let in.
- **Locked to your house.** The card is locked, and it unlocks only with
  the help of the Commonty box on your home network. A stolen Pi or card
  does not open.
- **Only what Commonty signed runs on it.** The Pi takes a new release
  only when it is signed and two boxes have checked it.
- **The app knows your Pi.** It remembers the Pi's key from pairing and
  refuses anything else that answers to its name.
- **Who can reach it:** the people in, on Commonty's network. Guests and
  the demo have no Smart home.

## Deleting and keeping

- **History:** Home Assistant keeps **30 days** of history on the Pi.
- **Backups:** every night, and whenever someone is let in or removed.
  Each one replaces the last.
- **Leaving:** the backups of the home controllers you set up are deleted
  from the boxes when you leave ([Leaving](../leaving.md)). Within
  **90 days** nothing of them is left on any box. The Pi itself is yours
  and keeps running; nobody else is affected.

## Limits

- It needs one of Commonty's boxes on your home network.
- The Pi must be wired to your router.
- There is no app for iPhone yet, and Home Assistant's own apps are not
  supported.
- A backup opens only with a device of someone who was in when it was
  made. If every one of those devices is lost, it cannot be opened.
