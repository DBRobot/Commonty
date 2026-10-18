# Calendar and contacts

A member's calendars and address books are folders in their own
[encrypted library](libraries.md): one `.ics` file per event, one `.vcf`
file per person. The Calendar and Contacts pages read and write them in
the browser tab, the same way Files does, and no box can read a title,
a time, a name or a number. This page is how that fits together, and how
to change it.

## The layout

```
library <id>/                    names and bytes locked on the device (rclone crypt)
  Calendars/
    personal/
      displayname                "Personal"
      color                      "#1D5C42"
      <uid>.ics                  one VCALENDAR: an event and its changed occurrences
    family-x3k9/ ...
  Contacts/
    contacts/
      displayname                "Contacts"
      <uid>.vcf                  one VCARD
```

This is the vdir layout that vdirsyncer, khal and khard read, so the
library's rule holds: no format of ours. `dd media` (or `rclone mount`
with the library's key) and khal open a member's calendars with nothing
of Commonty's running. A file holds one UID: the event, and any changed
occurrences of it (`RECURRENCE-ID`) beside it. A file name is the UID
when that is a plain name, otherwise a fresh one, as CalDAV programs do.

A calendar's first folder is made the first time the page opens on an
account with none (`first` in [vdir.js](../../servers/calendar/web/vdir.js)):
`Calendars/personal` and `Contacts/contacts`.

## Saving from a version

Two devices may change the same event. Every save names the version it
was made from, and the gate refuses one made from an older version (412):

```
read      GET   -> the text, and its ETag
save      PUT   If-Match: <etag>        a change
          PUT   If-None-Match: *        a new item; refused if the name is taken
delete    DELETE If-Match: <etag>       to the trash
list      PROPFIND depth 1 -> each file's getetag
```

The gate checks the version and writes under that object's lock
([libraries.md](libraries.md#webdav)). A page told 412 reads the item
again, shows it as it now is, and says the change was not saved. A change
or delete keeps the old version in the library's trash for 30 days, as
every library write does.

## The pages

```
servers/calendar/web/
  pages/calendar.html   the frame: head, rail, panel, the event form
  calendar.js           week, month and day list; the event form; import
  calendar.css          what Calendar and Contacts add to home.css
  ical.js               iCalendar and vCard: parse, write back, time zones, repeats
  vdir.js               the folders above, over library.js, kept on the device
servers/contacts/web/
  pages/contacts.html
  contacts.js           the list, a person, the contact form, import and export
servers/home/web/library.js
  libraries()           every library this device opens, own first, then shared
  readText, saveText, trashFrom   a small file by version
```

Both pages are served by the app, never by a box, like Files: the gate
hands a member's browser to the app (`/_dd/calendar`, `/_dd/contacts`,
`library_page` in the gate), and the app carries the files
([app/carried](../../app/carried)). The app's `/_dd/app/libraries`
([site.rs](../../app/src/site.rs), `libraries_value`) gives a page every
library this device opens, with its owner and whether this member reads
or writes it.

What a page has read is kept on the device in IndexedDB (`dd-vdir`), by
library, path and ETag. A page draws what it kept at once, then lists
each calendar (one PROPFIND) and fetches only the files whose ETag
changed, eight at a time.

### iCalendar

[ical.js](../../servers/calendar/web/ical.js) parses a file into its
components and properties and writes back only what the page changed:
anything another program put there (Google's and Outlook's own
properties, a contact's photo) is kept as it was.

- **Times** are worked out in the event's own zone with the browser's
  zone data (`Intl`). Outlook's Windows zone names and Mozilla's
  `/mozilla.org/.../America/New_York` are mapped to the zone they mean;
  a TZID the browser does not know is the viewer's own clock.
- **Repeats**: `FREQ` daily, weekly, monthly and yearly with
  `INTERVAL`, `COUNT`, `UNTIL`, `BYDAY` (with a position: "the second
  Tuesday", "the last Friday"), `BYMONTHDAY`, `BYMONTH`, `BYSETPOS` and
  `WKST`; `EXDATE`, `RDATE`, and changed occurrences by `RECURRENCE-ID`.
  Each occurrence is placed on the event's own wall clock, so a weekly
  meeting stays at 9:00 across a change of the clocks.
- **Writing**: a new or changed event's times are written in the
  viewer's zone with its IANA name as `TZID`, a day's event as
  `VALUE=DATE`. Editing one occurrence of a series adds a changed
  occurrence to the file; "this and later ones" ends the series
  (`UNTIL`) and starts a new one; deleting one adds an `EXDATE`.

Its tests run under node: `checks.calendar-js`
([nix/tests/calendar-js](../../nix/tests/calendar-js/ical.test.mjs)).

## Sharing a calendar

A calendar two people keep is a library of its own, shared: the owner's
entry names the other member in the library's `readers`, with `writes:
true` for one who may change it ([libraries.md](libraries.md#who-may)).
A member's main library is never shared for a calendar, since one key
opens all of it.

```
Calendar  New shared calendar   app POST /_dd/app/share { readers }: a new library,
                                its key sealed to each member's devices; the page
                                then makes Calendars/<name> in it
          Sharing (on one)      app POST /_dd/app/share { id, readers }: who, replaced
dd library share <lib> <member> [--write]
dd library unshare <lib> <member>
```

Both sign the member's entry with the account's main key, so they work
on the device that holds it, and in the app's own window only (as adding
a device does), never in a browser it let in. The key is sealed to the
other member's keys out of their entry as the release's member list holds
it (`account::set_readers`): a box cannot slip in a key of its own under
a member's name. Taking someone off removes them from `readers`, and the
gate refuses them from then on; what they copied off before stays theirs.
The library's key is not changed when someone is taken off, so a copy of
the ciphertext they kept would still open; a fresh shared calendar is the
way to a new key. A key is sealed to the devices a member's entry names
when it is shared; a device they add later opens the calendar once the
owner shares it with them again (Sharing, Save).
The gate gives that member the writer's role; what they save counts
against the owner's allowance, and only the owner deletes for good. A
library shared without `writes` is seen and not changed; the page hides
its edit controls and the gate refuses the writes.

## Birthdays

Calendar's **Birthdays** reads the member's contacts on the device and
draws a day's event on each `BDAY`. Nothing is written for it.

## Changing it

- A change to a page changes its copy under `app/web/_dd` in the same
  pull request (`one-set-of-pages`).
- A change to how a time or a repeat is worked out comes with a case in
  `nix/tests/calendar-js/ical.test.mjs`.
- Look at both pages in light and dark, at phone and desktop width.
