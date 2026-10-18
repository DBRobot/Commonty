# Your email address

Every member has an address at Commonty's domain, `<name>@<domain>`.
Mail sent to it is forwarded to an email address of your own, such as
the one you already use. Commonty's services write to you there, so
your own address never has to be kept on a box.

## Setting it up or changing it

1. Open Settings, then **Email**. **Sent on to** shows whether you have
   one set.
2. Press **Change email** (or add one), and confirm with your passkey.
3. Type the address mail should go to, and press **Continue**.
4. Cloudflare sends a link to that address. Open it to confirm. The
   page moves on once you have. Until then, nothing is forwarded.

**Show** reveals the address mail goes to. After your passkey, this
browser can show it for 30 days, until you sign out everywhere else or
that passkey is removed.

## What comes to it

- Codes and notices from [Passwords](services/passwords.md).
- Notice of any change to Commonty's policies, at least 30 days before it
  takes effect.
- Anything anyone sends to `<name>@<domain>`.

On its own, this address only forwards. Turn on [Mail](services/mail.md)
to keep your mail locked in Commonty and to send from this address and
your aliases.

## Aliases

An alias is an extra address you give one site, such as
`maple.heron.4821@<domain>`. Mail to it comes to your inbox like mail to
your own address. If a site starts sending spam, or passes your address
on, turn that alias off and its mail bounces; your other addresses carry
on as before.

1. Open Settings, then **Email**, and press **New alias**.
2. Say what it is for (the site's name, say). **Another** offers a
   different address if you want one.
3. If you only need it for a while, choose when to **Turn it off**:
   after a day, a week or a month.
4. Press **Make it**, then **Copy address**, and give it to the site.

Each alias in the list has a switch that turns it off and on, a copy
button beside its address, and a **⋯** menu to rename it, to turn it off
on a day of your choosing, or to delete it. One that turned itself off
can be switched back on.

### Who has mailed it

Each alias shows the sites that have mailed it. If a site it has not
heard from before writes to it, the alias says **New sender**: the site
you gave it to may have passed it on. Press **That's fine** if you know
the sender, or **Turn it off**. Anyone can put any sender on an email, so
treat this as a hint.

### Make aliases from Passwords

Passwords can make an alias for a site as you save its login.

1. In **Alias settings**, press **Connect…** and confirm with your
   passkey. A key is shown once.
2. In Passwords, open **Generator**, then **Username**, and choose
   **Forwarded email alias** with the service **SimpleLogin**.
3. Paste the **Server URL** and the **API key** shown.

Each username Passwords makes is a new alias, labelled with the site. The
key can only make aliases, from your ten a day. **Disconnect** stops it
working.

- The address is made up for you: you cannot choose one.
- You can have up to 100 aliases. Each day you are offered ten
  addresses to choose from, so you can make up to ten a day.
- Deleting one, and connecting Passwords, asks for your passkey.
- A deleted alias's address is gone for good. Its mail bounces, and no
  one is ever given it again.
- Aliases only receive mail. You cannot send from one.

## Privacy

- **Your own address is kept at Cloudflare, not on any box.** The
  forwarding is a Cloudflare Email Routing rule, and only a small
  Cloudflare Worker of Commonty's holds the key that can read or change
  those rules. No box can read your address.
- **Cloudflare can read every email** it forwards, and sees your own
  address (📖 **Read outside**).
- **Mail Commonty sends you** is handed to the same Worker and sent on by
  Cloudflare, which can read it too. No box holds a mail account.
- **Your own email provider** reads what arrives, as it does all your
  mail.
- **Which aliases are yours**, and the sites that have mailed each, are
  kept at Cloudflare, by the same Worker, not on any box. Whoever runs Commonty's Cloudflare account can see it,
  and Cloudflare reads mail to an alias as it passes it on, as it does for
  your own address.
- **Every alias ends `@<domain>`**, like every member's address, so a site
  you give one to can tell you are a member of Commonty.

Every service's ratings are in the [Privacy policy](../policies/privacy.md); how Commonty is protected is in [Security](../policies/security.md).

## Security

- **Changing it needs your passkey**, used in the last 15 minutes, and
  the Worker checks it against your own signed account record, not
  against anything a box says.
- **A box cannot redirect your mail.** None of them holds the key to
  the forwarding rules.
- **The new address must be confirmed** from its own inbox before mail
  goes there.

## Deleting and keeping

When you leave, the Worker removes your forwarding rule and deletes
your aliases at once (90 days after, if you are suspended), and your own
address too when no other rule uses it. It does so only on proof it checks
itself: your own signed statement that you left, or your suspension in the
signed member list. See [Leaving](leaving.md).

## Limits

- Forwarding only: no inbox, and no sending as `<name>@<domain>` or as
  an alias.
- If the confirmation link is never opened, nothing is forwarded.
- Guests have no address.
