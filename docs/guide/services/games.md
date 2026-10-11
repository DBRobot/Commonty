# Games

## What it is

Your own game servers. Pick a game, press **Start**, and a dedicated
server for it runs on a box, for you and the friends you invite.

## Using it

Open **Games** from the home page, in the app or in a browser. Games
works in a plain browser: it holds no keys. To play, your device must be
on Commonty's network, so the Commonty app has to be running on it (on a
phone, the Android app).

**Start a server**

1. **Pick a game** from the library. It has more than a hundred games;
   some run Windows servers under Wine.
2. Fill in the card's settings, and any of **More settings**. A game
   that wants a server password gets a random one unless you set your
   own.
3. Press **Start**. The page shows it starting, downloading the game the
   first time, then running.
4. Copy the **Address** and paste it into the game. For some games the
   page says where.

**Your servers**

- **Stop** and **Start** it again whenever you like; the world is kept.
- **Settings** take effect when the server starts; a running one
  restarts, world kept.
- **Keep world, delete server** saves the world under **Saved worlds**
  and frees the server. **Start a server with it** brings it back.
- **Delete server and world** removes both.
- You see your server's log, and why it failed if it did.

**Mods**

Some games come in a modded version in the library, such as Valheim with
BepInEx or Valheim Plus. Pick that version to play with mods. Adding your
own mod files is not built yet.

**Inviting friends**

1. Make a friend link on **Friends** and send it. Whoever opens it
   becomes your friend. Someone without an account can sign up as your
   guest.
2. On your server's page, under **Players**, tick who may play, then
   **Save players**.
3. They see the server under **Invited to**, with its address and
   whether it is up. They join from the Commonty app's network.

See [Inviting friends to games](../guests.md) for the guest's side.

## Privacy

| The box | Outside |
| --- | --- |
| 👁 **Open to the box** | 🏠 **Stays home** |

- **What the box keeps, and can read:** your servers' settings, worlds,
  saves and logs, and who you invited. A game server has to read its
  world to run it, so none of this is locked. The log can hold what
  players type in the game, so only you see it.
- **Outside companies:** the box downloads games from Steam and the
  servers' files from GitHub. Nothing of yours goes with those
  downloads. Players reach your server over Commonty's network, which
  may pass through Tailscale's relays, sealed
  ([Your home network and Wi-Fi](../network.md)).

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- **Each server runs in its own virtual machine** on the box, apart from
  the box and from other servers.
- **Only invited devices get in.** A server's ports open only to your
  devices and the devices of the friends you ticked, only on Commonty's
  network. Untick someone, or stop being friends, and they lose access
  within a minute.
- **Who can do what:** members start servers. Guests join servers they
  are invited to and see nothing else of Commonty. The demo can look
  at the library, not start.
- **If a box were taken over,** the attacker could read and change your
  worlds, saves and logs, and the server passwords in your settings.

## Deleting and keeping

- **Deleting** a server or a saved world removes it from the box at
  once. There is no trash.
- **Backups:** servers, worlds and invitations are in the box's nightly
  backup, kept up to five weeks, then its snapshots a week. Something you
  delete is gone from every box within **90 days**.
- **Leaving:** your servers stop, and every server and saved world of
  yours is deleted, at once (30 days after, if you are removed). See [Leaving](../leaving.md).

## Limits

- One server of yours runs at a time. You can keep up to five servers
  and five saved worlds.
- A box has a memory budget for games. If it is full, a new server cannot
  start until another stops.
- Some games use a fixed port, so only one of that game can run on a box.
- A game that fails to start three times stops trying; **Stop** then
  **Start** tries again.
- Players need the Commonty app's network. There is no app for Mac or
  iPhone yet, so players on those cannot join.
