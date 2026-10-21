# Staged rollout

Status: design (2026-10-21), branch rollout-1021, on top of failover
being on (failover-on-1020). Nothing here is built yet.

The goal: a release that fails keeps everything up. Today every box takes a
release within minutes of publish, and a box whose switch fails spends up
to `probeSeconds` (10 min) on a broken system before it rolls back, with
its services down the whole time. With staged rollout:

1. A few boxes (**wave 1**) take the release first. Every other box
   **holds** it until the control plane marks it good.
2. A box whose turn comes **drains** first: its single-writer services are
   handed to a healthy box by failover's planned handover, then it
   switches with nothing to lose. A failed switch rolls back while the
   other box serves.
3. Before wave 1, a small **staging** run on node1 tries what must never
   be tried first on a real box: a planned handover and a join with a
   simulated TPM, from the running release to the new one.

```
publish N ─► staging on node1 (VMs) ─► signed, pushed
   │
   every agent fetches N, submits "begin N" (the release file inside)
   │
   validators: begin checked against the release key, waves fixed
   │
   wave 1 (node2 with two boxes)            everyone else
     drain ─► switch ─► probe                 held: "waits for wave 1"
     soak 30 min: probe, units, restarts,
       metrics, site checks, every minute
     verdict good ──────────────────────►   wave 2 admitted
     verdict bad  ─► rollout stops,           drain ─► switch ─► probe ─► good
       wave 1 rolls back, owner mailed        ...
```

## State: the rollout record

The rollout lives in the control plane's agreed log, as one new request
kind, `rollout`, signed by the box's TPM key like every other request.
Nothing else in the log changes.

```
rollout begin    {counter, release}   release = current.json as published
rollout verdict  {counter, verdict, reason}
                 verdict: took | good | bad
```

The agreed state keeps one record, the newest rollout:

```
Rollout {
  counter     the release's counter
  digest      sha256 of the release file: what every box must be fetching
  began       agreed time of the begin
  waves       [[box…], [box…], …]   fixed at begin
  admitted    how many waves may take it (1 at begin)
  since       agreed time the newest wave was admitted
  reports     box -> {verdict, at (agreed), reason}
  status      soaking | good | done | bad
  why         for bad: which box, which check, or "not proven by <time>"
  paused      agreed ms not counted (gaps in agreement, below)
}
```

`good` is the mark: wave 1 has passed and the rest may follow. `done`:
every wave has. A box's agent reads the record from its own dd-control
(`127.0.0.1:4195/rollout`), which serves only state signed by a quorum of
validators, the same as `/state` and `/plan`.

## Signers

- **The release key** signs the release, as today. A `begin` carries the
  whole signed file and the validators check it against the release key
  (the one already in the control config for the identity judge). Only a
  real release can start a rollout, and only one with a higher counter
  than the record has.
- **Each box's TPM key** signs its own `begin` and its verdicts. A box may
  give a verdict only on the release of the current record, only while its
  wave is admitted, and only about itself.
- **The validators** (node1, node2 and the Cloudflare witness today; a
  quorum of 2) agree on the record. No box, the laptop included, can mark
  a release good on its own. The release key cannot either: it can start a
  rollout, never pass one.

## Who is in which wave

Fixed by the state machine when it accepts `begin`, from the agreed state
at that moment, so every validator gets the same answer and it does not
shift while boxes drain and switch. Never a hand-set role.

- The boxes are the release's that are alive (heard within `goneAfter`).
  A box not alive then is in the last wave.
- Sizes: wave 1 is 5% of them, at least one; wave 2 takes it to 25%; wave
  3 is the rest. With two or more boxes, at least one is always held back
  from wave 1. Empty waves are dropped.
- Order, for wave 1 first:
  1. boxes that hold no writer lease (so wave 1 needs no handover with
     nowhere proven to go);
  2. then, one at a time, the box that adds the most not yet covered:
     a house, an architecture, a hardware class (memory and cores, coarse),
     and the services it runs (`runs/<svc>` in its heartbeat). Wave 1 is
     as unlike itself as the fleet allows.
  3. ties by sha256(counter, name), so a different box goes first next
     time.
- **With two boxes:** node2 holds no writer, so wave 1 is node2 and wave
  2 is node1. If failover ever moves the writers to node2, wave 1 becomes
  node1 by the same rule.

## The judgment

A wave box judges itself, as each box already judges its own switch: the
verdict is its word, signed by its TPM. A box that lies can only pass a
release that the release key signed and two boxes vouched for (no access),
or stop a rollout (availability).

Every minute while it soaks (`dd-agent --soak`), against what was true
before its switch:

- **switched clean**: the switch and probe passed with no rollback, and the
  guard never fired;
- **health**: the switch probe again (no newly failed unit, sshd, the gate
  on 4181, reach to the fleet), and every placed service whose flag is up
  has its units active;
- **restarts**: no service restarted more than 3 times in the window
  (`NRestarts`): a crash loop never shows as failed;
- **metrics**: no scrape target of the box's own Prometheus that was up
  before is down, and no fleet-alert condition (disk, pool, backup) that
  was clear before;
- **site checks**: the site-links and site-ci checks (branches
  site-links-1021, site-ci-1021) when they land. Until then, plain
  probes: for every name this box serves (each placed service's `hosts`,
  and the front door if it holds the network's lease), `https://<name>`
  resolved to this box, any answer below 500.

A check fails when it fails on 3 samples in a row (a slow start is not a
fault); a rollback, the guard firing or a unit failing are at once. Any
failure: the box sends `bad` with the reason and rolls itself back without
waiting. The window clean for `soak` (30 min, agreed time between its
`took` and now): it sends `good`.

The state machine, at every batch:

```
any report bad, in the newest admitted wave          ─► bad
every box of wave k sent good, at ≥ took + soak(k)   ─► admit wave k+1
                                                         (wave 1 passing: good)
every wave passed                                     ─► done
a box of the admitted wave has no good by
  since + took deadline (40 min) + soak(k) + 30 min  ─► bad: "not proven"
```

`soak` per wave is in the release's control config (`rollout.soakMs`:
30 min, 10 min, 0); a 0 soak means a wave passes as soon as each box's
switch probe does.

**When the record is bad:** each box of an admitted wave whose wave has not
passed rolls back to the system it ran before (`previous`), and remembers
the release as refused. A box of a passed wave stays: it was proven. Held
boxes never move. The owner is mailed (below). The fix is a new release.

## The agent

The release file says what to run; the record says when. The agent's
loop (5 min timer, nudged at publish; 1 min while soaking):

```
fetch, verify, counter, entry, refused?, vouched?      as today
no record for N yet, or an older one   ─► POST begin N; held this run
record N, my wave admitted?           no ─► held ("release N waits for
                                             wave 1: node2, soaking 12 of
                                             30 min"), result held
record N bad                          ─► held; if I run N unproven: roll back
my wave admitted, not running N       ─► drain ─► switch ─► probe (as today)
                                         ─► POST took ─► soaking
soaking                               ─► the checks above ─► POST good|bad
```

- **Held** is a result like `ok` or `rollback` (`dd_agent_info{result="held"}`),
  on the Network page and in `dd release status` ("held for wave 1").
- **Soaking**: `/var/lib/dd-agent/soaking` holds `<counter> <path>` and the
  baseline taken before the switch (failed units, restarts, scrape targets
  up, alert conditions). The counter is written only when the soak passes,
  so `release` in the heartbeat names only proven releases. A reboot while
  soaking resumes the soak (the clean time restarts).
- **Bootstrap:** the release that first carries all this is taken the old
  way: a box takes a release at once when the record has no rollout
  support yet, which it knows from `/rollout` answering 404. From the next
  release on, every box waits for the record. A box with no control plane
  at all (the home controller) takes releases as today.
- **One box alone**: there is nobody to hold back; it takes the release,
  drains nothing, soaks, and that is the rollout.

## Drain before switch

When a box's wave comes and it runs services, its agent writes
`/var/lib/dd-facts/release-drain.prom` (`dd_release_drain <counter>`).
dd-control reads it with its other facts (as `draining` is read) and the
heartbeat carries it. Placement then:

- gives the box nothing new;
- moves each **group** it runs (a service and what must share its box:
  postgres with museum, forgejo, the vault and headscale) to a box that can
  hold the whole group, preferring boxes already on release N and proven
  (`release` ≥ N), then held ones (wave 1 has nowhere proven to go);
- leaves a group that fits nowhere else where it is, with "restarts in
  place: no other box holds it" in the plan's why.

The moves are failover's planned handover, unchanged: the old box stops the
writer, makes the final copy while it still holds the lease, gives the
lease back; the new box takes it, restores, starts; the names follow the
lease. The agent waits until the plan gives it only what restarts in place
and each moved writer's lease is held elsewhere, up to `drainSeconds`
(10 min). Then it switches. On timeout it switches anyway: what is still
on it restarts in place, as today without drain, and the result says so.

After the switch, ok or rolled back, the fact goes and the box is an
ordinary box again. Placement may move a service back only by its normal
rules (a better box by the damping margin). It does not move back on its
own.

**Restart in place, by design** (a single copy, or nowhere to go):

```
llama-swap      the chat model: loaded in one box's memory, one copy
dd-games        game servers: their worlds are on that box, moved only
                on failure
computers       members' remote computers: moved only on failure
transcode       jobs on that box's GPU; a job in flight is redone
not placed      everyBox and role-only services (nginx, the gate, the
                store, garage, pihole, …): every box has its own
```

Anything else stays only when the plan finds no room for its group, and
the plan says so.

**Photos and node2's old pool.** Until node2's half of the old shared
garage pool is retired (David's yes), the photos bucket does not fit in
node2's store. Museum must share a box with postgres, and so must forgejo,
the vault and headscale: the whole group stays on node1 and restarts in
place when node1 switches. The stateless placed services still move. Once
the pool is retired the group fits on node2, and node1's switch hands the
whole group over with no change here. Drain never splits a group: it
checks the group's room as one, so it cannot leave museum with nowhere to
run.

## Staging on node1

Before publish signs, `dd release publish` runs one VM test on node1's
store (`--store ssh-ng://admin@100.64.0.2`): `checks.rollout-stage`, with
the running release's rev and the new one.

```
stage-a   a generic box, failover on, swtpm   starts on the running
                                               release, holds the writers
stage-b   a generic box, swtpm                joins with its simulated
                                               TPM (enlistment, join ticket)
witness   relay and validator, the cache     one small VM
```

It checks: b joins and is given carries; a drains to b by the planned
handover (postgres and a page probed the whole time); a switches to the
new release; the group stays on b. Small on purpose: the services are a
stand-in group (postgres, a page on a lease), not the fleet's.

Cost on node1: about 4.5 GiB of RAM for 10–15 minutes (two boxes at 2
GiB, the witness at 512 MiB), and the builds of both releases' generic box,
which attest has mostly built already. A failure stops publish before
anything is signed, like a refused attestation. It runs the old rev's
generic box only, not node1's or node2's full systems.

## Timeouts

```
soak            wave 1 30 min, wave 2 10 min, wave 3 0     control config
took deadline   40 min after a wave is admitted            control config
proof deadline  took deadline + soak + 30 min              control config
drain           10 min, then switch anyway                 agent
check samples   every minute, 3 in a row to fail           agent
probe, guard    as today (10 min; 2×probe+5 min)           agent
```

## When things fail

- **The control plane is down** (relay or quorum). No record changes. Held
  boxes stay held. A soaking box keeps soaking on its own checks and still
  rolls back on its own if one fails; its verdict waits. Agreed time does
  not count a gap: when blocks resume after more than `goneAfter`, the gap
  is added to `paused`, so every deadline moves by it and no box is judged
  "not proven" for the outage. A soak spanning the gap starts its 30 min
  again from the first block after it, so the soak is always 30 minutes of
  watched, agreed time. A drain waiting on the plan times out and switches
  in place (the plan needs fresh agreement to move anything).
- **A wave 1 box goes offline.** Its verdict never comes. The rollout waits
  until the proof deadline, then stops: `bad`, "node2: not proven by
  <time>". The owner is mailed. A box that comes back on N unproven rolls
  back. The other boxes never moved.
- **A split vote**: one wave box says good, another bad. Bad wins: one bad
  stops the wave and the whole wave rolls back. Validators disagreeing is
  not a vote here: without a quorum nothing is agreed, which is the "down"
  case above.
- **A new release during a rollout.** A higher `begin` replaces the
  record. Boxes soaking the old one finish their own soak (and roll back
  on their own if it fails) but are judged no further; the new record's
  waves decide from there.
- **A release that changes the control plane's own rules.** During its
  soak the voters run two releases' rules. With three voters, wave 1's box
  and the witness (deployed at publish) agree on the new rules and keep
  agreement; the held voter drops out until it switches. If wave 1 fails,
  the two boxes on the old rules have a quorum again. Rule changes should
  ship in a release of their own.
- **The agent dies mid-drain.** The drain fact stays, and the box keeps
  being drained; the next run (5 min) sees its wave admitted and carries on.
  A box whose release passed or rolled back removes a stale fact on its next
  run.
- **The owner is mailed** by `dd-fleet-alerts` on the box that mails: every
  agent writes `dd_rollout_status{counter,status}`, and a `bad` is mailed
  once with its why.

## Two boxes, today

```
publish N
  node2: wave 1, holds no writer; drains nothing; switches; soaks 30 min
  node1: held, serving everything
  node2 good ─► node1's turn
  node1: drains; the writer group does not fit node2 (photos), so it
         restarts in place; stateless placed services move to node2;
         switches; probe; done
```

So today, two boxes give: a bad release is caught on node2, which serves
nothing members use, and never reaches node1. A release that is good on
node2 but fails on node1 alone still costs node1's group its restart and
a rollback; that goes away when the old pool is retired and the group fits
on node2.

## Decisions for David

1. **Where the mark lives**: a new `rollout` request in the agreed log
   (above), or plain heartbeat facts that every box judges the same way
   (no state machine change, but no agreed history: deadlines on each
   box's own clock). Proposed: the log.
2. **The writer handover is not instant.** A planned handover leaves a
   writer down for its final copy and restore, seconds for the vault and
   headscale, up to about 30 s for postgres with forgejo. No box serves
   the names meanwhile. Options:
   a. the old box's front door answers its moved names with `503
      Retry-After: 5` and a "one moment" page until the names have moved,
      and the app and our pages retry on it. No new channel. **Proposed.**
   b. the old box forwards the moved names to the new holder over the
      fleet network until the names move: no member sees anything, but it
      is a box-to-box channel carrying members' traffic, against the
      isolation direction.
   c. accept the gap: it is still far shorter than a failed switch.
   The VM test's probe counts a 503 with Retry-After answered within 60 s
   as held, not failed, under (a).
3. **Drain timeout**: switch anyway with what is left restarting in place
   (proposed), or give up the turn and mail.
4. **Wave 1 rule**: writer-free boxes first, then diversity (proposed),
   or diversity alone (wave 1 then needs a handover to a held box).
5. **Staging inside publish** (proposed) or as a separate step.
