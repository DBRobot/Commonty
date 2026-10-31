# Using a browser

Every part of Commonty works in an ordinary browser. Some parts open
straight from a box. The parts that hold your keys open through the
Commonty app on the same computer.

## What opens directly

These need nothing but a browser:

- the public site, these docs, the demo and [the app's download page](https://commonty.org/download);
- a Passwords Send link someone gave you;
- once you sign in with your passkey: home, Games, Git, Metrics, Friends,
  Ad blocking and Settings. In Settings, your disk images, your device
  names and the Leave tab's downloads hold keys, so a button there has
  the app open them in your browser, as below.

## What opens through the app

Files, Movies & TV, Chat, Photos and Passwords hold your keys, and so
do Backups, device names and the Leave tab's downloads in Settings.
Their code comes with the app you installed, never from a box, so a box
cannot change it to steal a key. The app serves them to its own window,
and to your browser on the same computer once you allow it.

The first time, you link your browser to the app once, for all of them:

1. Open the page in your browser, for example Photos, and press **Open with
   the Commonty app**. In Settings, press **Open Backups**, **Show names**
   or, on the Leave tab, **Open them**.
2. Your browser shows a 6-digit code, and the app comes up with
   **Link this browser?** and a code.
3. Check the two codes match, then press **Link this browser** in the app.
   If they do not match, or you did not ask, press **Not me**.
4. The page opens in your browser.

After that, every one of these pages opens in the app by itself, with no
code and no button: Photos, Passwords, Files, Movies & TV, Chat and Mail,
from a link, a bookmark or the address bar, even after the app or the
computer restarts. Settings, Backups included, opens the app's copy
straight away, with nothing to press.

A linked browser is also signed in to your boxes by the app: Games, Git,
Metrics and the rest open without your passkey. Open each from one of the
app's pages once (the bar or the home page); from then on that box signs
you in through the app by itself, from a link or a bookmark too. Signing
out in the browser stops this until you next open a box from the app's
pages.

To stop a browser opening them, and being signed in by the app, open
the app's **This device** screen and press **Forget** beside it under
**Browsers you let in**. Signing
out in that browser forgets it too. A forgotten browser asks for a code
again next time.

If the app does not answer, it is not installed or not signed in on this
computer: the page says so and offers to get the app.

Good to know:

- The app only links a new browser if one of these pages asked in the last
  two minutes, and only after your click. If two browsers ask at once,
  both are refused and you start again.
- The address your browser opens holds no secret. What it keeps is under
  a name only the browsers you let in know.
- The app has to be running for these pages to open, and it is unless
  you choose **Quit**. While it is closed, another program on the
  computer could take its place and try to catch your browser's pass.
  **Forget** ends a pass at once if you are ever unsure.
- The app trusts your browser with what it opens, as you already trust it
  with your passkeys. A program that can read your own files could read
  the app's keys too; none of this protects against that.

## The app keeps running

So these pages always have something to open them, the app keeps running
when you close its window:

- **On a computer**, it stays as an icon next to the clock. Click it, or
  choose **Open Commonty** from its menu, to bring the window back.
  **Quit** in the same menu stops it. It also starts by itself, without a
  window, when you sign in to the computer, and if it ever stops by
  itself (a crash, or another program ending it) it starts again a few
  seconds later. Only **Quit** keeps it stopped. If your desktop shows no
  icons next to the clock, open Commonty again to get its window back.
- **On an Android phone**, it keeps running with a quiet notification,
  and starts again when the phone restarts. After a restart, tap the
  notification once to connect. The first time, the app asks to show
  notifications and to be left out of battery saving; saying no is fine,
  but Android may then stop it after a while.

To stop it starting by itself, open the app's screen for this device
(**This device** under your name in the app) and turn off **Start
Commonty when you sign in to this computer** (on a phone, **when the
phone starts**). On a phone that also stops it running in the
background.

## On a phone

On Android, use the app. Its window has every page.

## No Mac or iPhone app yet

There is no Commonty app for Mac or iPhone yet. On those, you can use
everything under [What opens directly](#what-opens-directly), and nothing
that holds a key:

- no Files, Movies & TV, Chat, Photos or Passwords;
- in Settings, no Backups and no device names;
- no adding a device, or leaving, if your account's main key is on a
  device (it is, if you joined with an invite code);
- no joining a game server, which needs the app's network.

Use a Linux, Windows or Android device for those. Your data is not
affected: it waits, locked, until you open it from a device with the
app.

## Privacy

Pages the app serves come from your own computer, and what they ask the
boxes goes over Commonty's own network. While the app runs on a computer,
the browser there reaches the boxes the same way. Without the app, pages
from a box come through the front door and pass through Cloudflare, which
can read them (📖 **Read outside**). Each service's ratings are in the [Privacy policy](../policies/privacy.md).
