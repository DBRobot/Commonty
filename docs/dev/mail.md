# Mail

A member's own inbox, which no box can read. The mail Worker locks each
email to the member's key as it arrives, a box keeps it, and only the
member's devices open it. Sending runs on the member's own device: the box
carries the bytes to the other side's mail server and cannot read them.

Off-the-shelf mail servers (Stalwart, Mox, Mailcow, Forward Email,
OpenNeutron) all read mail on the machine they run on while they receive
it. Our rule is that no box reads a member's mail
([no node is trusted](../policies/security.md)), so receiving stays in the
Worker, where Cloudflare is already the one outside reader of mail from
outside.

## Who can read what

| Mail | The member's devices | A box | Cloudflare |
|---|---|---|---|
| Stored, all of it | reads it | locked: sizes and times only | nothing, once a box has it |
| From outside | reads it | locked | reads it once, as it arrives |
| To outside | reads it | which server, when, how much | the DKIM request's headers (From, To, Cc, Date, Message-ID), never the subject or body |
| Member to member | reads it | locked, and who sent it to whom, and when | nothing |
| Forwarded copy (on unless turned off) | - | - | sends it, readable, to the member's own address, whose provider reads it |

## The key

Each member has one mail key: 32 random bytes made by their app the first
time they turn Mail on. It is sealed to the keys of their entry a device
may seal to (`account::seal_for_keys(.., "mail", ..)`: every device, the
root and the paper key, or this device alone when the root is a passkey)
and kept on the box as `<name>/key/doc`, which the box cannot open. Each
device opens it with its own key and seals it again for any device added
since (`app/src/mail.rs`).

From those bytes comes an X25519 key pair (RFC 9180 DeriveKeyPair). The
page holds the private half as a WebCrypto key it cannot export. The public
half goes to the Worker with the member's passkey on the Worker's own page,
`mail.<domain>/inbox` (`inbox.html`): a box's page can have a passkey sign
many things, but never with the Worker's origin in what it signs. The app
puts the key and its device key in that page's fragment, never sent
anywhere, and both show the same code (the first 6 bytes of sha256 of
name, device and key) so the member can tell it is their app's.

That same step names the device that may send for the member: its
ed25519 key, which signs everything the app asks of the Worker. The Worker
takes it only if the member's own signed entry names that device, and that
device signed the mail key (`commonty mail key v1\0<name>\0<pk>`). A link
someone else made cannot have the member's passkey lock their mail to a key
of theirs. For an account whose root is a passkey, a box's page could have
the passkey sign an entry naming a device of the box's. So for those, the
first device to turn Mail on stays the only one; its key opens only there
in any case. That first step is still on trust, and the page says to go on
only after pressing Turn on in the app.

**Locking** (`client/mail/worker/mail-hpke.js`): HPKE base mode,
DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, AES-256-GCM, in WebCrypto alone,
checked against RFC 9180's own vectors and against a Python implementation
in both browser tests.

**Vouching** (`mail-object.js`). Anyone can lock anything to a public key,
a box included, so a lock alone proves nothing about who made it. Every
locked object is

```
"CMA1" | u32 header length | header | u32 summary length | summary | message
```

The summary and message are sealed with info
`commonty mail v1 [<kind> ]<part> <id>`. The header, in the open, names the
kind, the id and the sha256 of both sealed parts, and who vouches for them,
over `commonty mail object v1\0<kind>\0<id>\0<hs>\0<hr>[\0<name>\0<fp>]`:

- `worker`: the mail Worker's Ed25519 key (`MAIL_SIGN`), for mail that
  came. The Worker keeps every public half it has used (KV `signkeys`) and
  tells a member's device (`/api/mailkey`), so older mail stays believed
  after a new key.
- `me`: an HMAC with a key derived from the member's mail key (HKDF, "commonty
  mail mac v1"), which only their devices have: changes, Sent, drafts and
  imported mail.
- `member`: a device of another member, through their app
  (`/_dd/app/mail/vouch`, which fills in the name and device itself), and
  only when the box says that member handed it over.

The page shows nothing it does not believe. A box can delete, but cannot
make up, change or swap.

## Mail coming in

1. A member turns Mail on: the Worker keeps `mailkey:<name>` and changes
   their address's routing. Their own rule (`Commonty member <name>`) is
   kept, turned off, as the record of where they forward; a new rule
   (`Commonty inbox <name>`) sends their address to the Worker. Aliases
   already come through the catch-all.
2. The Worker (`inbox.js`) reads the message, writes a summary of it
   (`mail-mime.js`: sender, recipients, subject, date, a snippet,
   attachment names, and Cloudflare's SPF/DKIM/DMARC results, taken only
   from the Authentication-Results under Cloudflare's own name). It
   locks both, signs them, and puts the blob in the Hold Durable Object.
3. It forwards the message as well, unless the member turned forwarding
   off, and always when it could not be locked.
4. A box with the mail bucket (`library` role,
   `nix/modules/library/mail.nix`; fleet.js `MAIL_BOXES`, and no other box)
   takes it every 15 seconds, page by page:
   `GET /api/hold/list`, the parts, `POST /api/hold/ack`, each signed by
   the box's TPM key through `dd-mail-hold-sign`, a root socket that signs
   only hold requests for this box. The gate keeps it at
   `<name>/in/<id>` in the mail bucket.

The Hold keeps each blob in 1 MiB rows and each box's newest signed time,
so a recorded request cannot be sent again.

## Reading

The Mail page (`servers/mail/web`) is carried by the app, which alone holds
the key; a browser is handed to the app, as for Files. It reads through the
app's proxy to the gate:

- `GET /_dd/mail/list/<kind>?after=<id>`
- `GET|PUT|DELETE /_dd/mail/o/<kind>/<id>` (a `Range` reads the summary
  alone)
- `POST /_dd/mail/deliver/<to>/<id>`

Kinds are `in`, `sent`, `drafts`, `state`, `index`, `filter` and `key`.

`mail-mailbox.js` opens what it lists and keeps what it opened in the
device's IndexedDB. The list is drawn from there before the box is asked
anything.

Ids start with the time they were made, by whoever made them: the Worker
when the mail arrived, or a sender's device. A box can store one after a
later one, for example mail the Worker held while the box was away. So
each sync asks the box again from a day before the last id it saw, and
once a day from the start, skipping what the device already has.

**Changes** a member makes (read, starred, moved, deleted) are locked ops
in `state/`, replayed by every device in id order.

**Search** (`mail-search.js`) is on the device. Messages are numbered as
they come and frozen into segments of 2,048, each with a sorted term list,
packed postings and a bloom filter of terms and their first four letters.
A query walks segments newest first, skips those whose bloom cannot hold a
word, and stops at enough matches. The word being typed is a prefix.
`from:`, `to:`, `subject:` and `file:` narrow it. Measured on 300,000
made-up messages: 0.24 ms median and 3 ms at worst (`search.test.mjs`
fails above 16 ms).

**Spam**: the Worker's word first (a DMARC failure is spam). Then a filter
on the device (`mail-filter.js`, Graham's "A Plan for Spam") learns from
what the member marks. It is kept on each device.

**HTML** is drawn in a frame that runs no script and loads nothing from
the internet until the member says so. The sanitizer drops script, forms,
frames, event handlers and styles that reach out. The frame's own policy
allows only `data:` and `blob:` pictures, and `cid:` pictures come from the
message itself. A draft's HTML is cleaned the same way before it is
edited. The app gives a browser it let in the page policy of its own
window, so no inline script runs where the key is.

**Attachments and Files.** The page opens the member's library the first
time an attachment goes to or comes from Files (`library.js` `unlock()`,
which the app answers for Mail as it does for Files), and saves with
`pick.js` (`pickFolder`, `saveInto`, which never writes over a file) into
a folder that starts at `Files/Mail/<year-month>`. Where each attachment
went is a locked change like read and starred: `{ saved: { "<n>:<name>":
{ lib, path, at } } }`, keyed by its place in the message as well as its
name, since a message can carry two `image.png`; every device shows
"Saved to Files / ...". Attach from Files reads the file with
`fetchPlain` and adds it like one from the device, refused past about
18 MB of files. A PDF attachment opens in the PDF editor on its bytes
(`openEditor({ bytes })`, [pdf-editor.md](pdf-editor.md)), and **Reply with
it attached** hands the finished PDF to a reply. The box sees none of it:
the attachment is opened with the mail key and sealed with the library key
in the same tab.

**Photos** ([`mail-photos.js`](../../servers/mail/web/mail-photos.js)).
A picture or film attachment has **Save to Photos**: it goes into an album
called From Mail (made the first time), dated with the email. The compose
Attach menu has **From Photos**, which attaches the chosen photos up to
the same 18 MB, and **Photos by link**, which puts a link to them in the
email. The link opens a new album that holds only those photos, as Ente's
own Share link does, and anyone who has the link can see them. The key to
that album is after the link's `#`, so no server is sent it.

The page asks the app for Photos' password through its one key route
(`POST /_dd/app/keys {services: ["photos"]}`,
[app.md](app.md#one-key-route)). It opens the account in the tab with our
Rust in the browser ([`client/web/src/photos.rs`](../../client/web/src/photos.rs)
over [`client/ente`](../../client/ente/src/gallery.rs)), and seals and opens
files the same way Ente's apps do: a 4 MiB stream with its own key, a small
picture, metadata, and the file's key sealed with the album's. The page may
only reach its own origin, so museum calls go to `/_dd/ente/...` and the
store's presigned addresses go to `/_dd/ente-store/<host>/...`. The app
passes these on with no credential of the device's. Photos' master key is
in that tab and nowhere else. Before a device has opened Photos once in the
app, there is no password to hand over, and the page says so.

## Sending

From the member's own device (`app/src/mail.rs`, `client/mailsend`):

1. The page writes the message (`mail-compose.js`: text beside HTML,
   attachments, encoded words, CRLF, no Bcc header).
2. To a member with Mail on: the page locks it to their key, signs its
   hash with the device key (`/_dd/app/mail/vouch`), and the gate puts it
   in their inbox, with the sender written beside it as the gate knows
   them. The reader's page checks the signature against the sender's
   devices (`/api/mailkey/<name>`); a mismatch is spam and says so.
   This is only for mail from the member's own address. From an alias it
   goes the outside way below, so an alias is never tied to their name.
3. To anyone else, the app:
   - asks the Worker for the domain's DKIM signature over the headers
     alone and the body's hash (`/api/dkim`, signed by the device; the From
     must be the member's address or a live alias of theirs; 200 a day);
   - looks up the MX by DNS-over-HTTPS;
   - opens the box's pipe, a WebSocket `/_dd/mail/pipe` that the gate
     joins to port 25 of a server the domain names as its MX, only ever a
     public address;
   - runs SMTP through it, with STARTTLS required and the certificate
     checked against the MX's own name. No TLS, or the wrong
     certificate, and nothing is sent.
4. A copy goes to `sent/`, locked to the member's own key.

`dd mail dkim` makes the domain's DKIM key and the Worker's `MAIL_SIGN`.
Both secrets go to the Worker and are kept nowhere else. The DNS Worker
publishes `cm1._domainkey` from fleet/mail.json, and, when the domain has
none, a DMARC record `p=quarantine` (`convergeMail`). That sends to spam
mail claiming the domain without the Worker's signature, which is what a
member could send straight through the pipe. SPF is left as it is: DMARC
passes on the DKIM signature alone.

The Worker signs a From only when it is the member's address or a live
alias of theirs, written plainly: no encoded words, no second address,
no address in the name. Each signed header is listed once more than the
message has it, so a From added later breaks the signature.

The pipe refuses any request that carries an Origin, since the app sends
none, and a box's journal keeps who opened one to which server.

## Leaving

The Worker's forget deletes the inbox rule, `mailkey:` and anything still
in the Hold (`leaving.js`). The gate deletes every object under the
member's name in the mail bucket (`leaving.rs`).

## Limits, said plainly

- **Who mailed whom, between members.** Mail one member sends another is
  stored under the recipient's name with the sender beside it
  (`x-amz-meta-from`), so the box knows who wrote to whom and when, though
  never what.
- **Forwarding is on by default.** Until a member turns it off, every email
  also goes, readable, to their own address. Their mail provider (Gmail,
  say) reads that copy as it always did. Locking protects the copy Commonty
  keeps, not that one.

- **Very large mail and the free plan.** Locking costs about 2 ms per MB of
  Worker CPU, and the free plan allows 10 ms. Mail over 2 MB (`LOCK_MAX`)
  goes to the member's own address whole, forwarding on or off, and the
  inbox gets a locked note of what came: its sender, subject and size.
  This is why forwarding is on by default. On the paid plan `LOCK_MAX` can
  be 25 MB, Email Routing's own limit.
- **Passkey-root accounts.** For an account whose root is a passkey, the
  key opens only on the device that made it, as Photos' password does.
- **Mail apps.** Thunderbird and Apple Mail are not served: there is no
  IMAP, as they cannot hold the key.
- **Outgoing reputation.** Mail leaves from the box's home address, which
  has no reverse DNS name. Gmail and Outlook took connections from node1
  on 2026-10-18; whether they file its mail as spam is not measured yet.
- **The fleet's own aliases** (`security@` and the like) forward as they
  did and are not locked.

## Tests

- `checks.mail-worker`:
  - the JS units: HPKE against RFC 9180, MIME, search with the benchmark,
    the filter, composing and the mailbox;
  - the Worker end to end: turning on, rules, locking opened by Python,
    the Hold's signatures, forwarding off, device signatures, DKIM checked
    by dkimpy, the inbox page, off.
- `checks.mail-page`: the page in a browser against a made-up app and box,
  with screenshots.
- `checks.dns-worker`: the mail records, and nobody else's touched.
- The crate tests: `mailsend` (SMTP over TLS, refusals, dot-stuffing,
  DKIM's body hash) and the gate's `mail::tests` (ids, public addresses,
  the hour's share).
