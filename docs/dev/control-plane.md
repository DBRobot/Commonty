# The control plane

Status: running on node1 and node2 with the Cloudflare witness (since
release 176). Boxes heartbeat into it; the gates keep the directory in it
(below). It plans placement as a dry run at `/plan`; nothing acts on the
plan yet.

## What it is for

Every box needs the same answer to three questions, without any box being
able to impose it:

1. Which boxes are alive, and what they have (cores, RAM, disk, load, heat).
2. Who is the one writer for each single-writer role (Postgres, Forgejo)
   right now.
3. Who belongs: which houses, boxes and witnesses.

From those, every box computes the same role placement (a pure function, in
the signed release) and starts or stops only its own roles. The control plane
gives nothing orders. It keeps one agreed log, and the log only holds
requests that pass the rules.

k3s and etcd/Raft are out: whoever leads them can write anything.

## Shape

```
  box ──ws──┐                       ┌──ws── box
  box ──ws──┤   relay (one Durable  ├──ws── validator (house A)
  box ──ws──┤   Object, Cloudflare) ├──ws── validator (house B)
            └───────────────────────┴──ws── witness 1, witness 2
```

- **Validators** run SmartBFT (github.com/hyperledger/SmartBFT, Apache-2.0,
  the consensus engine in Hyperledger Fabric 3). One validator per house, plus
  outside witnesses until there are four houses. BFT needs n ≥ 3f+1: four
  voters survive one bad or absent voter.
- **Everyone else** (other boxes in a house, and later hundreds of boxes)
  reads the agreed state and submits signed requests. They never vote.
- **No box talks to another box.** Every box dials out to the relay over one
  WebSocket. The relay forwards frames addressed to one node, to the
  validators, or to everyone. Every frame is signed, so the relay can delay or
  drop messages but never forge them. Safety never depends on it; liveness
  does.

## What is voted on

The validators order and validate signed requests. Each request is accepted
only if it passes the rule, run identically by every validator:

| Request | Valid when |
|---|---|
| heartbeat {box, seq, facts, roles running} | signed by that box's TPM key from the signed box list; seq above the last |
| lease acquire {role, box} | signed by the box; lease free or expired; placement (from agreed state) says the box may hold it |
| lease renew {role, box, epoch} | signed by the current holder, before expiry |
| lease release {role, box, epoch} | signed by the holder |
| membership {add/remove house, box, witness} | signed by the owner key(s): today the release key, later k of n house owners |
| directory {an entry, an invite, or an ack} | signed by a box of the fleet; at most 64K; at most 60 a minute per box. Its content is the gates' to judge, not the validators' |
| built {a build result} | signed by a box of the fleet; at most 16K; at most 20 a minute per box. Its content (what the box built for a build order, signed by its host key) is the laptop's to judge, not the validators' |

Rules, copy counts and role requirements come from the signed release, not
from the log.

**Time.** The proposer stamps each batch. A validator rejects a batch whose
stamp is more than 2 s from its own clock and earlier than the last one.
Lease expiry uses this agreed time, so no single clock can end a lease early.

**Fencing.** Each lease grant raises the role's epoch. The holder stamps its
epoch on what it writes (WAL segments, for example), and readers ignore
anything from a lower epoch. A writer that wakes up confused can't win.

## The directory

Who people are (servers/gate/src/directory.rs) lives in the agreed log, not in
copies the boxes pull from each other. No box talks to another for it.

- A write (a new person, a device added, a passkey changed, an invite) is
  checked by the gate it reaches, then handed to that box's dd-control on a
  socket only the gate can open, and goes into the log as that box's
  request.
- The validators order directory items and judge none. Every gate replays
  the log through the same rules (identity::accept, the invite and library
  checks), at each item's agreed time, so every box ends at the same
  directory. One implementation of the rules, in the gates.
- An update is the owner's own signature and lands as soon as it is in the
  log.
- A name nobody holds yet is a claim. It is the claimant's only once the
  gate of every voting box has acknowledged that very claim in the log. While
  there are three voters (not yet Byzantine-safe, see the README), a lying
  voter can only stall a new name, and never give it to two people.
- While agreement is down, nobody signs up or changes a device. Signing in,
  and every service, keep working from each box's copy.
- Each item is visible to the relay, Cloudflare included: names, public
  keys, passkey ids. It was before too, through the tunnel. Encrypting the
  items to the boxes' TPM keys is in the README's Cloudflare TODO.
- A house's Wi-Fi relay still goes box to box inside that house, on 4181
  and to that house's boxes alone. It moves to the log once house items
  can be encrypted to the house's boxes.

## Timing

| | Value | Why |
|---|---|---|
| batch interval | 2 s | agreement within seconds, and fits the free tier |
| lease TTL | 15 s, renewed every 4 s | failover in about 15–20 s |
| ordinary heartbeat | 30 s | liveness for placement; cheap |
| a box counts as gone | 3 heartbeats missed (90 s) | stateless roles move |
| SmartBFT leader heartbeat / view change | 2 s / 4 s | Fabric's defaults (1 min / 20 s) are far too slow |

Free tier, as estimated: about 55k billable requests/day, under the 100k limit
(Durable Objects bill incoming WebSocket messages at 20 per request; outgoing
messages are free). Every box heartbeating fast would need Workers Paid,
about $5.50/month.

## When things fail

- **The relay is down.** Nothing new is agreed. Running roles keep running.
  Leases can't renew, so writers stop writing when theirs expire (15 s).
  Nothing unsafe happens. A second relay can be added later; frames are
  deduplicated by hash.
- **A validator is down or lying.** With n ≥ 4, one is tolerated. A lying
  proposer can only omit requests, and the proposer rotates.
- **More than a third of validators are down.** The fleet halts safely, as
  above. Enough houses make this rare.
- **A box lies about its facts.** Caps per box and copies spread across
  houses limit it. The data it attracts is end-to-end encrypted.
- **A box can't reach the relay.** It stops any writer role when its lease
  runs out and keeps serving read-only where the role allows.

## Trust

- Box keys are the TPM-held keys already in fleet/boxes.json (unlockKey).
  Validator keys are the same keys.
- Voting weight is per house: a house runs one validator however many boxes
  it has.
- Witnesses are validators listed in the membership with no boxes behind
  them. They are removed once four houses exist.

## Placement

Every box computes the same plan from the same agreed state
([`place`](../../servers/system/control/place/place.go)), from the
release's placement inputs ([servers](servers.md#needs-and-placement)) and
what the heartbeats say: who is alive, each line as measured, memory, what
each service holds, the roles each box runs. It is greedy, not optimal:

- What every box runs comes first.
- Then the service the fewest live boxes can run, and so on. What a
  service needs on its own box is decided before it, so the service it
  needs goes where that suits best and the dependent follows.
- A `spread` service (the storage pool) goes on every live box that can
  hold it; a `perHouse` one on that many boxes in each house; the rest on
  their count, one if unsaid.
- A box is ranked by its score, by its line against the traffic the
  service wants, by what already sits beside it, and only as it fills past
  60% of its memory, by room. Spreading load is not the aim.
- A service that needs one already at its count goes where that one is:
  one vault and its send page together, one postgres for all its users.
- Memory is what a box measured the service holding, never below what it
  keeps aside (the largest model), and the needs' estimate until measured.
- What runs stays unless another box is better by a margin: 0.3, plus
  0.01 for each GiB the move would carry. 100G of game worlds moves for a
  much better box; a stateless page moves for a clearly better one.

`dd-control -placement` serves the plan at `127.0.0.1:4195/plan`, each
choice with why. It is a dry run: no box starts or stops anything by it
yet.

## Not in scope

The control plane doesn't run commands, push configuration, hold secrets or
move data. Build orders are not an exception: the order is a file the
release key signs, beside the release, and a box builds because the order
names it, as it moves to a release because the release names it. The log
carries only what each box says it built ([the app's
releases](app-release.md)); the bytes never pass through it. Placement and stateful replication build on it and come after.

## Build order

1. The relay (a Durable Object in the existing Worker) and its tests.
2. The validator: a Go program around SmartBFT holding the state machine
   above. It ships through Nix (buildGoModule, Go 1.26).
3. A VM test: four validators and a fake relay, killing and lying validators,
   relay outages, lease handover and fencing.
4. Run it on the boxes beside everything else, with nothing acting on it,
   and watch.

## Open

- Both boxes are in one house today, so there is one house validator. Four
  voters means three witnesses until more houses join. Who runs them?
