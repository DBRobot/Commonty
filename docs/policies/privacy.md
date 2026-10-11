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
That also means nobody can recover it for you: if you lose every device,
passkey and your recovery key, what is locked stays locked.

**What is not private.** Some things are shared on purpose, so they are
not locked:

- **Git.** Every repository is public. Code that must stay private goes in
  an encrypted repository instead.
- **Games.** A game server has to read its world to run it, so worlds,
  saves and server logs are open to the box that runs them.

A few things also have to be read by a box for a moment to work at all:
a chat message while it is answered, a search you send out, a film being
converted for your phone. Nothing of them is kept. Each one is in the
table below.

**Open and honest about it.** Commonty is open source. Every line that
handles your data is public, and so is how it is built: the boxes run only
releases built from that public code and signed, and two boxes must build
each release to the same result before it ships. This page says exactly
what each part of Commonty can and cannot see, including where it falls
short today. When we find a gap we say so here and fix it. If you find
one, write to [security@commonty.org](mailto:security@commonty.org).

## How private each part of Commonty is

Every service is rated on two things: what the **box** it runs on can see,
and what **outside companies** can see. A box is one of the computers that
members keep Commonty on, and whoever looks after a box (its owner, or
anyone who breaks into it) can see what the box can see.

**What the box can see**

| Rating | Means |
| --- | --- |
| 🔒 **Locked** | Locked on your own devices before it leaves them. The box keeps it but cannot read it, not even the names of your files. |
| 🔏 **Locked, labelled** | The contents are locked, but the box sees the label on the outside: who it belongs to, when it changed, how big it is. |
| 👁 **Open to the box** | The box can read it. |

**What outside companies can see**

| Rating | Means |
| --- | --- |
| 🏠 **Stays home** | Nothing leaves Commonty's own boxes. |
| 📦 **Passes sealed** | Another company carries it, but sealed: they cannot read it. |
| ✉️ **Envelope seen** | Another company sees who, when and what was asked for, but not the contents. |
| 📖 **Read outside** | Another company can read it. |

## Service by service

| Service | What it keeps | The box | Outside | Who outside, and what |
| --- | --- | --- | --- | --- |
| **Files** | Your files and folders | 🔒 Locked | 🏠 Stays home | — |
| **Movies & TV** | Your films and shows, where you stopped, the shelf's details | 🔒 Locked | ✉️ Envelope seen | Your device asks TMDB for a film's poster with its title and year |
| **Disk images** | Copies of your old computers | 🔒 Locked | 🏠 Stays home | — |
| **Devices** | The names you give your devices | 🔒 Locked | 🏠 Stays home | — |
| **Chat history** | Your conversations | 🔒 Locked | 🏠 Stays home | — |
| **Chat, while it answers** | The message being answered, only while it is answered | 👁 Open to the box | 🏠 Stays home | Demo chats pass through Cloudflare, which can read them |
| **Web search** | Nothing kept | 👁 Open to the box | 📖 Read outside | Search engines see what you search for, sent from the box, not from you |
| **Passwords** | Your vault | 🔏 Locked, labelled | ✉️ Envelope seen | Codes and notices are emailed to you, and the company that sends them sees each email |
| **Photos** | Your photos and albums | 🔏 Locked, labelled | ✉️ Envelope seen | Sign-in codes are emailed, as above |
| **Your account** | Your name, devices and passkeys (public keys only) | 🔏 Locked, labelled | 📖 Read outside | Sign-in pages pass through Cloudflare, which can read them |
| **The network** | Which devices you have, and when they were online | 🔏 Locked, labelled | 📦 Passes sealed | Tailscale's relays may carry your traffic, sealed |
| **Storage counts** | How many bytes you keep in each service | 🔏 Locked, labelled | 🏠 Stays home | — |
| **Metrics** | How the boxes are doing | 🔏 Locked, labelled | 🏠 Stays home | — |
| **Git** | Repositories, issues, pull requests | 👁 Open to the box | 📖 Read outside | Every repository is public, by design. For private code use an encrypted repository |
| **Games** | Your worlds, saves and server logs | 👁 Open to the box | 🏠 Stays home | Games download from Steam and GitHub; nothing of yours goes out |
| **Ad blocking** | Nothing about which sites you visit is kept | 👁 Open to the box | ✉️ Envelope seen | Quad9 sees each name looked up, encrypted on the way |
| **Playing a film on a phone** | The one film, in memory while it plays | 👁 Open to the box | 🏠 Stays home | — |
| **The front door** (commonty.org, home., sign-in) | Request logs: address, page, browser, for three weeks | 👁 Open to the box | 📖 Read outside | Cloudflare carries these pages and can read them |

## Cookies

Commonty sets only the cookies it needs to keep you signed in, and none
from anyone else.

| Cookie | What it is for | How long |
| --- | --- | --- |
| `dd_session` | Keeps you signed in. Page scripts cannot read it, and it is only sent over a secure connection. | 8 hours |
| Photos and Passwords | Each keeps its own sign-in in your browser. | Until you sign out |
| The app's own | Lets the Commonty app's windows talk to the app on your computer, and nothing else. | Until the app closes |

There are no tracking, advertising or analytics cookies, and no outside
company sets cookies on Commonty's pages. Some pages keep small things in
your browser so they open instantly, like the last list you saw; they stay
on your device.

## Keeping and deleting

Something you delete stays in the trash for **30 days**, so a mistake can
be undone. Within **90 days** it is gone from every box and every backup.

## Leaving

You can take everything with you at any time: your files, photos,
passwords and disk images, from Settings.

When you leave, or are removed, every sign-in and device of yours ends at
once. Everything of yours is kept, still locked, for 30 days in case you
come back. Within 90 days of leaving it is gone from every box and every
backup.

Public repositories you made are deleted too. Work you gave to other
people's repositories stays there, as it does on any open-source project.

## When this page changes

Every member is emailed about a change to this policy at least **30 days**
before it takes effect. The date this page last changed is at the bottom,
and every change to it is public in Commonty's history.
