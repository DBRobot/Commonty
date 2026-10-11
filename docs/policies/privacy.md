# Privacy

Privacy is Commonty's first concern. In an age of data farms, AI theft and
more and more private data held by a few companies, Commonty is built so
your private life stays yours.

**Your private data is end-to-end encrypted.** It is locked on your own
devices before it leaves them, and only you hold the keys. Commonty's
servers work on zero trust: no server in the network is trusted to see
your data - not the box it is kept on, not whoever looks after that box,
and not us. You are the only person who should ever be able to read it.
For everything locked, privacy is not a promise: it is a cryptographic
certainty. Nobody has to keep their word for your data to stay yours,
because without your keys there is nothing anyone can read.
That also means nobody can recover it for you: if you lose every device
and your recovery key, what is locked stays locked.

**What is not private.** Some things are shared on purpose, so they are
not locked:

- **Git.** Every repository is public, and anyone can read it. Code that
  must stay private goes in an encrypted repository instead.
- **Games.** A game server has to read its world to run it, so worlds,
  saves and server logs are open to the box that runs them.

A few things also have to be read by a box for a moment to work at all:
a chat message while it is answered, a search you send out, a film while
it plays. Nothing of them is kept. Each one is in the table below.

**Open and honest about it.** Commonty is open source. Every line that
handles your data is public, and so is how it is built: the boxes run only
releases built from that public code and signed, and every box checks
that two boxes built each release to the same result before it runs it.
This page says exactly what each part of Commonty can and cannot see, and
every company outside Commonty that sees anything, including where it
falls short today. When we find a gap we say so here and fix it. If you
find one, write to [security@commonty.org](mailto:security@commonty.org).

## How private each part of Commonty is

Every service is rated on two things: what the **box** it runs on can see,
and what **outside companies** can see. A box is one of the computers that
members keep Commonty on, and whoever looks after a box (its owner, or
anyone who breaks into it) can see what the box can see.

**What the box can see**

| Rating | Means |
| --- | --- |
| 🔒 **Locked** | Locked on your own devices before it leaves them. The box keeps it but cannot read it, not even the names of your files. It sees only how much there is and when it changed. |
| 🔏 **Locked, labelled** | The contents are locked, but the box sees the label on the outside: your account name there, who it is shared with, how many there are, when they changed. |
| 👁 **Open to the box** | The box can read it. |

**What outside companies can see**

| Rating | Means |
| --- | --- |
| 🏠 **Stays home** | Nothing leaves Commonty's own boxes. |
| 📦 **Passes sealed** | Another company carries it, but sealed: they cannot read it. |
| ✉️ **Envelope seen** | Another company sees who, when and where, but not the contents. |
| 📖 **Read outside** | Another company can read it. |

## Service by service

| Service | What it keeps | The box | Outside | Who outside, and what |
| --- | --- | --- | --- | --- |
| **Files** | Your files and folders | 🔒 Locked | 🏠 Stays home | — |
| **Movies & TV** | Your films and shows, where you stopped, the shelf's details | 🔒 Locked | 📖 Read outside | Your device asks TMDB for a film's poster: TMDB sees the title, from your own internet address |
| **Disk images** | Copies of your old computers | 🔒 Locked | 🏠 Stays home | — |
| **Devices** | The names you give your devices | 🔒 Locked | 🏠 Stays home | — |
| **Chat history** | Your conversations | 🔒 Locked | 🏠 Stays home | — |
| **Chat, while it answers** | The message being answered, only while it is answered | 👁 Open to the box | 🏠 Stays home | Demo chats pass through Cloudflare, which can read them |
| **Web search** | Nothing kept | 👁 Open to the box | 📖 Read outside | The search engines the box asks (Google, Bing, DuckDuckGo and others) see what you search for, sent from the box, not from you |
| **Playing a film** | The one film, in memory while it plays | 👁 Open to the box | 🏠 Stays home | — |
| **Passwords** | Your vault | 🔏 Locked, labelled | 📖 Read outside | Its notices are emailed to you through Cloudflare, which can read each email |
| **Photos** | Your photos and albums | 🔏 Locked, labelled | 🏠 Stays home | — |
| **Your account** | Your name, and your devices' and passkeys' public keys. Nothing secret | 👁 Open to the box | 📖 Read outside | Sign-in pages pass through Cloudflare, which can read them; Cloudflare also carries your account record between boxes |
| **Your email address** | Where your address here forwards to | 👁 Open to the box | 📖 Read outside | Cloudflare keeps where it forwards and handles every email sent to your address |
| **The network** | Which devices you have, their names, and when they were online | 👁 Open to the box | ✉️ Envelope seen | Tailscale's relays may carry your traffic, sealed, and see your devices' internet addresses. Cloudflare sees when your devices connect. Every name your devices look up on the network goes to Quad9, encrypted on the way |
| **Ad blocking** | Nothing about which sites you visit | 👁 Open to the box | ✉️ Envelope seen | Quad9 sees each name looked up, encrypted on the way |
| **Storage counts** | How many bytes you keep in each service | 👁 Open to the box | 🏠 Stays home | — |
| **Metrics** | How the boxes are doing. Nothing about any member | 👁 Open to the box | 📖 Read outside | Alerts about a box are emailed to its owner through Cloudflare, which can read them; ipinfo.io sees each box's internet address |
| **Git** | Repositories, issues, pull requests | 👁 Open to the box | 📖 Read outside | Every repository is public, by design. A picture in a repository from another site tells that site you looked |
| **Games** | Your worlds, saves and server logs | 👁 Open to the box | 🏠 Stays home | Games and their mods download from Steam, GitHub and modding sites; Steam's server list sees that a server is running. Nothing of yours goes out |
| **The front door** (every page's address) | Request logs: address, page, browser, for 20 days | 👁 Open to the box | 📖 Read outside | Cloudflare carries the public pages and can read them |

**Companies that see nothing of yours.** The boxes also reach a few
companies for themselves alone, never with anything of a member's: Let's
Encrypt (the site's certificate), cache.nixos.org (software updates), the
NTP pool (the time), Hugging Face (Chat's model) and Cloudflare, which
keeps half of each box's disk key so a stolen box cannot unlock itself.

## Cookies

Commonty sets only the cookies it needs to keep you signed in, and none
from anyone else.

| Cookie | What it is for | How long |
| --- | --- | --- |
| `dd_session` | Keeps you signed in. Page scripts cannot read it, and it is only sent over a secure connection. | 8 hours |
| `mail_session` | Keeps you signed in on the email page. | 30 days |
| Git's | Keeps you signed in to Git. | Until you sign out |
| Passwords' sign-in | Ties a Passwords sign-in to the page that started it. | 10 minutes |
| `dd_browser` | Lets a browser you allowed open the pages the Commonty app serves it. It never leaves your computer. | Until the browser closes, or the app ends it |

There are no tracking, advertising or analytics cookies, and no outside
company sets cookies on Commonty's pages. Photos and Passwords keep their
sign-in in your browser's storage, not in a cookie: Passwords' lasts 30
days unused. Some pages keep small things in your browser so they open
instantly, like the last list you saw; they stay on your device.
Cloudflare asks browsers to report pages that fail to load to it; those
reports name the page, not you.

## Keeping and deleting

Something you delete goes to the trash, so a mistake can be undone. It
stays there **30 days**, unless you delete it for good from the trash,
which deletes it at once. Either way, within **90 days** it is gone from
every box and every backup.

## Leaving

You can take everything with you at any time: your files, photos,
passwords and disk images, from Settings.

When you leave, every sign-in and device of yours ends at once, and
everything of yours is deleted straight away. Copies in the boxes'
backups cannot be picked out and deleted; they expire on their own, and
within 90 days nothing of yours is left on any box or in any backup.

If you are removed rather than leaving yourself, everything of yours is
kept, still locked, for 30 days, in case it was a mistake. Within 90 days
of that it is gone from every box and every backup.

Two things stay: your name and your public keys, in the record every box
keeps, so that nobody else can take your name; and work you gave to
other people's repositories, as on any open-source project. Public
repositories you made are deleted.

## When this page changes

Every member is emailed about a change to this policy at least **30 days**
before it takes effect, at their address here. Every change to it is
public in Commonty's history.
