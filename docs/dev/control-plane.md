# The control plane

Status: running on node1 and node2 with the Cloudflare witness (since
release 176). Boxes heartbeat into it; the gates keep the directory in it
(below). It plans placement at `/plan`, and switches the placed services
on and off by it (below): every service but what runs on every box and a
few with no unit of their own. Which boxes have each built in comes from
their capability, not a list kept by hand (below).

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
| heartbeat {box, seq, facts} | signed by that box's TPM key from the signed box list; seq above the last |
| lease acquire {role, box} | signed by the box; lease free or expired; placement (from agreed state) says the box may hold it |
| lease renew {role, box, epoch} | signed by the current holder, before expiry |
| lease release {role, box, epoch} | signed by the holder |
| membership {add/remove house, box, witness} | signed by the owner key(s): today the release key, later k of n house owners |
| directory {an entry, an invite, or an ack} | signed by a box of the fleet; at most 64K; at most 60 a minute per box. Its content is the gates' to judge, not the validators' |
| built {a build result} | signed by a box of the fleet; at most 16K; at most 20 a minute per box. Its content (what the box built for a build order, signed by its host key) is the laptop's to judge, not the validators' |
| rollout {begin a release's rollout, or a box's verdict on it} | begin: carries the release file, signed by the release key, with a counter above the record's. verdict: signed by a box of the record's newest admitted wave, about the record's release. Judged at every batch ([staged rollout](rollout.md)) |

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

### When consensus wedges

SmartBFT v1.0.2 can wedge a validator for good: one left a sequence behind
never rejoins (upstream #700), and the controller and view changer can
deadlock (#713, #724). It happened twice on 2026-10-22, and under failover
a wedge stops every writer once its lease lapses.

- **The fixes.** `servers/system/control/third_party/smartbft` is v1.0.2
  with those three upstream commits picked on, and nothing else (go.mod's
  `replace`). Messages, the log and the WAL are unchanged, so validators
  on either side of a release agree. A new leader's heartbeat now names the
  sequence it proposes next, so a follower still on v1.0.2 looks one behind
  and syncs every few seconds until it is switched too. Drop the copy for
  the first upstream release that has them.
- **Nothing waits on consensus but consensus.** The relay's reader queues
  frames for it and goroutines of their own hand them on, so a wedged
  validator never also holds up its peers' catch-up: a peer's "send me
  your latest" is answered from what was last decided, however consensus
  is doing.
  - Votes come only from validators; any other sender's are dropped. They
    wait in a queue of their own (4096, the oldest dropped first, said in
    the log), apart from the boxes' requests, so a flood of requests can
    push out only requests, never a vote.
  - A request passed on comes only from a validator. A request is checked
    before it is queued: at most 20 a second (200 at once) from any one
    sender but the box itself, and only one some box signed goes on.
- **A validator heals itself.** Every 5 s it checks itself. It exits, and
  systemd starts it afresh (`Restart=always`), when:
  - a call into consensus, or the delivery of a decision, has not returned
    for 30 s;
  - its peers are at a higher height that a quorum signed, and its own has
    not moved for 60 s;
  - requests have waited 120 s, enough peers answer to make a quorum, and
    no height has moved. Only a request signed by a box of the fleet, newer
    than what was agreed of that box and not a join, counts as waiting:
    junk or a replay from whoever reaches the relay never makes a stall.

  And only when all of these hold too:
  - it is more than 60 s since it started, and more than 10 minutes since
    it last healed itself, whatever the reason;
  - it is its turn: the validators take 30 s slots in the order of their
    ids, so no two restart at once;
  - a peer it heard from in the last 30 s is at least as far as it is, so
    it is never the only one up to date when it goes.

  While any sign stands, it asks the peers for their latest to learn their
  heights; while decisions come, it asks nothing. A fleet that is down (no
  peers answer) is not a wedge, and nothing restarts.
- **It says so.** The log line starts `self-heal:` and gives the reason.
  `/var/lib/dd-control/selfheal.json` keeps the count across restarts, and
  `/var/lib/dd-facts/control.prom` has `dd_control_selfheal_total`,
  `dd_control_selfheal_last_seconds`, `dd_control_height` and
  `dd_control_dropped_total`. The fleet's alert mail names a box that
  healed itself in the last day.
- The witness runs the same validator, queue included. It can't restart
  itself; a stuck witness is one voter down, and the boxes still decide.

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
each service holds, what the release builds each box with (what is not
switched runs wherever it is built). It is greedy, not optimal:

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
  On a box that measured it itself, what it held there: a CI runner takes
  as many jobs as its box holds, so a small box's is not sized by a big
  one's.
- What runs stays unless another box is better by a margin: 0.3, plus
  0.01 for each GiB the move would carry. 100G of game worlds moves for a
  much better box; a stateless page moves for a clearly better one.
- A service whose needs say `moves = "on-failure"` stays on the live box
  that runs it whatever the scores, and whatever that box can now run or
  hold: fit and room are checked only when choosing a new box, once that
  one is gone. One that stays where it no longer fits is named in the
  plan's `doesNotFit` for that box.
- What a box can run is the release's table (its signed report, judged
  by the release's rules), narrowed by its heartbeat's word when it sends
  one (`can_live`, `can/<service>`: the same rules on its report as it is
  now, [servers](servers.md#needs-and-placement)). Its word only takes
  away: a service the release does not give it, its `barred` (its owner's
  trust and line choices) or a lease the release does not let it hold
  never comes from what it says. New hardware counts from the release
  that carries its new report.
- A box that joined on its own key is in no release's table. The
  release's rules for such boxes (`joined` in the placement inputs) name
  what a member's box may run by the policy's trust tiers (trust `any`,
  no code handing people their secrets, nothing the owner keeps to their
  own hardware, no lease), and its word only narrows that. Until its
  hardware and boot are attested in the agreed state (`attested`, off
  today) it is given no service at all and holds copies only; and what it
  says it runs counts for nothing, so it can neither keep a service nor
  draw a name.
- A box whose facts say `draining` (below) is given nothing new: no
  service, no new copy. What may move moves off it; what moves on failure
  stays until the box leaves. A copy it keeps already stays listed only
  while no other store has room for it.

- Members' remote computers ([remote computers](remote-computers.md))
  are placed after the release's services, each as the service
  `computer-<id>`, from the computers agreed at the same state that are
  `up` (dd-control's own `/computers`, never judged again here). A
  computer is plaintext to its host, so only a box of the release whose
  table gives it `computers` and whose owner does not bar them may hold
  one: never a stranger, never a box on its own word, and a box's word
  that it runs one keeps it only there. Each
  goes on one live x86_64 box with KVM whose `computers.prom` room
  (memory, threads, disk) holds it, less what the plan has put there
  already; a computer's disk counts only what it may still grow into on
  a box that keeps some of it. `own` goes only on boxes whose release
  owner (box.json) is the computer's owner; an enlistment is the member's
  word alone and places nothing.
  Those running on a live box stay first, as `moves = "on-failure"`;
  then, by id, the rest go where moving them carries least
  (`dd_computer_used_bytes`), then where most memory is left. A box
  runs a computer as a writer under its lease, `svc/computer-<id>`,
  which the validators grant only while the computer is agreed up and
  only to a box placement may give it (state/computerlease.go,
  `rules.computerHosts`); its flag `/run/dd-placed/computer-<id>` holds
  the lease's epoch, and the fence stops `dd-computer@<id>` before the
  lease may lapse. Powered off, it loses the lease at the next renewal.
  Its room
  (`computers.prom`) is the box's word, never taken past the hardware of
  its signed report.

So when the needs or a box's capability change, the next round plans
again: needs with the release that changes them, a box's hardware with
the release that carries its new report.

### Draining a box

Before a box leaves, its owner runs `dd-drain on` as root on it. That
writes `/var/lib/dd-facts/draining.prom` (`dd_box_draining 1`), which
`dd-control` reads with its other facts (any file saying 1 drains it) and
carries as `draining`. `dd-drain off` removes it. It is on no page.

`dd-control -placement` serves the plan at `127.0.0.1:4195/plan`, each
choice with why.

### Placed services

A service whose needs say `placed = true` is switched by the plan
([`box/placed.nix`](../../nix/modules/box/placed.nix),
[`placed.go`](../../servers/system/control/cmd/dd-control/placed.go)):

- Every service that does not run on every box is placed (or a writer,
  below), but for a few that have no unit of their own: pages nginx and
  the gate serve (`disk-images`, `libraries`, `metrics-page`,
  `forgejo-public`, `ente-web`, `vaultwarden-send`, `dd-dns`), the
  shared pool's `garage`, which every box's agent fetches its releases
  through, and members' computers, each switched by its own lease.
- What a box is built with comes from its capability
  ([`fleet-placement.nix`](../../nix/lib/fleet-placement.nix)
  `carried`), never a hand-set list: every bundle of modules
  ([`nix/roles/bundles.json`](../../nix/roles/bundles.json)) of which it can run a service that does not run
  everywhere (its signed report, its owner's box.json and the policy:
  `capability.nix`), whose secrets its own sops file holds (`secrets`:
  what the bundle's modules read whatever else; [secrets](secrets.md)),
  and whose companions (`with`) it is built with too. It *carries* a
  placed service (`/etc/dd-placed/carries.json`, from `carries`) when it
  is built with it and able to run it, and has units of it: a module
  that runs without a secret the box lacks (`dd.hasSecret`) makes none.
- What keeps state on its box's own disk alone asks for its dataset
  (needs `requires.dataset`: the games' worlds), so only a box whose
  owner gave it one carries it, and a box that would start it empty
  never does. The house's DNS and Tang ask for the house's own box
  (`requires.place = "house"`: its household in box.json), the address
  the router hands out and the keys a controller's card opens with; and
  stay there while it is up (`moves = "on-failure"`).
- Its units start only while `/run/dd-placed/<service>` exists; /run is
  empty at boot, so nothing placed runs until the agreed state puts it
  somewhere. Its units are its own name and its needs' units as they are
  on the box: a name ending in `*` every unit starting so, one ending in
  `@` a template and its instances, with their timers, sockets and paths.
  A unit two services claim is the one's that claims it most closely
  (`gitea-runner-members*` before `gitea-runner-*`).
- When its flag goes up, dd-control starts what of it starts at boot
  (`/etc/dd-placed/starts.json`): a timer, not the run it starts; no
  template's instance. When it goes, every unit of it stops, instances by
  their pattern. The first round that does not give a carried service to
  its box also stops what of it still runs without a flag: the switch that
  first gives a running service its condition stops its services and
  timers, but leaves its sockets listening.
- Each box reports `carries/<service>` and `runs/<service>` in its
  heartbeat. The plan puts a placed service only on a box that carries it,
  and counts it running where its box says so. What the plan does not
  switch it counts running where the release builds it (`built` in the
  placement inputs). Heartbeats name no roles; the agreed state keeps an
  empty `roles` field only while a box of an older release may send one.
- Every 10 seconds each box raises the flags the plan gives it and starts
  their units, and lowers the rest and stops them. Only from agreed state
  at most 30 seconds old, and only from a plan this box is in: a box cut
  off keeps what it runs, and one the others count gone changes nothing
  on its own.
- A box's reports are its own word, signed by its TPM. A box that lies
  about carrying a service can take it from another box, but only among
  the boxes the signed release already lets run it: availability, never
  access.
- Without a lease this is for what may run twice for a moment: two boxes
  running a stateless page while one hands over is harmless.
- Its names (needs `hosts`) follow it: the DNS Worker points each at a
  box the agreed plan gives the service, which the witness computes on
  the agreed state with the release's inputs, never at a box on its own
  word ([failover: the names](failover.md#the-names)). A service that answers
  under another's name depends on it locally, so the two go together;
  `checks.placement` refuses a name two services claim.
- What the rest of the release picks a box by (the storage pool, the
  mail boxes, the ports a member's device may reach, the observe
  sidecar, the app's clients) is the services the box is built with
  (`nix/lib/fleet-placement.nix` `built`), never a role's name.

### Checking a plan against the boxes

`scripts/plan-check` says what a release would change, before it ships.
It reads the agreed state from a box's control plane (`/state`) and each
box's `systemctl list-units`, nothing else, and plans with the release's
own inputs (`.#plan-release`: its placement inputs and needs, and each
box's `built-<box>.json`: what it is built with and what it carries) as
the first round after the switch would:
no placed service's flag up yet, so the plan alone says where each
starts. It prints, per box, each service that runs there now and would
not (`STOPS`, and it exits 1), and each that would start. `-sticky`
plans as later rounds do, with what runs now counted as running;
`-fact box:svc/<service>/mem=<bytes>` tries a measure the box does not
report yet; `PLAN_LIVE=1` compares the plan the boxes serve now, as if
it were carried out whole. Build on node1 with
`NIX_FLAGS="--store ssh-ng://admin@100.64.0.2 --eval-store auto"`.

### Writers: leases

A service with one writer says `lease = true` in its needs. It is placed,
and runs only while its box holds the lease `svc/<name>`, which the
release gives to the boxes able to run it:

- Given the service by the plan, the box asks for the lease. Once the
  agreed state shows the lease as its own, it waits 3 seconds (a holder
  before it may still be stopping), then raises the flag and starts.
- It renews every 5 seconds (the lease lasts 15). The agreed state keeps
  which of the holder's requests last set the lease. A request can't be
  agreed before it was sent, so the lease can't end sooner than that
  request's send time plus 15 seconds. That send time, less 3 seconds, is
  *until*, kept by the box's clock since boot (suspend included), not its
  wall clock. The flag holds the epoch and *until*.
- It stops at *until* unless a renewal is seen agreed, fresh state or
  not. dd-control does that, and so does `dd-placed-fence`, a 2-second
  timer that needs nothing but the flag, so a dd-control that crashed or
  hangs does not leave a writer running.
- Moved by the plan, it stops its units, waits for them, and only then
  gives the lease back. Another box is refused the lease until it is
  given back or has lapsed, by which time the old holder has stopped.
- No box's wall clock matters. What has to hold is that the agreed clock
  runs at about the rate of the boxes' own, within the two 3-second
  margins over a lease. After a dd-control restart, the flag's *until*
  stands until a renewal the box sent, and has a record of, is agreed.
- A rule ships in the release that brings it. A box still on the release
  before skips that lease's requests, and agrees again once its switch
  restarts dd-control, which replays the log under the new rules.

### Postgres follows its lease

[`storage/postgres-follow.nix`](../../nix/modules/storage/postgres-follow.nix):
the box holding `svc/postgresql` archives every WAL segment, encrypted on
the box (wal-g, libsodium), into the store under its epoch
(`postgres-wal/main/e<epoch>`), at least every 30 seconds. The next holder
starts from there. The databases are small (about 100M on node1), so this
takes seconds, and no box keeps a live standby.

- **Taking the lease:** the newest base backup, then every segment after
  it from the epochs since, then promote. A box that held the newest
  epoch keeps its own data, which has everything it wrote, archived or
  not.
- **Handing it over:** on stop, the last segment is switched and archived
  before postgres goes, so a planned move loses nothing. An unplanned one
  loses at most what was not yet archived.
- **Fencing:** a segment is archived only while the flag holds the epoch
  this postgres started at. A holder whose lease moved on archives
  nothing more.
- **Nothing is deleted:** a data directory set aside for a restore stays
  beside it, named `<dir>.before-e<epoch>.<when>`.
- **The archive's key** is made once, by the first holder, only when the
  archive lists no keys at all. A box that can't list the store doesn't
  start. Each holder seals it to every box able to run postgres (their
  TPM-held secrets keys) beside `keys/fingerprint`, and seals only a key
  that matches the fingerprint. No secrets file holds it.
- **Until each box has its own store:** today the two-box cluster takes no
  writes while either box is away. A holder on the remaining box starts
  and runs, but its archive and base backup wait until the other box is
  back.

### Failover

The services with one writer, the data they keep, and the names that
point at them move between boxes by these leases: [failover](failover.md).

## Not in scope

The control plane doesn't run commands, push configuration, hold secrets or
move data. Build orders are not an exception: the order is a file the
release key signs, beside the release, and a box builds because the order
names it, as it moves to a release because the release names it. The log
carries only what each box says it built ([the app's
releases](app-release.md)); the bytes never pass through it. Placed
services are switched on and off, not configured; stateful replication
builds on it and comes after.

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
