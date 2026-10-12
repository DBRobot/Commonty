# Your devices

Every phone or computer with the Commonty app is a device of your
account. Your keys live on your devices, and nowhere else.

## What a device holds

- **Its own device key**, kept in the system's keystore (on Android, in
  the app's private storage). It signs everything the app asks a box.
- **Your libraries' keys**, locked to that device key. They open Files,
  Movies & TV and your disk images.
- **Your Photos password**, made by the app at random. Nobody types it.
- **Your account's main key**, only on the device you made the account
  on (or recovered it on). It is what adds and removes devices.

Anyone who can read your device's own files can read these keys. Keep
your devices locked.

## Adding a device

You approve a new device from the device that holds your account's main
key.

1. Install the app on the new device, from [commonty.org/download](https://commonty.org/download).
2. On the device with the main key, open Settings, then **Devices**, then
   **Add a device**. It shows a code and a QR code.
3. On the new device, choose **Have a code?** and type the code. If you
   scan the QR code instead, it opens a page that shows the code to type
   into the app. A code lasts 10 minutes.
4. Both screens show six digits. Check they match, then press **Approve**
   on the old device. If they do not match, press **It isn't mine**.

The digits are worked out on each device, so a box that swapped the new
device's key could not make them match. The box only carries the
messages. Once approved, the new device gets your libraries' keys and
signs in by itself.

## Naming a device

Devices start as "Device 1", "Passkey 2" and so on. In Settings,
Devices, press **Show names**, then the pencil next to a device. Names
are locked with your library key, which only your devices hold. In a
browser, **Show names** has the app on this computer open the tab in the
same browser ([Using a browser](browser.md)).

## Removing a device

In Settings, Devices, press **Remove** next to the device, then confirm.
The device's sign-ins end at once, Passwords signs it out, and it no
longer opens anything new.

You can remove a device only from the device that holds the main key. If
you have lost that one, use [your recovery key](recovery-key.md).

Removing a device does not wipe it. The boxes refuse it from then on,
but whatever it already downloaded stays on it. Your libraries' keys are
not changed when a device is removed, so a copy of a key it kept would
still open data it could get some other way.

## Privacy

Device names are 🔒 **Locked** and 🏠 **Stays home**. Your account (which
devices you have, as public keys, and when each was last used) is
👁 **Open to the box** and 📖 **Read outside**, with nothing secret in
it. More in the [Privacy
policy](../policies/privacy.md).

## Limits

- Only the device holding the main key can add or remove devices. Moving
  the main key to another device is not built, except by starting over
  with your recovery key.
- There is no app for Mac or iPhone yet, so those cannot be devices.
