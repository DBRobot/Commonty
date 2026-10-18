# Identity: one judge, one agreed state

Status: being built (branch `idjudge-1017`). This page is the contract the
judge and its readers are built against. Until the readers switch, the gates
keep judging the directory themselves (servers/gate/src/directory.rs) and the
judge runs beside them in shadow.

The shape: everything a person signs about themselves goes into the control
plane's agreed log; the control plane judges each item once, as it is
agreed; the result is agreed state; everyone else reads it. No second copy
of the rules anywhere.

```
app / dd / browser ──signs──▶ gate ──submits──▶ control plane log
                                                   │
                                     judged once, in order (state/ident)
                                                   │
                                          agreed identity state
                                     ┌─────────────┼──────────────┐
                                 gates (read)   Workers (read)  placement
```

## 1. What is signed: the envelope (v2)

Every signed item is an envelope. The bytes that were signed travel as they
were signed - base64, so nothing on the way (the log's JSON, a re-encoder)
can change them - and a checker verifies the signature over those bytes
before it reads anything in them. No checker ever rebuilds the bytes from a
parsed value, which is what tied the old format to one language: any
language checks a v2 item with a hash, a signature check and a JSON parse.

```json
{
  "v": 2,
  "kind": "entry",
  "body": "<base64 standard, padded: the body's bytes, exactly as signed>",
  "sig": "<base64: an ed25519 signature, or a passkey assertion (below)>",
  "recovery_sig": "<entry only, optional: the same message signed by the recovery key on file>"
}
```

- **message** = `"commonty " + kind + " v2" + 0x00 + body bytes`. The kind and
  version are in what is signed, so a signature over one kind never counts as
  another.
- **body** is UTF-8 JSON, written however the signer's language writes it. Its
  fields are the item's, below. Readers parse it after the signature passes;
  unknown fields are kept in the agreed state as they came, never dropped.
- **an ed25519 signer** signs the message itself.
- **a passkey signer** (a `webauthn:` root) signs with a WebAuthn assertion
  whose challenge is `sha256(message)`. `sig` is then base64 of
  `{"authenticatorData","clientDataJSON","signature"}` (each base64url, as the
  browser hands them over) - the same assertion shape as today.

### Kinds

| kind | signed by | body (JSON fields) |
|---|---|---|
| `entry` | the root the body names; a changed root also by the recovery key on file (`recovery_sig`) | `name, root, recovery, recovered_from[], devices[{fingerprint, public_key, added}], passkeys[{id, cred, added, rp_id?}], grant?, libraries[], terms?, version, updated` - the fields of `identity::Entry` |
| `leave` | the root on file for `name` | `name, root, at, back?` |
| `invite` | the release key | `public_key, issued, expires` |
| `enlist` | the root on file for the member | `member` (sha256 hex of the root), `box` (the joined box's name), `ticket` (the id of the join ticket it joined with, 32 hex), `at` |
| `unenlist` | the root on file for the member | `member, box, at` |
| `extend` | the root on file for the fleet's admin (the release's `identity.admin`, fleet/policy.json `owner`) | `name` (whose grace), `until`, `at` (seconds) |

A grant inside an entry body keeps today's form - `{invite: {invite:
{public_key, issued, expires}, signer, signature}, redeemed, proof}` -
because both of its signatures are already over fixed text any language
writes the same: the release key signs
`{"public_key":"<s>","issued":<n>,"expires":<n>}` and the code key signs
`{"invite":"<s>","root":"<s>","redeemed":<n>}` (`proof_bytes`; for a passkey
root, `root` is `webauthn:<credential id>`), both with JSON string escaping.

### Boxes' own items (not signed by a person)

`ack` (a box's gate acknowledging a first claim) is carried by the box's
TPM-signed request like every request; its body is `{"ack": "<the claim's log name>"}`
as today.

`joined` is the state machine's own: when a box joins with a member's
ticket (state/join.go), the join puts `{"joined": {"key", "ticket"}}` in
the directory log under the box's name, at the join's agreed time. It is
how the judge knows, in log order, which ticket a box came with. No gate
can put one: the state machine refuses a directory item with `joined` at
its top, and the judge takes one only under the name its key gives. Once the readers have switched the judge no longer needs acks (the
agreement is the acknowledgement); they are accepted and ignored after that.

### The old format (v1), during the move

Everything in the log today is v1: `{"entry": SignedEntry}`, `{"invite":
SignedInvite}`, `{"leave": SignedLeave}`, `{"ack": hash}`, with signatures
over serde_json's output of the Rust structs. The log stores items with
their keys sorted (the gate submits a `serde_json::Value`), so the signed
bytes are not in the log; they are rebuilt. The judge accepts v1 by
rebuilding exactly serde_json's bytes for the structs above
(state/ident/v1.go: struct field order, `skip_serializing_if` fields left
out when empty, nested JSON values with sorted keys, serde's string
escaping) - checked against bytes the Rust code produces (testdata).

v1 is accepted until every entry in the agreed state is v2 (the app and dd
re-sign their member's entry in v2 on the next update; members are asked to
open the app once). Then v1 is refused and state/ident/v1.go is deleted.

## 2. The agreed identity state

The judge keeps, per name:

```json
{
  "height": 1234, "time": 1792270000000, "changed": 1200,
  "members": {
    "<name>": {
      "id": "<sha256 hex of the current root>",
      "ids": ["<current>", "<recovered from, newest first>"],
      "root": "<base64 ed25519 | webauthn:...>",
      "kind": "ed25519 | passkey",
      "recovery": "...",
      "devices": [{"fingerprint": "...", "public_key": "...", "added": 0}],
      "passkeys": [{"id": "...", "cred": {}, "added": 0, "rp_id": "..."}],
      "libraries": [...],
      "grant": {...},
      "version": 3, "updated": 0, "agreed": 1792270000000,
      "status": "member | none | left | suspended | revoked",
      "left": 1792270000000, "suspended": "<reason>", "removed": 1792270000000,
      "boxes": ["<box name>"],
      "access": "invite | box | none",
      "enlisted": [{"box": "...", "ticket": "...", "since": 0, "qualifies": "compute | storage | gpu | ",
                    "lastSeen": 0, "counts": true, "why": "seen | our fault | extended | gone | not qualifying"}],
      "grace_until": 1792270000000,
      "item": "<the v1 item or v2 envelope as agreed, verbatim>"
    }
  },
  "enlisted": {"<box name>": {"ticket": "<id>", "owner": "<name>", "since": 1792270000000}},
  "handovers": {"<name>": [{"before": "<item>", "after": "<item>"}]},
  "invites": {"<public key>": {"issued": 0, "expires": 0, "item": {}}},
  "outcomes": {"<item hash>": "ok | refused: <why>"}
}
```

- **times**: every time the judge adds (`time`, `agreed`, `left`,
  `removed`) is in ms; `updated` and `version` are the entry's own.
- **status**, in this order (the gate's `member()` and `suspension()`):
  - `left`: their own `leave` (not `back`) was agreed; `left` is when.
  - `suspended`: an id of theirs is on `revoked`, the release's `suspended`
    gives a reason for it and `removed` a date, and the agreed time is
    within 90 days of that date (leaving.rs WINDOW). Past the window:
    `revoked`.
  - `revoked`: an id is on `revoked`. Nothing below lets them back in - not
    `members` naming another of their ids, not a grant, not a box.
  - `member`: they have access (below): an id is on `members`, or a grant
    the release key signed, or a box of theirs that counts.
  - `none`: none of the above (signed up, not let in).
- **access**: `invite` (on `members`, or a grant), `box` (an enlisted box
  that counts, docs/dev/joining.md "Access"), else `none`; `none` whatever
  else for `left`, `suspended` and `revoked`. What the gates open the
  services by.
- **enlisted**: their standing enlistments, each with what its own report
  qualifies it as and whether it keeps their access now. The top-level
  `enlisted` is the same by box: whose each is and the ticket it was
  enlisted with (what the pending backups and the purge go by).
- **grace_until**: the admin's extension, while it runs.
- **removed**: the release's `removed` date for any of their ids (the
  newest), whatever their status: what a reader that forgets a removed
  member counts from (the mail Worker).
- **ids**: the current root's id first, then the roots it recovered from,
  newest first.
- **boxes**: enlistments that stand: by a root that is still theirs, and
  they have not left.
- **item**: what was agreed, so any reader that wants to can check the
  signature itself.
- **outcomes**: what became of each item, by its hash, for the writer
  waiting on it. Kept for the last 1024 items.

### Item hash

One definition, used by the judge (`outcomes`, Go `ItemHash`) and by every
gate (refusing an item already sent or already judged, Rust
`identity::item_hash`); the shared vectors hold the two to the same bytes:

- **sha256 of what was signed and the signatures over it**, so one signed
  item has one hash however its outer JSON is written or re-wrapped, and a
  forgery over the same body never shares the real item's hash (its refusal
  never stands for the real one):
  - a v2 envelope: `commonty <kind> v2` NUL body, then NUL `sig`, and NUL
    `recovery_sig` when there is one
  - a v1 entry: the canonical entry, NUL `signature` (NUL
    `recovery_signature`); an invite: the invite, NUL `signature`; a leave:
    the leave, NUL `signature` - each as serde writes it
  - anything else (a box's ack, or what reads as none of these): the item's
    JSON, keys sorted
- **The log name** is different and only for v1's claims and acks: sha256
  of the item's JSON, keys sorted (Go `LogName`). Acks name the claim they
  acknowledge by it; it goes when acks do.

### How readers get it

- **On a box**: `GET http://127.0.0.1:4195/identity` (dd-control) returns the
  whole state above; `?name=<name>` returns `{height, time, changed, name,
  member}` for one (404 when the name has no entry); `?since=<changed>`
  returns `304` while the answer is the same as when `changed` was given
  (`changed` is the height the answer last changed at on that server; a
  reader keeps the `changed` it last got, per server).
- **Cloudflare Workers** (mail, DNS): the control relay's Durable Object
  serves `/identity` beside `/leases`, the same queries and answers, from the
  witness's agreed state (cmd/witness exports `controlIdentity`), to Workers
  that bind it. Never reachable from outside (the front passes only
  `/relay` and `/health`). 503 until the witness runs.
- The judged state is a view kept beside the agreed state, not part of it:
  each server judges the agreed log itself (state/ident Replayer), so a
  judge bug cannot split the validators while the gates still decide.

## 3. The rules, once

These are today's rules (identity::accept, the gate's replay, leaving.rs),
moved, not changed. Each item is judged at its agreed time `at`.

- **entry, a name nobody holds (a claim)**: the body must pass `accept`
  against nothing (well-formed name, at least one device unless a passkey
  root, no earlier roots, valid library ids, signed by its own root); a grant
  must verify and be unused by any other root and not expired at `at`; its
  library ids must be no other entry's. Until the readers switch, a good
  claim waits until every voting box's gate has acknowledged it (an `ack` per
  voter, voters = validators that are boxes), and the first acknowledged in
  log order takes the name; later claims to the name are refused, an update
  by the same root that waited behind it lands after it.
- **entry, an update**: strictly higher version, same name; same root, or a
  new root co-signed by the recovery key on file (then the old root joins
  `recovered_from`, and a handover is kept); only the recovery key changes
  the recovery key; a passkey root's update is checked with the passkey on
  file and must keep that credential; grant checked as above (an unchanged
  grant is not checked against the window again).
- **guest names** (`guest*`): gone 15 minutes after their last update, by
  agreed time.
- **invite**: signed by the release key; refused if expired at `at`; kept
  until it expires.
- **leave**: signed by the root on file for the name; its `at` within 5
  minutes ahead of / 24 hours behind the agreed time; the newest statement
  stands, the same one twice is a no-op, an older one is refused.
- **enlist / unenlist**: signed by the root on file whose id is `member`;
  `at` within 5 minutes ahead of / 24 hours behind the agreed time, and
  newer than the last enlist or unenlist taken for that box (kept after an
  unenlist), so an old one sent again changes nothing. An enlist names the
  ticket the box joined with (its `joined` record): signed by a device in
  the member's entry, for them (`user`), for `join`, not expired at the
  enlist's agreed time, and never used before; once enlisted, the ticket is
  used for good, even after an unenlist. The ticket is read by the judge
  itself (state/ident ticket.go: the authority block of biscuit-auth's
  format, its signature, its facts and its one time check). A member who
  is left, suspended or revoked enlists nothing. A box already someone's is
  refused while that enlistment stands (see **boxes** above); unenlist only
  by its owner. Which boxes count for access is the view's (state/ident
  access.go, docs/dev/joining.md "Access").
- **extend**: signed by the root on file for the release's admin; dated
  like a leave, the newest standing; an `until` already past takes it back.
  It keeps a member's access while their enlisted boxes are gone, until
  `until`.
- **an item that says more than its signature** is refused: unknown fields,
  a key twice, keys in another case, `null` for a field left out, floats,
  anything that does not read back to the same value that is checked.
- **before the log**: a box's gate puts the entries and unexpired invites it
  held before the log into it once (directory.rs `migrate`), as ordinary v1
  items, signatures as they were. They are judged like any other: a claim,
  acknowledged, first in log order. No original agreed time is carried;
  nothing before the log was agreed. Leaves held before the log
  (leaving.json) are not carried over; on the real fleet there are none.
- **the release's member list** (`fleet/members.json` in the release, the
  release key, and the admin, fleet/policy.json `owner`) is part of the
  control plane's config, as the box list is.

## 4. Shadow check

Before any reader switches, `state/ident` replays the real log and must end
at exactly the members, keys, statuses and outcomes the gates' replay ends
at (`go test ./state/ident -run Shadow` with a log fetched from a box, and
the gate's `shadow_dump` test on the same file). Any difference is a bug in
one of the two, written down here and fixed before the switch.
`TestShadowFixture` holds the judge to the gate's dump of a made-up log of
every case (testdata/shadow-gate.json; regenerate it with the gate's
`shadow_dump` whenever vectors.rs changes its `shadow_log`).

Results:

- 2026-10-17, the real log (node1, 10 items: david's pre-log entry from
  each box, migrated, and the acks): identical. Both boxes' pre-log
  directories held only that one entry, both say every pre-log entry is in
  the log, their invites had all expired before the log began, and no box
  has a leaving.json. fleet/members.json lists one member.
- The made-up log (35 items): identical.

### Where the gate and the judge part, on purpose or not

- The gate checks guest expiry and suspension against its own clock
  (`identity::now()` in commit, entry, claims_free, suspension), so two
  gates can disagree for a moment. The judge uses the agreed time only.
- Ed25519: the gate verifies with ed25519-dalek's default (non-strict)
  `verify`, the judge with Go's `crypto/ed25519`. They agree on everything
  an honest signer makes; on crafted edge cases (small-order keys or `R`)
  they may not. Not tested beyond the vectors.
- Floats: the gate's `serde_json::Value` keeps `1.0` as a float; the judge
  refuses any number that is not a 64-bit integer.
- No re-invite after removal: in the gate an id on `revoked` is out for
  good, whatever later grant it holds. The judge does the same. Whether a
  removed member re-invited later (`grant.redeemed` after `removed`) should
  count is a decision, not a rule either side has.
- The envelope (v2) signs the body's bytes as they were written, not a
  length-prefixed binary form: JSON every language reads, and nothing is
  rebuilt to check a signature.

## 5. Readers outside the gate and the Workers

Two kinds of reader, and they read differently on purpose:

- **On a box** (the gate, the library, placement): reads the agreed state
  (§2) and judges nothing. The library and the media gate only use
  `identity`'s helpers (fingerprints, library ids); they never judged.
- **A person's own device, and the home controller** (the app, `dd`, the
  git remote, the browser, the door): checks signatures itself and takes no
  box's word, since a box may hand it a forged entry to read what it writes
  (a library key of the box's own making, say). It does not re-judge the
  fleet's rules: it checks the signature over the bytes signed, and holds the
  entry to an anchor it already has (a root it pinned, or the release's
  member list), exactly as before.

Both forms read into the one `identity::SignedEntry`: a v2 envelope keeps
its body as signed (`signature` holds `v2:<body>:<sig>`, and it is written
back out as the envelope), so `verify`, `accept`, `anchored` and
`directory::resolve` check either form with no change to any caller.

| component | judged | now | dead |
|---|---|---|---|
| client/directory `resolve` | signature + agreement + anchor | same, v1 or v2 | nothing |
| client/account, client/media, app | through `resolve` / `verify` | same, v1 or v2 | nothing |
| client/gitremote | signature, directories agree on the root | same, v1 or v2 | nothing |
| client/web (browser) | signature of its own entry | same, v1 or v2 | nothing |
| servers/smarthome/door | `accept` against the root it pinned at pairing | same, v1 or v2 | nothing: the home trusts no box, and reads no agreed state |
| servers/library, client/media gate | nothing (helpers only) | unchanged | nothing |
