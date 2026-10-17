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
| `enlist` | the root on file for the member | `member` (sha256 hex of the root), `box` (the joining box's name), `key` (its TPM key, SPKI base64), `ticket` (32 hex), `expires` |
| `unenlist` | the root on file for the member | `member, box, at` |

A grant inside an entry body keeps today's form - `{invite: {invite:
{public_key, issued, expires}, signer, signature}, redeemed, proof}` -
because both of its signatures are already over fixed text any language
writes the same: the release key signs
`{"public_key":"<s>","issued":<n>,"expires":<n>}` and the code key signs
`{"invite":"<s>","root":"<s>","redeemed":<n>}` (`proof_bytes`; for a passkey
root, `root` is `webauthn:<credential id>`), both with JSON string escaping.

### Boxes' own items (not signed by a person)

`ack` (a box's gate acknowledging a first claim) is carried by the box's
TPM-signed request like every request; its body is `{"ack": "<item hash>"}`
as today. Once the readers have switched the judge no longer needs acks (the
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
  "height": 1234, "time": 1792270000000,
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
      "status": "member | invited | none | left | suspended | revoked",
      "left": 0, "suspended": "<reason>",
      "boxes": ["<box name>"],
      "item": "<the v1 item or v2 envelope as agreed, verbatim>"
    }
  },
  "handovers": {"<name>": [{"before": "<item>", "after": "<item>"}]},
  "invites": {"<public key>": {"issued": 0, "expires": 0, "item": "..."}},
  "outcomes": {"<item hash>": "ok | refused: <why>"}
}
```

- **status**:
  - `member`: some id is on the release's `members` and none on `revoked`;
    or a grant the release key signed; or an enlisted qualifying box
    (docs/dev/joining.md).
  - `revoked` / `suspended`: an id is on `revoked`; `suspended` when the
    release's `suspended` gives a reason.
  - `left`: their own `leave` (not `back`) was agreed.
  - `none`: an entry that is none of the above (signed up, not let in).
- **item**: what was agreed, so any reader that wants to can check the
  signature itself.
- **outcomes**: what became of each item, by its hash, for the writer
  waiting on it. Kept for the last 1024 items.

### How readers get it

- **On a box**: `GET http://127.0.0.1:4195/identity` (dd-control) returns the
  whole state above; `?name=<name>` one member; `?since=<height>` returns
  `304` when nothing changed since.
- **Cloudflare Workers** (mail, DNS): the control relay's Durable Object
  serves `/identity` beside `/leases`, from the witness's agreed state, to
  Workers that bind it. Never reachable from outside (the front passes only
  `/relay` and `/health`).

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
- **enlist / unenlist**: docs/dev/joining.md (the hardware rules live there).
- **the release's member list** (`fleet/members.json` in the release, and
  the release key) is part of the control plane's config, as the box list is.

## 4. Shadow check

Before any reader switches, `state/ident` replays the real log and must end
at exactly the members, keys, statuses and outcomes the gates' replay ends
at (`go test ./state/ident -run Shadow` with a log fetched from a box, and
the gate's `shadow_dump` test on the same file). Any difference is a bug in
one of the two, written down here and fixed before the switch.
