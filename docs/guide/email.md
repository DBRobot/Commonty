# Your email address

Every member has an address at Commonty's domain, `<name>@<domain>`.
Mail sent to it is forwarded to an email address of your own, such as
the one you already use. Commonty's services write to you there, so
your own address never has to be kept on a box.

## Setting it up or changing it

1. Open Settings. Your **Email** line shows whether you have one set.
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

You cannot send mail from this address. It only forwards.

## Privacy

- **Your own address is kept at Cloudflare, not on any box.** The
  forwarding is a Cloudflare Email Routing rule, and only a small
  Cloudflare Worker of Commonty's holds the key that can read or change
  those rules. No box can read your address.
- **Cloudflare can read every email** it forwards, and sees your own
  address (📖 **Read outside**).
- **Mail Commonty sends you** leaves through Google's mail service, which
  can read it too.
- **Your own email provider** reads what arrives, as it does all your
  mail.

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

When you leave, your address is kept 30 days in case you come back. Then
the Worker removes your forwarding rule, and your own address too when
no other rule uses it. It does so only on proof it checks itself: your
own signed statement that you left, or your removal in the signed member
list. See [Leaving](leaving.md).

## Limits

- Forwarding only: no inbox, and no sending as `<name>@<domain>`.
- If the confirmation link is never opened, nothing is forwarded.
- Guests have no address.
