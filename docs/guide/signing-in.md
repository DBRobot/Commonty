# Signing in

## In the app

The app is a device of your account. It signs everything it asks with
its own key, so it never asks you to sign in. Once a device is added
([Your devices](devices.md)), it stays signed in until you remove it or
press **Sign out on this device**.

## In a browser: one sign-in with your passkey

1. Go to the sign-in page and type your name.
2. Your browser asks for your passkey. Use it.

That one sign-in covers every service in that browser: home, Games,
Git, Metrics, Friends, Settings, and Passwords' own sign-in. Nothing asks you
again. It lasts 8 hours, then you sign in once more.

Pages that hold your keys (Files, Movies & TV, Chat, Photos, Passwords,
and Backups and device names in Settings) open through the Commonty app
on that computer, not from a box. That is one click in the app, not
another sign-in. See [Using a browser](browser.md).

Passwords is the one place you type something more: your vault opens with
its own master password, on your own device. Signing in only proves who
you are.

## What a passkey is

A passkey replaces a password. It is a pair of keys made by your phone,
computer or password manager:

- the private half never leaves it, and it signs in by answering a
  one-time question;
- the boxes keep only the public half, so there is no password on any
  box to steal.

Where it lives depends on where you made it: your phone or computer, or a
password manager that syncs it between your devices. Settings shows
"synced passkey" for one that syncs.

On Commonty a passkey only signs you in. No key of yours is made from it
or locked to it, so a passkey can never open your files, even if a box
asks it to. Your keys are on your devices.

You make a passkey from the app (**Set up a passkey**). You can have
several, and remove one in Settings, under Devices.

## Signing out

- **This browser:** your name in the bar, then **Sign out**.
- **The app on this device:** **Sign out on this device** on the app's
  own screen. It forgets your name and the keys it held for Photos here.
- **Everywhere else:** Settings, Devices, **Sign out everywhere else**.
  It ends every other sign-in at once, on every box, including Passwords
  and your email page. You may be asked for your passkey again first. If
  your account's main key is on a device, not a passkey, do it in the app
  on that device, or with `dd device sign-out`. You can also
  sign out one sign-in at a time from the list under **Signed in now**.
- **Removing a device or passkey** also ends every sign-in it made, and
  signs you out of Passwords everywhere.

## Privacy

Your account is 👁 **Open to the box** and 📖 **Read outside**, with
nothing secret in it. The box keeps your name, your devices' and
passkeys' public keys, and a list of where you are signed in: what kind
of browser, and when it was first and last used. Sign-in pages pass through Cloudflare, which can read them.
More in the [Privacy policy](../policies/privacy.md).

## Limits

- A box does not keep a passkey's sign-in counter, so it cannot spot a
  copied passkey that way.
- Pages that hold keys need the app on the same computer. There is no
  Mac or iPhone app yet.
