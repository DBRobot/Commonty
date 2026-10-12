# Leaving and deleting

A member can take everything out and leave, or be removed by the fleet's
owner. Either way every way in ends at once. Leaving by their own word
deletes everything of theirs from every box straight away, and there is
no coming back; a removal keeps it 30 days, since it was not their
request and the owner can undo a mistake. Within 90 days it is gone from
every box and every backup.
Policy changes are announced to every member 30 days before they take
effect ([below](#announcing-a-change)).

## Taking things out

Settings has a **Leave** tab
([leave.js](../../servers/home/web/leave.js)) whose **Take your things with
you** downloads everything from the page itself, where the app serves it
(its window, or a browser it let in): no links off to the services, and no
command. A plain browser of a member says they work from the app, or a
browser let in from it.

```
Files & Movies   one zip of the whole library, made in the page: walked by
                 PROPFIND, each file read by ranges and opened with the key
                 the app hands over (/_dd/app/library, library.js pieces), and
                 written as it goes (zip.js: stored, zip64, a descriptor
                 after each file)
Photos           one zip of every original, made by the app (export.rs): it
                 signs in to Ente with the password it holds, lists with
                 client/ente's photos(), and opens each original as it
                 downloads (Original); a folder for each photo's first album
                 by name, "Not in an album" for the rest
Disk images      Backups' own list and Download (backups.js `images`), without
                 Delete
Passwords        the vault opened on its own Export screen (/#/tools/export):
                 the member types their master password there; nobody else
                 ever holds it
```

**Where a zip goes.** Never whole in memory. In a browser the app let in,
the page starts a job (`POST /_dd/app/export {kind}`) and opens its
download (`GET /_dd/app/export/zip?id=`), which the app streams as the zip
is made: Photos' by the app, Files' from the page in pieces of about 8 MiB
(`POST /_dd/app/export/chunk`, then `end`), a few pieces in flight at most,
so a slow download holds the making back. The app's window cannot stream
an answer on its scheme, so there the app writes the zip to the Downloads
folder (site.rs, the same routes behind the launch secret) and the page
says where. Its window on Android takes no request body, so there the page
keeps a library's zip as a Blob of pieces; elsewhere without the app,
a file the person picks (`showSaveFilePicker`) takes it. Progress is the
page's own count for Files and `GET /_dd/app/export/state` for Photos;
**Cancel** stops the job (`POST /_dd/app/export/cancel`), breaks the
download off and removes a half-written file. A job is its starter's alone:
the window's, or one browser session's (`export::Owner`), behind the same
guards as `/_dd/app/library`.

**Photos' sign-in.** Ente's sign-in reaches museum with an HTTP client of
its own, which does not know the fleet's network. For the moment it takes,
the app passes it on from an address on loopback under a random path, to
museum alone and carrying none of the device's credentials (`relay`);
everything after goes through the fleet's network with the app's own
client, with no overall timeout, since an original can be large.

## Leaving, and being removed

**By the member.** The Leave button has the root sign a statement
(`identity::Leave {name, root, at, back}`, under its own label so no
other signature counts as one): the passkey, for an account made in a
browser; the app, for a root on its device (`/_dd/app/leaving`, in its own
window only, `account::leave`: it cannot be undone, so a browser the app
let in may not ask it, as it may not remove a device). `dd leave --yes`
signs the same; without `--yes` it says what it would do and stops. A
plain browser of an account whose root is on a device says to leave from
the app's window. The Leave tab puts **Take your things with you** right
above the button, and its dialog says everything is deleted now and
cannot be undone. The gate checks it against the entry on file and puts it in
the control plane's agreed log as a directory item (`Item::Leave`), so
every box replays the same statement at the same agreed time. That time is
when they left, and when their things are due. A statement dated more
than 5 minutes after it is agreed, or more than a day before, is refused:
it can neither put off the deleting nor be kept and played later. A gate
takes no coming back (`back`) any more, nor a second leaving
(`say_leave`, `leaving::start`); one already in the log still replays as
it was, so every box keeps agreeing on history.

**Anyone may leave**, not members alone: a friend's guest, and an account
made in a browser that is still waiting, leave with their passkey the same
way (Settings shows a guest the Leave tab, and a waiting account nothing
else). A guest's statement goes through the log like a member's, and the
guest is in `leavers` from then on: at once nothing lets them in (`guest()`
asks `has_left`), their sessions end and their devices are reaped, and the
gate deletes their friendships, their guest record, any link of theirs
and their sign-ins (`friends::forget` touches nothing of the
members they were friends with, nor any other guest), the games manager
their servers, and the other services find nothing of theirs and report
done. A waiting account is on one box only, held and never in the
directory, so its statement is that box's alone (`say_leave` records it
against the held entry). `held_leavers` ends its sessions and deletes
the hold, its friendships and sign-ins at once, and the statement with
them, so the name is free again and whoever takes it next has not left.
Someone who left accepts no friend link.

**By the owner.** `dd member remove <name>` moves their id to `revoked` in
[fleet/members.json](../../fleet/members.json) and writes the time under
`removed`; the signed release carries both to every box. Everything of
theirs is kept for the window (`WINDOW`, 30 days;
`dd.verify.leaveWindowSeconds` shortens it in tests), so `dd member add`,
which takes both away (for every root they have had), undoes a mistake.
An id revoked before dates were kept has none and is never scheduled for
deletion. A removal of any root a person has had counts for the person
after a recovery ([gate.md](gate.md#roles)).

When someone is out, and when their things are due, is the earlier of the
two ([leaving.rs](../../servers/gate/src/leaving.rs), `out_at`): their own
word at once, a removal the window after. Both live where every box
agrees on them, never on one box's disk.

**At once**, on every box (the gate's `leaving::tick`, every minute, and
`member()` the moment the statement is replayed):

- they are no member: every page, token and service refuses them;
- every session of theirs in the sign-ins registry ends, their name goes to
  `/run/dd-ended` so Passwords ends its own, and the friend links they
  sent are dropped;
- their devices come off the network (Headscale, the same reap as for a
  removed member, run straight away);
- their game servers stop (the games manager asks its gate every minute;
  the firewall closes them within 15 s anyway).

## Deleting

Once someone is due (at once for their own word, the window after a
removal), each box deletes its own share and reports it. `GET /internal/leaving`
on a box's own gate lists who left, when, whether they are due and which
services have reported; `POST /internal/leaving/done {name, service}`
reports one. Every deleting step is safe to run again; the logs name a
member by the first 8 hex of their member id, never by name.

```
gate (every box)   names/<name>.bin, notes on their libraries and their
                   progress in others', friendships, their own guest
                   record, their sign-in history                   leaving.rs
libraries          every object under each of their libraries' prefixes
                   and images-<name>, trash and all (the gate that holds
                   the bucket key)                                 dav.rs forget
photos             their Ente account, deleted by museum itself once the
                   box lists them in its dd_delete table; no
                   admin account, every 5 minutes                 dd-forget-photos
passwords          their Vaultwarden account, through upstream's admin
                   delete on loopback, with a token made at boot,
                   every 5 minutes                                dd-forget-passwords
forge              their own repositories, then their account; what they
                   gave to others' repositories stays, under Ghost,
                   every 5 minutes                                dd-forget-forge
games              every server and kept world of theirs          games forget
mail               their <name>@<domain> rule, its destination if no other
                   rule uses it, and what the Worker pinned        mail Worker leaving.js
usage              their usage facts stop when there is nothing left to count
```

Which libraries go is decided with care: an entry is a document the person
writes, so only well-formed ids (32 lowercase hex), never the demo's, and
only ids no other entry in the directory names are deleted
(`leaving::deletable`); the rest are logged and kept. `images-<name>`
needs a valid name.

The mail Worker deletes only on evidence it checks itself
([lib.rs](../../client/mail/src/lib.rs) `forget_due`): the member's own
statement, fetched from `home.<domain>/_dd/leaving/<name>` with its read
key (the statement is read as the entry is) and verified against the
entry the Worker pinned (or the directory's newer one, if it follows from
the pinned one by the accept rule: a recovery since), said by now and not
a coming back; or a removal date 30 days ago or more, for any root the
entry has had, in the member list it was deployed with.
The front door's gate asks it (`VERIFY_MAIL_FORGET`).

Forgejo's admin account is never deleted this way; the forge says so and
keeps it until another admin is named. A Passwords account that is the
last owner of a shared organization, or a forge account that last owns an
organization, is refused by the service: the unit logs it and tries again
every hour until someone hands the organization over.

### The guarantee

From the moment someone leaves:

- **At once**: nothing opens for them.
- **Within minutes**: every live copy is deleted, as each box's next run
  comes round (the gate every minute, the other units every 5 minutes;
  museum purges the account's data and its files' objects over the next
  hours, `museum-delete-now.patch`, [packaged.md](packaged.md#the-patches)).
- **By 90 days at most**: no copy anywhere: the backups and snapshots
  expire on their own (1 + 35 + 7 = 43 days).

From the moment someone is removed: nothing opens at once, every live
copy is deleted by 30 days and an hour, and no copy is left anywhere by
30 + 1 + 35 + 7 = 73 days (Photos': 30 + 1 + 0 + 0 + 7 = 38 for the live
objects). `checks.retention` holds the removal's sums, the longer of the
two.

What stays: their entry and statement in the control plane's agreed log
(their name, public keys, and library keys sealed to keys only they hold,
for libraries that no longer exist) and the name with it, so nobody else
can take it. The log keeps every item; compacting it is not built.
Headscale keeps a user record by their name with no devices.

### Coming back

- **By their own word**: there is none. Everything was deleted when they
  left. Settings shows someone who left the date they left and that it is
  gone; their name stays theirs (below).
- **Removed**: only the owner can undo it (`dd member add`, then release),
  and within 30 days nothing was deleted.

### Testing it

[leaving.nix](../../nix/tests/leaving.nix) runs it with a two-minute
window (`dd.verify.leaveWindowSeconds`): `dd leave` without `--yes` does
nothing; a member leaves with `dd leave --yes`, their session and token
stop at once, they are due at once, and their library, disk images,
notes and names are gone within the gate's next minute; leaving again is
refused; a second member removed by the owner keeps their library until
the window is out, then loses it; nobody else's is touched. The schedule is
unit-tested in leaving.rs, the statement in the identity crate, the
Worker's check in client/mail, the games manager's share in its own
tests, and the library prefix check in dav.rs.

## Announcing a change

```
dd policy announce change.md --takes-effect 2026-12-01 [--dry-run]
```

([policy.rs](../../client/cli/src/policy.rs)). It refuses a date less than
30 days away. It mails every member, by name, at their `<name>@<domain>`:
everyone on the member list or in by an invite the release key signed,
not revoked and not gone by their own word. Cloudflare forwards it to the
address each gave the mail Worker; no box ever holds that address.

The mail leaves from the box with the vault role, which already writes to
members ([member-mail.nix](../../nix/modules/member-mail.nix)): `dd` puts
the change in `/var/lib/dd-announce/outbox` over ssh, and `dd-announce`
(every five minutes) refuses it there too if it is under 30 days away,
mails each member once, and keeps what it sent in `sent/`. `dd` then adds
a line to [docs/policies/changes.md](../policies/changes.md): what
changed, when it was announced, when it takes effect, and the commit.
