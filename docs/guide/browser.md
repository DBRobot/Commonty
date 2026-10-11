# Using a browser

Every part of Commonty works in an ordinary browser. Some parts open
straight from a box. The parts that hold your keys open through the
Commonty app on the same computer.

## What opens directly

These need nothing but a browser:

- the public site, these docs, the demo and the **Get the app** page;
- a Passwords Send link someone gave you;
- once you sign in with your passkey: home, Games, Git, Metrics, Friends,
  Ad blocking and Settings. In Settings, Backups and your device names
  hold keys, so those two parts work only through the app.

## What opens through the app

Files, Movies & TV, Chat, Photos and Passwords hold your keys, and so
do Backups and device names in Settings.
Their code comes with the app you installed, never from a box, so a box
cannot change it to steal a key. The app serves them to its own window,
and to your browser on the same computer once you allow it.

1. Open the page in your browser, for example Files. It says **This
   opens through the Commonty app**. Press **Open**.
2. The app comes up and shows **Allow this browser**, with a 6-digit
   code.
3. Your browser shows a code too. Check the two codes match, then press
   **Allow this browser** in the app. If they do not match, or you did
   not ask, press **Not me**.
4. The page opens in your browser.

From a page opened this way, the other key pages (Photos and Passwords
too) open without asking again, until the app closes. After the app
restarts, you allow the browser once more.

If nothing opens, the app is not installed or not signed in on this
computer. The page offers **Get the Commonty app**.

Good to know:

- The app only answers a request it was told to expect in the last two
  minutes, and only after your click. If two browsers ask at once, both
  are refused and you start again.
- The address your browser opens holds no secret. What it keeps is under
  a name only that browser knows.
- The app trusts your browser with what it opens, as you already trust it
  with your passkeys. A program that can read your own files could read
  the app's keys too; none of this protects against that.

## On a phone

On Android, use the app. Its window has every page.

## No Mac or iPhone app yet

There is no Commonty app for Mac or iPhone yet. On those, you can use
everything under [What opens directly](#what-opens-directly), and nothing
that holds a key:
<!-- coming: no Mac or iPhone app yet -->

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
