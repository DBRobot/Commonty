# Mail

## What it is

Your own inbox for your Commonty address and your aliases. Mail is locked
to a key only your devices hold the moment it arrives, before any box has
it. The boxes keep it and cannot read it. Search, spam sorting and
everything else happen on your device.

## Turning it on

1. Open **Mail** in the Commonty app.
2. Press **Turn on Mail**. The app makes your mail key on this device.
3. Press **Confirm with passkey**. A page from your mail service opens:
   check it shows the same code as the app, then use your passkey.
4. Mail opens by itself once you have.

From then on, mail to your address and your aliases goes to Mail. A copy
still goes to your own email address (the one in
[Your email address](../email.md)) unless you untick that when you turn
Mail on.

## Using it

- **In the app:** open **Mail** from the home page.
- **In a browser:** open Mail. The page says it opens through the
  Commonty app; press **Open** and allow the browser in the app
  ([Using a browser](../browser.md)).

Then:

- **Read:** pick a message. It shows as a page. Pictures from the
  internet are held back until you press **Show pictures**, because
  loading them tells the sender you opened it.
- **New email:** write it in the viewer. The formatting bar is along the
  top; **Attach** and **Send** are on its right. Add **Cc** or **Bcc**
  beside To. Pick **From** to send from your address or one of your
  aliases. What you write is kept as a draft as you go.
- **Reply, Reply all, Forward:** from the bar at the top of a message. A
  reply goes from the address the mail came to.
- **Search:** type in **Search mail**. It finds as you type. Narrow it
  with `from:`, `to:`, `subject:` or `file:`.
- **Star, Archive, Mark unread:** from the bar at the top of a message.
- **Spam:** press the warning sign to move a message to Spam, or
  **Not spam** to bring it back. Your filter learns from both. Mail whose
  sender fails the checks its own domain publishes goes to Spam by itself.
- **Delete:** moves a message to Trash. In Trash, **Delete for good**
  deletes it from your devices and the boxes at once.
- **Bring in old mail:** export your old mail as an mbox file (Google
  Takeout gives one for Gmail), then press **Bring in old mail** at the
  bottom of the folders. Each message is locked on your device before it
  goes to the box.

Mail over 2 MB is not locked as it arrives. It goes to your own email
address whole, and Mail shows a note of what came.

## Privacy

| The box | Outside |
| --- | --- |
| 🔒 **Locked** | 📖 **Read outside** |

- **What the box keeps:** your mail, locked. It cannot read who it is from,
  what it says or what it is about, and it never holds the key.
- **What the box can see:** how many messages there are, how big each one
  is, and when each came. When you send to another domain, it sees which
  mail server it connected to, when, and how much went through, never
  what or to whom; its log keeps which member opened the way to which
  server.
- **Outside companies:** Cloudflare reads each email from outside
  Commonty once, as it arrives, to lock it. When you send outside
  Commonty, Cloudflare sees who it is from and to, its date and its id
  (to sign it for your domain), never its subject or what it says. The
  other side's mail server reads what you send, as with any email.
- **Between members:** mail from one member to another is locked on the
  sender's device. Nobody else reads it, Cloudflare included. The box does
  see who sent it to whom, and when.
- **The copy to your own address:** while forwarding is on (it is, unless
  you untick it), each email also goes to your own address as it always
  did, and whoever runs that mailbox can read it.

Every service's ratings are in the [Privacy policy](../../policies/privacy.md); how Commonty is protected is in [Security](../../policies/security.md).

## Security

- Every locked message is signed by whoever locked it: Commonty's mail
  service for mail that came, your own devices for what you wrote and
  changed, another member's device for mail from them. Mail shows nothing
  else, so a box cannot make up a message, change one, or pass one off as
  another.
- **Turn on Mail** only from the Commonty app. If a link or a page you did
  not open yourself asks you to confirm your mail key, close it.
- Mail you send outside goes encrypted to the other side's server or not
  at all: if that server offers no encryption, or the wrong certificate,
  nothing is sent.
- Your key is sealed to your devices and your paper key. A device you add
  later gets it the first time it opens Mail. For an account signed in by
  passkey alone, Mail opens only on the device that turned it on.
