# Photos

## What it is

Your photos and albums, end-to-end encrypted. Photos is
[Ente](https://ente.io), run on Commonty's own boxes, with Commonty's
sign-in.

## Using it

- **In the app:** open **Photos** from the home page. It signs in by
  itself.
- **In a browser:** open Photos, press **Open**, and allow the browser in
  the app ([Using a browser](../browser.md)).
- **On a phone:** use the Android app. There is no app for iPhone yet.
  <!-- coming: no Mac or iPhone app yet -->

Then use it as Ente: upload photos, make albums, and hide photos and
albums. **Confirm it's you** in Ente's menu is
answered by the app.

You never type a Photos password. The app makes one at random, keeps it
with your device's keys, and passes it, locked, to your other devices.

## Privacy

| The box | Outside |
| --- | --- |
| 🔏 **Locked, labelled** | ✉️ **Envelope seen** |

- **What the box keeps:** every photo, its details and your albums,
  locked on your device. The box cannot see the pictures, where or when
  they were taken, or album names.
- **What the box can see:** your Photos account (`<name>@users.<domain>`),
  how many files you have, their sizes, when they were added, and which
  albums are shared with whom.
- **Outside companies:** your photos go between your device and the box
  over Commonty's own network, and no other company carries them. You
  never get a sign-in code by email: the gate confirms your Photos
  account itself. Photos sends no email at all.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- **Keys on your devices.** Your Photos password is locked to your
  devices and your [recovery key](../recovery-key.md). A copy is believed
  only when one of your own devices signed it.
- **The code comes from the app.** Ente's web client is built into the
  app's signed release. No box serves it to you, so a box cannot change
  it to read your password.
- **Session tokens are stored hashed** on the box, so a copy of its
  database signs nobody in.
- **Who can reach it:** only members, on Commonty's network. Guests
  cannot. The demo sees its own gallery on the home page, not Ente.
- **If a box were taken over,** the attacker would get locked photos and
  the labels above. Older accounts whose Photos password came from a
  passkey are moved to an app-made password the first time Photos
  opens; after that the old password opens nothing.

## Deleting and keeping

- **Trash:** Ente's trash keeps a deleted photo **30 days**. Ente then
  removes its stored file 45 days later, and the storage's week of
  snapshots follows. A deleted photo is gone from every box within
  **90 days**.
- **Leaving:** your account is kept for 30 days in case you come back,
  then deleted, and its files are gone within 90 days. See
  [Leaving](../leaving.md).
- **Taking it with you:** download an album or a selection from Photos.
  <!-- coming: every photo as a zip, through the app, from the Leave page -->
  The Leave tab in Settings downloads every photo as one zip, through the
  app.

## Limits

- Ente's own phone and desktop apps cannot be used: nobody knows the
  Photos password to type into them.
- The app does not upload a phone's camera roll by itself; you upload
  from the Photos page.
- Ente's account settings, plans and its own sign-out are hidden.
  Signing out is Commonty's.
- Photos in the bin still count against your allowance until they are
  gone.
