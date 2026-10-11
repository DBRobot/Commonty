# Passwords

## What it is

A password manager for each member. It is
[Vaultwarden](https://github.com/dani-garcia/vaultwarden), a server for
Bitwarden's apps, run on Commonty's own boxes. Your vault is locked on
your device with your master password.

## Using it

- **In the app:** open **Passwords** from the home page. The app signs
  you in at the gate by itself.
- **In a browser:** open Passwords, press **Open**, and allow the browser
  in the app ([Using a browser](../browser.md)). The browser signs in
  once with your passkey, and you unlock the vault with your master
  password.
- **On a phone:** use the Android app. There is no app for iPhone yet.
  <!-- coming: no Mac or iPhone app yet -->
- **Bitwarden's own apps and browser extension** can sign in too, while
  your device is on Commonty's network: choose a self-hosted server at
  `vault.<domain>`, then single sign-on. The sign-in passes through the
  gate.

The first time, you choose a **master password**. It unlocks your vault
and only you know it. Nobody can reset it for you.

Then use it as Bitwarden: save logins, cards and notes, fill them in,
share with other members in an organisation, set up emergency access,
and make **Send** links. A Send link can be opened by anyone, even
outside Commonty.

## Privacy

| The box | Outside |
| --- | --- |
| 🔏 **Locked, labelled** | ✉️ **Envelope seen** |

- **What the box keeps:** your vault, locked with your master password on
  your device. It cannot read your items.
- **What the box can see:** your account (`<name>@<domain>`), when items
  change, how many there are, organisation names and who is in them,
  and the devices you sign in from.
- **Site icons are never fetched,** so the box does not learn which
  sites are in your vault. Breach checks for a password ask a copy of
  the breach list kept on the box, not an outside service.
- **Email:** sign-in codes and notices (such as emergency access and
  invitations) are sent to `<name>@<domain>`. They leave through
  Google's mail service and Cloudflare forwards them to your own address
  ([Your email address](../email.md)). Both see each email.
- **Send links:** someone outside opening a Send reaches it through
  Cloudflare. What passes is locked; the key is in the part of the link
  after `#`, which never reaches a server.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- **Your master password stays on your device.** The box holds a locked
  vault. Signing in proves who you are; only your master password opens
  the vault.
- **The web vault comes from the app.** Its code is in the app's signed
  release, so a box cannot change the page you type your master password
  into.
- **Sign-in is Commonty's only.** Passwords takes no other sign-in. The
  gate signs you in with your passkey. Removing one of your devices or
  passkeys signs every Passwords app of yours out.
- **Stored tokens are hashed,** so a copy of the box's database signs
  nobody in.
- **Who can reach it:** members, on Commonty's network. From outside, only
  Send links answer. Guests and the demo have no Passwords.
- **If a box were taken over,** the attacker would get locked vaults and
  the labels above. A Bitwarden app you installed is Bitwarden's code,
  not the box's.

## Deleting and keeping

- **Trash:** a deleted item stays in the vault's trash **30 days**. The
  vault's event log is kept 30 days too.
- **Backups:** the box's nightly backups keep a deleted item up to five
  more weeks, and their snapshots a week. It is gone from every box
  within **90 days**.
- **Leaving:** your vault is kept 30 days in case you come back, then
  your account is deleted. If you are the last owner of a shared
  organisation, hand it to someone first, or the deletion waits.
  See [Leaving](../leaving.md).
- **Taking it with you:** use **Export vault** in the vault's Tools.
  <!-- coming: the Leave page opens the vault's own Export screen -->
  The Leave tab in Settings opens the vault's own Export screen, where you
  type your master password.

## Limits

- If you forget your master password, your vault cannot be opened. Your
  recovery key does not open it.
- Bitwarden's own apps work only while your device is on Commonty's
  network.
- Attachments and Sends count against your storage allowance.
