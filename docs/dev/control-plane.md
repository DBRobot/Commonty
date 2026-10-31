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
stamp is not after the last one, more than 1 s ahead of its own clock, or
more than 4 s before it first saw the newest request the batch carries (one
it never saw counts as seen as it checks). Lease expiry uses this agreed
time, so no single clock can end a lease early.

- How late a batch arrives never counts. On 2026-10-24 node1 stamped a
  batch and sent it 2.9 s later, busy on disk (a VM test, a third of its
  time in I/O wait). The rule then was 2 s either way of the checker's own
  clock, so node2 refused it, and with the witness off the two never agreed
  again (below). SmartBFT checks a proposal once, as it arrives, and a
  delay between the stamp and that, at the proposer, the relay or the
  checker, now costs nothing as long as the requests reached the checker
  on time. Each box sends every request to all the validators, so they
  do; the checker keeps when it first saw each one for 2 to 4 minutes.
- Why leases stay safe. A renewal sets the lease to the batch's time plus
  the TTL, and its holder stops 3 s before its send time plus the TTL
  (below). A batch is never stamped more than 4 s before a request it
  carries was first seen, so before it was sent. Another box's grant needs
  a later batch stamped past the expiry, never more than 1 s ahead of a
  checker's clock, and it waits 3 s more before it starts. So the old
  holder has stopped before the new one starts with 1 s to spare: 4 s +
  1 s within the two 3 s margins. During the switch from 361, whose
  checkers allow 2 s either way, the sum is 6 s, still within them. An
  honest proposer stamps after it has the requests, so only a lying one
  could use the 4 s.
- A batch with nothing left to apply is held only to the order and the
  1 s. An old stamp there can only keep a lease or a box alive a little
  longer, never end one sooner.

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
| lease TTL | 45 s, renewed every 4 s | failover in about 45–50 s; a pause in deciding under 45 s (the witness switching at a publish took 13 s) drops no lease |
| ordinary heartbeat | 30 s | liveness for placement; cheap |
| a box counts as gone | 3 heartbeats missed (90 s) | stateless roles move |
| SmartBFT leader heartbeat / view change | 2 s / 4 s | Fabric's defaults (1 min / 20 s) are far too slow |

Free tier, as estimated: about 55k billable requests/day, under the 100k limit
(Durable Objects bill incoming WebSocket messages at 20 per request; outgoing
messages are free). Every box heartbeating fast would need Workers Paid,
about $5.50/month.

## When things fail

- **The relay is down.** Nothing new is agreed. Running roles keep running.
  Leases can't renew, so writers stop writing when theirs expire (45 s).
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
- **A pool holds only what is still to be decided.** A validator that
  catches up takes its peers' latest state and never delivers the blocks
  in between, so nothing took the requests those blocks decided out of its
  request pool. A leader drops such a request as already processed, so the
  validator complained about the leader, and with three validators one
  complaint changes the view for all. Each view change started its wait
  again: on 2026-10-24 the views changed for over a minute while a box
  switched under load (CI run 853).
  - A request its box's agreed sequence has reached is decided: no block
    can apply it (`state.ErrStale`). A validator drops one that arrives,
    and takes every decided one out of its pool when it catches up. A join
    is never taken for decided.
  - A validator of 359 does neither. When one passes a decided request on,
    the leader puts it in its next block (at most 64), where it changes
    nothing, and that validator takes it out of its pool once it is
    decided.
- **Two restarted validators can name different leaders.** At a restart
  SmartBFT takes its view from its write-ahead log, a view change it had
  started included. On 2026-10-24 node1, on release 359's unfixed v1.0.2,
  had sent the same view change for 11 hours that node2 never joined.
  When both switched to 360 together, each came back leading its own view
  and proposed sequence 103786 itself ("got pre-prepare from 2 but the
  leader is 1", "wrong digest at processPrepares"). With the witness off,
  a view change needs both boxes, and each refused the other's next view
  as one off. Restarting node1 and then node2 cleared it. Self-heal would
  have restarted one after 120 s of waiting requests. Before switching
  validators from 359, check that neither keeps logging "got viewChange
  ... expected view" with the same numbers, and restart that one first.
- **A witness in the config but off makes every third view change.**
  With n = 3, f is 0: a view change needs both boxes, and leader rotation
  hands every third view to the witness, which never sends a heartbeat. On
  2026-10-24 the views changed every 5 to 20 s all evening. After node2
  refused node1's late proposal (above, "Time") the two fell a view apart:
  each kept sending its vote for a view the other had already left ("got
  viewChange ... with view 2833, expected view 2834") and refused the
  other's new view ("expected sender to be 3"). SmartBFT moves a view at a
  time, and a vote for the view before is answered only now and then
  ("help the lagging nodes"), so nothing agreed for over a minute and every
  lease lapsed. Restarting one box at a time did not clear it: the one
  restarted came back at the view in its log, several behind. Stopping
  both and starting them together did, within about 30 s. While the witness
  is off, restart both validators together, never one after the other; the
  lasting fix is the witness on, or out of the config while it is off.
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

## Changing the state machine

A release reaches the validators one at a time: a box switches when its
agent gets to it, the witness when the relay is deployed. Each proposal
names the hash of the state it builds on, so validators that apply a block
differently, or write the state out differently, stop agreeing. That
happened on 2026-10-22: release 359's validators and main's computed
different states from the same blocks. Main's config had lease rules for
the placed writers (`svc/postgresql` and the rest) and 359's had none.
Each side refused the other's proposals, consensus stopped, and under
failover the writers stopped once their leases lapsed.

So the state machine has a version (`state.Machine`,
[state/machine.go](../../servers/system/control/state/machine.go)), and so
does the config it applies under:

- **Each validator says what it runs.** Every 5 s it sends the others a
  stand, signed with its validator key: its version, the digest of its
  config, when it first stood for that config, and its clock
  ([validator/stand.go](../../servers/system/control/validator/stand.go),
  frame kind 6, which a validator of 359 drops). Each box also reports its
  version in its heartbeat (the fact `machine`).
- **The validators decide; a new version and config take effect in one
  agreed block.** A proposer that holds a stand from every validator that
  is not off (below), each under 60 s old and all with its own config,
  puts the stands and the config in the proposal's header. Each validator
  checks them before it votes. From that block on, the agreed state holds
  `machine`, `config` and `configTime` (the block's agreed time), and
  every block applies under that config.
- **A validator that is off is left out, and only one that is.** A live
  validator says something in every decision (its prepare and commit) and
  every 5 s (its stand), whatever release it runs. So a validator counts
  as off when nothing at all has come from it for 20 s, since it was last
  heard or since the one judging started, while the one judging delivered
  10 decisions. Each voter judges that by what it heard itself: the
  proposer leaves out only a validator it finds off, and a voter refuses a
  header that leaves out one it has heard from. A validator is never off
  to itself, and the stands must still number a quorum. A validator of
  359 that is live therefore always holds activation back; the witness
  turned off (as since 2026-10-23) does not. One that was off and comes
  back on 359 after the activation can't follow the agreed state (below);
  it is one voter down, as it was while off.
- **No stale stand rolls a config back.** A validator votes for an
  activation only if it carries the config its own release ships, its own
  stand is no older than the latest it has kept, and every stand is newer
  than the block that agreed the config now in force.
- **A box's word counts only for a while.** A live box whose heartbeat
  reports an older version holds activation back for at most 30 minutes
  after the last validator stood for the config. After that it counts for
  nothing. A box the new config drops, or knows by another key, never
  counts. So a box that never reports, or reports low, can't veto a
  change.
- **Across the change, keys must hold under both configs.** In the block
  that activates, a request applies only if the configs before and after
  know its box by the same key, and joins wait for the next block. So a
  key one of the two configs drops never acts across the change.
- **Until then, nothing changes.** With no config agreed, a block applies
  under the config the release before ran, which the release ships as
  `before` ([fleet/control-before.json](../../fleet/control-before.json),
  release 359's config, exactly). The state's new fields stay empty and
  are left out of its JSON, so the hash matches the old code's. A validator
  of 359 accepts what a new one proposes, and the reverse, so either side
  can decide.
- **After that, a config change waits for every validator.** A later
  release that changes the config, a new lease rule or a new member, takes
  effect only once every validator stands for it. Until then each one
  applies the agreed config, whatever file its release ships.

What that costs:

- A validator that is off holds activation back for 20 s and 10
  decisions after the voters start or last heard from it. Two boxes
  switched together with the witness off agree the new version 25 to 30 s
  after the switch, and run the leased writers again at about 35 s
  (`checks.control-together`).
- A validator's "first stood" time is kept in memory. A validator that
  restarts starts the 30 minutes again, which delays a laggard's cutoff
  and nothing else.
- A reading box still on the older version when the grace runs out stops
  following the agreed state (below).
- A placed writer whose lease rule is new waits for the activation. On the
  move from 359, a box that has switched holds no `svc/*` lease until every
  live validator and every live box has switched too, and the switch
  itself stops the writers (their units gain the flag's condition). Two
  boxes switched together have them back within the lease's 45 s.
  - Nothing runs a writer without a lease meanwhile, by choice. A
    release-359 box may still run it by its roles, and only the
    activation says none does. Running it unleased would also run it
    without an epoch, so without the fence, the archive's fencing and the
    restore the follow modules do (they refuse epoch 0), on a box the plan
    picks, which may hold old data or none.
  - Nor are leases granted under the config before. 359's has no `svc/*`
    rule, and granting one at version 1 makes this release's state differ
    from 359's: the split of 2026-10-22.
- Activation is one way. After it, a validator or a reading box moved back
  to 359 can't follow the agreed state; it needs this release again. A
  reading box that stays on the older version stops at the last state it
  could check, and says nothing. Its writers stop once their leases could
  have lapsed.

### Adding a change to the state machine

Anything that changes what `Check`, `Apply` or `Block` decide, or what the
state's JSON holds, follows these rules:

1. **Raise `state.Machine`** and gate the new behaviour on
   `s.Version() >= N`. Below N, `Check` and `Apply` do exactly what the
   version before did, down to which requests are refused. A refused
   request takes no sequence number either.
2. **A new field in the state** is `omitempty`, and stays empty or zero
   until version N is agreed. An empty field adds nothing to the hash. Never
   rename a field, reorder fields or change a field's type. Sorting or
   normalizing an old field counts as a change too.
3. **Read the config through `s.In(cfg)`**, never the release's own
   `cfg`, in anything a block applies: `Block` already receives it. A
   change to the config's data alone (`fleet/*.json` feeding
   `control-config`) is safe from version 2 on, because the config is
   agreed. Keep `before` as it is until the fleet has agreed a config;
   after that it is read by nothing and can go.
4. **Test it from the release before.** `checks.control-upgrade`
   ([nix/tests/control-upgrade.nix](../../nix/tests/control-upgrade.nix))
   builds release 359's dd-control and witness from its commit, starts a
   fleet on them, and switches it a box at a time in two orders. It checks
   that every box holds the same state at each height, that `svc/db`
   keeps its holder and epoch, that decisions keep coming, and that
   version 2 and the new lease rule arrive only after the last switch.
   After a change to the state machine, point `old` at the release the
   fleet runs. The validators' own test
   (`TestVersionsSwitchOneAtATime`) does the same in one process.
   `checks.control-together` (the same script, `SCENARIO=together`) is
   release 360's switch: the witness in the config and off, both boxes
   switched at once from 359's programs and config. Decisions must come
   within 30 s, and postgresql and forgejo must run on their lease's box
   within 75 s (`TestSwitchTogetherWithTheWitnessOff` in one process).

## Trust

- Box keys are the TPM-held keys already in fleet/boxes.json (unlockKey).
  Validator keys are the same keys.
- dd-control signs in process
  ([tpmsign.go](../../servers/system/control/cmd/dd-control/tpmsign.go)):
  one connection to `/dev/tpmrm0`, the key loaded once, one signature at
  a time, a policy session per signature that is never saved. An error
  starts afresh and is tried again for up to 8 s; a TPM out of room for
  contexts first has its stale sessions flushed. Running tpm2-tools,
  five processes a signature, a stopped dd-control left a session saved
  for good, and 65535 contexts later node1's TPM signed nothing
  (2026-10-24 23:52). Every user of the TPM on a box (dd-control and the
  scripts still on tpm2-tools) takes `/run/dd-tpm/lock`, root's alone.
- `dd-control -check-sign -box <name>` tries a box's signing on its own:
  one signature through the same path, checked under the key the fleet
  has for the box, under the lock, leaving nothing in the TPM and touching
  no state, relay or lease. It prints `ok <alg> sha256:<SPKI hash>`.
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
  what a member's box may run by the policy's trust tiers (trust `any`, no
  code handing people their secrets, no secret from the box's own file, no
  name, no shared bucket, no lease), and its word only narrows that. A
  service that is its box's owner's or their household's alone (trust `own`
  or `household`, `joined.own`) is
  built and carried there, and the plan does not give it yet. Until its
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
  agreed at a time more than 4 s before it was sent ("Time", above), so
  the lease can't end sooner than that request's send time plus the TTL
  (45 s), less those 4 s. That send time, less 3 seconds, is
  *until*, kept by the box's clock since boot (suspend included), not its
  wall clock. The flag holds the epoch and *until*.
- It stops at *until* unless a renewal is seen agreed, fresh state or
  not. dd-control does that, and so does `dd-placed-fence`, a 2-second
  timer that needs nothing but the flag, so a dd-control that crashed or
  hangs does not leave a writer running.
- The plan keeps a writer with the box holding its lease while that box
  is alive and can run it, whatever another scores: its data is there.
  On 2026-10-24 node1 only failed to renew, and node2, with no base
  backup, took postgres.
- A lapsed lease is gone from the agreed state at the next block, so the
  plan cannot give a writer back to its last holder. That box does it
  itself: it keeps the epoch it last held beside the flags
  (`/run/dd-placed/.held-<service>`), and when that is still the agreed
  newest epoch and no one holds the lease, it asks for it again at once,
  given the writer by the plan or not, unless it is draining. Any other
  box waits 20 seconds before asking for a lease no one holds that
  someone held before. Holding it again, the plan keeps it there. A box
  that gave a lease back on a move forgets the epoch. On 2026-10-25 and
  26 every stall of agreement longer than the lease handed the group to
  whichever box scored best, and back at the next.
- A box that took a writer over and cannot start it refuses it
  (`refused/<service>`, for 10 minutes), and the plan gives it to a box
  that can. Only the writer's own unit is judged (the first of what
  starts it), and only failed, or stopped with no job queued (what it
  needs failed), is a failure: coming up, restarting, or waiting on a
  restore however long is not, and once it is up nothing later counts.
  A helper beside it failing (a handover's archive, museum's health
  check) counted before, and refused the holder's own writer 5 s after
  it started. One taking back its own lapsed lease never refuses it. A
  refusal is kept beside the flags (`.refused-<service>`), so a
  dd-control that starts again keeps it, and a boot forgets it.
- A writer every live box able to run it refused, none of them draining,
  is placed all the same ("every box able to run it refused it; tried
  again"), and the box given it tries again. Before, two refusals at once
  left it nowhere until a dd-control restart forgot them (2026-10-26).
- What runs on a handover (`dd.store.handover`: `dd-pg-handover`,
  `dd-forge-handover`, `dd-headscale-handover`) is never started with
  its service.
- Moved by the plan, it stops its units, waits for them, and only then
  gives the lease back. Another box is refused the lease until it is
  given back or has lapsed, by which time the old holder has stopped.
- No box's wall clock matters. What has to hold is that the agreed clock
  runs at about the rate of the boxes' own, within the two 3-second
  margins over a lease ("Time", above, says why the validators' bounds fit
  inside them). After a dd-control restart, the flag's *until*
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
