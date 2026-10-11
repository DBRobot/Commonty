# Releasing the boxes

How a merged change becomes the system every box runs. A release is one
signed file naming, for every box, the NixOS system it should run. The
laptop signs it; each box fetches it, checks it, and moves itself. Nothing
pushes to a box, and nothing but the release key can move one.

This page is the boxes' own systems. The app a stranger downloads has
[its own page](app-release.md); the pull request, CI and the release lock
are in [CI](ci.md).

## From merge to every box

```
pull request, inside the release lock (ci.md)
  CI: build_host builds every box's system on every builder box
      ─► status build/<box>@<builder> = the toplevel's store path
      same_build: all builders recorded one path ─► status build/<box>
        (one path is one evaluation; the bytes are dd-attest's, below)
  merge to main
│
dd release publish            the laptop, with the dd built from what merged
  local main == forge/main?                                 no ─► stop
  the paths: build/<box> on main, main^2 or main^1 (same tree only)
    each == this machine's `nix eval` of that box?          no ─► stop
    each, and its whole closure, in the cache bucket?       no ─► build every
                                                                  box instead
  every box runs dd-attest over ssh; 2 must agree per box   no ─► stop
  counter = the published counter + 1 (signature checked first)
  sign with the release key (v2, or legacy for one release),
    the boxes' signed answers inside it
  built here? copy the closures to the cache bucket
  releases branch: current.json + history/<counter>.json, pushed
  nudge: ssh each box, `systemctl start --no-block dd-agent`
  the DNS Worker (before the nudge), the control relay Worker, the mail
  Worker, the forge page r<counter>
  dd release app-next in the background (app-release.md)
│
each box: dd-agent, nudged or on its timer (every 5 min)
  fetch current.json ─► verify ─► counter ≥ mine? ─► my entry? ─► tried before?
  two boxes of my fleet vouch for my closure?               no ─► refused, nothing changes
  nix copy from the cache ─► nar hash ─► closure digest
  arm the guard ─► set the profile ─► switch
  healthy within probeSeconds, and still reaching the fleet?
    yes ─► counter written, result "ok"
    no  ─► back to the previous system, the release remembered as tried
│
dd release status: every box "current"   ─► release the lock
```

The code:

- [client/cli/src/release_cmd.rs](../../client/cli/src/release_cmd.rs):
  `dd release` on the laptop (`init`, `publish`, `sign`, `show`, `status`,
  and the app's commands).
- [servers/system/release/src/lib.rs](../../servers/system/release/src/lib.rs):
  the file format, signing and checking, the closure digest.
- [servers/system/release/src/bin/dd-agent.rs](../../servers/system/release/src/bin/dd-agent.rs):
  what each box runs to move itself.
- [servers/system/release/src/bin/dd-attest.rs](../../servers/system/release/src/bin/dd-attest.rs):
  a box's word on a release before it is signed, and its builds for build
  orders.
- [nix/modules/box/agent.nix](../../nix/modules/box/agent.nix): the units
  and timers; [nix/roles/core.nix](../../nix/roles/core.nix) turns them on
  for every box, and `mkBox` in [flake.nix](../../flake.nix) sets each box's
  cache and reach.

## The release file

`current.json` on the releases branch, served raw by the forge at
`https://git.<domain>/<repo>/raw/branch/releases/current.json`:

```
{
  "payload": {
    "counter": 223,                    strictly increasing; a box never goes back
    "rev": "512d508c95e9…",            the commit every path was built from
    "issued": 1791666517,              seconds since the epoch, when signed
    "boxes": {                         box name -> what it runs, sorted by name
      "node1": {
        "path": "/nix/store/…-nixos-system-node1-…",
        "nar_hash": "sha256-…",        the toplevel's nar hash, as nix path-info prints it
        "closure": "79f62b…"           sha256 over the whole closure (below)
      },
      "node2": { … }
    },
    "attestations": [                  each box's answer from dd-attest, as signed
      {
        "attester": "node1",
        "statement": "{\"v\":1,\"attester\":\"node1\",\"flake\":…,\"rev\":…,\"boxes\":{…}}",
        "signature": "-----BEGIN SSH SIGNATURE-----…"
      },
      { "attester": "node2", … }
    ]
  },
  "signer": "fdpY…=",                  the release key's public half, base64
  "signature": "WeQx…=="               ed25519 over the payload, base64
}
```

**Why the closure digest.** A NixOS toplevel is a tree of symlinks. Its
nar hash pins the *names* of what it points at, not their contents, and
the contents come from a cache bucket more than one machine can write. So
`closure` covers everything: every path `nix path-info --recursive --json`
lists, as `<path> <narHash>` lines, sorted, sha256, hex
(`closure_digest`). It is optional in the format: releases before it
(history/1 to 80) have none, and a box checks it when it is there.

**Why the attestations.** Publish refuses to sign unless two boxes rebuilt
every closure (below), but a box that only checked the release key's
signature would take whatever that key signed: the key alone could move
the fleet. So the boxes' answers travel inside the signed payload, exactly
as dd-attest printed them, and each box checks them again before it
switches ([What the agent does](#what-the-agent-does), step 7). Absent on
releases before they were carried. `"bootstrap": true` instead says, under
the release key, that no box was asked (`DD_ATTEST=off`); see [The
bridges](#the-bridges).

**What is signed.** Two forms; a box accepts either (`verify_json`):

```
v2      the payload exactly as published, every field known or not, keys
        sorted at every level, no whitespace (canonical_v2). The default.
legacy  the payload as this build's struct writes it out: counter, rev,
        issued, boxes in that order, no closure (canonical).
```

The first agent checked the legacy form: it read the file into its own
struct, wrote it back out, and checked the signature over that. Any field
it had never heard of was dropped, so the first release to add one
(`closure`) would have been refused by every box, and so would the release
carrying the agent that could read it. The fleet would have stopped
taking releases for good. v2 signs the bytes as published, so a newer
publisher can add fields and an older agent still verifies them.
`DD_RELEASE_FORM=legacy` existed to cross that once (below). The keys are
sorted by hand rather than left to serde_json, whose map order is a
feature flag any crate in the build can turn on.

**The signer must be the release key**, exactly: a valid signature by any
other key is refused (`Error::Signer`).

The release key also signs other documents in the v2 form, each naming its
own `kind` (`sign_doc`, `verify_doc`): `app.json` (kind `app`) and the
build orders (kind `build-order`). A box release has no `kind` and a
different shape, so neither can be passed off as the other.

## The releases branch

An orphan branch on the forge, never merged anywhere: the history of what
the release key has signed. `to_releases` adds files in a throwaway
worktree, commits, and pushes.

```
current.json            the release the boxes follow
history/<counter>.json  every release ever published, the same bytes
app.json                the app's signed manifest (app-release.md)
app-history/<tag>.json  every app manifest
build-order.json        the order the boxes are building for an app release
build-check.json        the nightly check's order
```

The boxes read `current.json`; `dd-build` reads the two orders from beside
it. The cache's garbage collector (`dd-cache-gc`, on the box with the
forge, [nix/modules/storage/cache.nix](../../nix/modules/storage/cache.nix))
reads `history/` to keep the closures of the last ten releases, and
anything a week old or newer.

## The keys

```
release key      ed25519. The private half in the laptop's OS keyring
                 (service "commonty", account "release-key"); DD_KEYRING_FILE
                 points dd at a json file instead, for tests. The public half
                 is fleet/release.pub, built into every box as
                 /etc/dd/release.pub, into the gate (dd.verify.releasePublicKey)
                 and into the control relay Worker.
cache bucket     s3://nix-cache in garage. publish reaches it at
                 100.64.0.2:3900 (node1, over the fleet's network); a box
                 through its own garage, or a storage box's.
cache keys       fleet/cache-keys.pub: the laptop's, node1's, node2's. A box's
                 nix trusts only dd-cache-laptop-*; the boxes upload what they
                 build signed with their own key, which nobody has to believe.
cache writer     secrets/fleet.yaml: cache.keyId, cache.keySecret,
                 cache.signingKey, readable by the laptop's sops key alone.
host keys        each box's ssh host key, in fleet/boxes.json (hostKey): what
                 dd-attest signs with, namespace commonty-attest-v1.
```

The agent does not lean on the cache's signatures at all: it copies with
`--no-check-sigs` and holds what arrives to the nar hash and closure digest
the release key signed. A poisoned upload can mislead CI on the box that
reads it, never a release.

`dd release init` makes the release key, once, and writes the public half
to `fleet/release.pub`; it refuses if a key is already there. Losing the
key means no box takes a release again until each is switched by hand to
a system built with a new public half.

## What publish checks before signing

`dd release publish [--ref main] [--cache …] [--url …] [--dry-run]`.

1. **The ref is the forge's.** The local branch must equal
   `forge/<ref>`; push it first.
2. **CI's paths, checked by evaluation.** It reads the `build/<box>`
   statuses from the forge's API (every page) on the commit, or on its
   second or first parent when that has the same tree: CI ran on the pull
   request's head, and the merge commit has the same tree. A status is
   written by whoever holds a job token, a runner credential or the forge,
   so it is never taken on its word: each path must equal what this
   machine's `nix eval` of that box's toplevel gives, from the same ref.
   A mismatch stops everything ("Whoever posted that status does not agree
   with the commit"). Evaluation takes seconds and writes no store; it
   settles "is this the closure this commit describes", not "are these the
   bytes it makes". That is the next step's job.
3. **In the cache.** Each path, and the path-info of its whole closure, must
   be readable in the cache bucket; its nar hash and closure digest are
   taken from there. If any box's is missing, publish forgets CI's paths
   and builds every box from git (never from the working tree): on the
   store `DD_BUILD_STORE` names (`ssh-ng://admin@…`, so a box builds and the
   laptop keeps no store), or here when it is unset.
4. **Two boxes vouch.** Every box in `fleet/boxes.json` is asked at once,
   over ssh, to run `sudo dd-attest --flake git+https://<forge>/<repo> --rev
   <rev> --box <name>=<path> …` for every box of the release. Each answer
   must be signed by that box's host key in `boxes.json`, in the attest
   namespace, about this commit of this repository. For every box of the
   release, at least two attesters must report the same path and the same
   closure digest, or nothing is signed. CI, the forge and the cache can
   each lie about a path; they cannot make two boxes rebuild the lie. A
   fleet of one box cannot pass this.
5. **The counter.** The published `current.json` is fetched and its
   signature checked against `fleet/release.pub` before its counter is
   believed; the new one is that plus one. The forge serves the file and
   does not sign it: an unchecked `u64::MAX` would leave every box refusing
   everything after it, for good. If the file cannot be fetched but the
   releases branch exists, publish stops rather than start again at 1.

Then it signs, copies the closures to the cache (only if it built them
itself; CI's are already there, signed with the laptop's cache key from
`secrets/fleet.yaml`), pushes `current.json` and `history/<n>.json`, and
prints `release <n>  commit <rev>`. That line is how a ship script learns
its number.

After the push, and none of them can fail the release (each prints how to
do it by hand):

- **nudge**: every box is told over ssh to start `dd-agent` now. It changes
  when a box looks, not what it accepts. A box not reached looks on its
  timer.
- **the id Worker** (sign-in, `dd id deploy --ref <ref>`) and **the DNS
  Worker** this release carries are deployed before the nudge,
  so a box given a new name finds it there when it asks
  (`dd dns deploy --ref <ref>`; [gate.md](gate.md#the-zone-names-and-certificates)).
- **the control relay Worker** and **the mail Worker** this release
  carries are deployed (`dd control deploy --ref <ref>`,
  `dd mail deploy --ref <ref>`).
- **the forge's release page**, tagged `r<counter>` at the commit, with the
  merged pull requests' titles grouped by kind.
- **the app**: `dd release app-next` starts in a session of its own, with
  every inherited file descriptor closed at exec, so it never holds the
  release lock (release 196's did). Log: `~/.cache/dd/app-release.log`.

`--dry-run` checks everything above except the vouching (no box is asked;
it says "not vouched (dry run)"), and prints the signed release instead of
pushing it.

## dd-attest: accounting for every path

With `--box` (publish's path), for each box named:

1. The flake must be `git+https://host/owner/name` and the rev 40 hex.
2. Evaluate `nixosConfigurations.<box>.config.system.build.toplevel` at that
   rev, here. Its out path must be the one asked about.
3. `nix-store --realise` the derivation: substituted only where a key the
   box trusts signed it, otherwise built by this box's daemon.
4. Walk `nix path-info --recursive`. Every path must be one of:
   - signed by cache.nixos.org, checked against that key alone
     (`nix store verify --sigs-needed 1` with only upstream's key trusted),
     not against the signatures a narinfo claims;
   - content-addressed, so the path is its own proof;
   - built on this box by its own daemon (`ultimate`);
   - already rebuilt here and found the same (`/var/lib/dd-attest/checked`,
     by path and nar hash);
   - or rebuilt now: its deriver must be among this box's own evaluation's
     derivations (a deriver recorded with a path came from wherever the
     path came from), and must make it; then `nix-store --realise --check
     --keep-going` over all of them, so every mismatch is named at once.
5. Answer with the closure digest, signed with the ssh host key:
   `{"v":1,"attester","flake","rev","boxes":{<name>:{path,closure}}}`.

One attestation runs at a time (`/var/lib/dd-attest/lock`). CI builds every
box's system on every builder box first (`build_host`), so at publish time
the paths are this box's own builds and the check takes seconds.

With `--order` (the `dd-build` timer), the same rules over the jobs of a
signed build order; the result goes to the agreed log through dd-control
instead of back over ssh. That is the app's path: [Releasing the
app](app-release.md).

## What the agent does

`dd-agent.service` is a oneshot. Its timer fires 2 minutes after boot and
then every `interval` (5 min) after its last start, with up to 30 s of
jitter; `dd release publish` also starts it. A switch never restarts it
(`restartIfChanged = false`, `stopIfChanged = false`): it is the thing
doing the switching. Its state is in `/var/lib/dd-agent`:

```
counter        the last release this box applied
applied        "<counter> <path>": what that counter was for this box
previous       the system it ran before the last switch
pending        "<counter> <path>": switched to, not yet proved
failed-before  the units failed before that switch, one a line
refused        "<counter> <path>" lines: tried here and rolled back
attested       the first release this box took on two boxes' word; from
               then on it takes nothing else
```

Step by step:

1. **Fetch** `current.json`. A 404 is "no release yet", not an error.
2. **Verify** against `/etc/dd/release.pub`, over the bytes as published.
   Refused: the unit fails with `release refused: …`. The metrics keep the
   counter this box applied, with result `refused`.
3. **The counter.** Lower than the one in `counter`: refused, the unit
   fails. Equal or higher goes on.
4. **Its entry.** No entry for this box's hostname: nothing to do
   (result `absent`).
5. **Tried before?** A `counter path` pair in `refused` is not tried again;
   each go costs the probe window twice in a system that does not work. A
   republished fix has another counter and path, and comes in.
   **The same counter, another path?** Equal to `counter` but not what
   `applied` says it was: refused, the unit fails. One number is one
   system; the same release fetched or published again comes in.
6. **Already running it?** If `/run/current-system` is the path: if
   `pending` names this release, the box rebooted inside the probe window,
   came back on the new system never checked, and owes the probe now,
   against `failed-before` (what was failing before the switch, not what
   is failing after the reboot); failing it rolls back to `previous`.
   Otherwise the counter is written and the run ends `ok`.
7. **Two boxes vouch.** The boxes whose word counts are the ones this
   box's own system was built with: `/etc/dd/attesters.json`, each box in
   `fleet/boxes.json` and its `hostKey` (`dd.agent.attesters`, set in
   core.nix). A release cannot add an attester for itself. An attestation
   counts when its attester is on that list, its statement is about this
   release's `rev`, names this box with exactly its `path` and `closure`,
   and `ssh-keygen -Y verify` finds it signed by that box's host key in
   the attest namespace (`release::vouchers`, `release::ssh_signed`). At
   least two distinct boxes must vouch; the box itself may be one of them,
   never the only one. Each attestation that does not count is logged
   (`attestation: node3: not one of this fleet's boxes`). Then
   (`judge` in dd-agent.rs):

   ```
   fewer than 2 boxes on the list      taken: a fleet that cannot vouch
   2 or more vouch                     taken; `attested` written the first time
   fewer, and `attested` exists        refused: required since that release
   fewer, and the release carries      refused: what it carries does not hold
     attestations
   none, "bootstrap", no `attested`    taken: bootstrap
   none, no `attested`                 taken: a release from before them
   ```

   A refusal comes before anything is fetched or changed: the box stays on
   the generation it runs, the result is `refused`, and the unit fails with
   `release N: not switching to <path>: 1 of the 2 boxes needed vouch for
   it (node1)`. It is not added to `refused`, so a box that gets its
   `attested` file removed by hand would take it on the next run.
   The running release, when it is already this box's, is never refused
   here, but if two boxes vouch for it `attested` is written: that is how a
   box whose release came before the agent that checks starts requiring.
8. **Fetch and check.** `nix copy --from <cache> --no-check-sigs <path>`,
   then the toplevel's nar hash must equal `nar_hash`, and the digest of
   the recursive path-info must equal `closure`. A mismatch refuses the
   release before anything changes.
9. **Get ready to go back.** Write `previous`, the units already failed
   (`failed-before`), then `pending`. Note whether any other box is
   reachable.
10. **Arm the guard**: a transient `dd-agent-guard` timer
   (`systemd-run --on-active=2×probeSeconds+300`) that sets the profile
   back to the *current* system and runs its `switch-to-configuration
   switch`, as the agent's own rollback does. If the agent dies mid-way, or
   the switch takes it down, the guard brings the old system back, and the
   next boot comes up on it too.
11. **Switch.** Set `/nix/var/nix/profiles/system` to the new path, run its
    `switch-to-configuration switch`.
12. **Is it a box?** Healthy means all of:
    - the switch exited 0;
    - within `probeSeconds` (600), a probe passes, tried every 10 s: no
      *newly* failed unit (units failed before the switch, and `dd-game@*`,
      do not count), `sshd` active, the verifier answering on
      127.0.0.1:4181;
    - if the box reached the fleet before, it still does: two answers in a
      row, 5 s apart, within `probeSeconds`.
13. **Healthy:** stop the guard's timer, then look again at what runs. If
    the guard went off while the probes ran (its service started, or
    `/run/current-system` is no longer the release), the pass was measured
    across a switch back: it is a rollback, "the guard switched back before
    it was proved". Otherwise disarm, remove `pending`, write `counter` and
    `applied`, result `ok`.
14. **Not:** append to `refused` first (so a crash before the undo does not
    meet the same release in five minutes), let a guard already switching
    back finish, set the profile back, switch back, disarm, remove
    `pending`, result `rollback`. The unit fails with
    `release <n>: <path> did not come up healthy; back on <previous>`, and
    `(it could no longer reach the fleet)` or `(the guard switched back
    before it was proved)` when that was why.

**Reach** (`dd.agent.reach`): for each other box, `<net>:4181` (the gate)
and `<net>:443` (the front door). A connection or a refusal is reach: a
refusal came back from the other box, so the way there works. Silence is
not. This is not a health check of the other box; it asks whether this
box's network still carries a connection off itself. A release that leaves
a box healthy inside and cut off from everyone is rolled back. Release 211
went back on node1 because node2 has nothing on 443 and an older agent did
not count a refusal. With no other boxes, there is nothing to compare and
reach always holds.

**Migrating to attestations.** Releases published before them carry
none, and the agent that checks them reaches each box in a release
published by the publisher that adds them. The rule is per box, kept in its state: until a
box has taken one release on two boxes' word it still takes a release
without attestations (one published by an older `dd`, or marked
bootstrap), and logs that it did; the first attested release it takes or
finds itself running writes `attested`, and from then on it takes nothing
less. A release that carries attestations is held to them from the start.
The alternative, requiring them from the agent's first run, refuses
nothing more in practice (publish has carried them since the same pull
request) but would leave no way to bootstrap a new fleet of two boxes
before dd-attest runs on them. A box that must be moved without two boxes
vouching once it requires them is moved by hand, at the box (remove
`/var/lib/dd-agent/attested`, or switch it yourself): no flag in a release
and no setting per box turns the check off.

**Why the counter only goes up.** Every release is signed, so an old one is
still a valid file. Without the counter, whoever serves `current.json` (the
forge, or anyone in front of it) could hand a box an old, signed release
with a known hole and it would move back to it. The box refuses anything
below what it applied; publish never reuses a number, and checks the
current one's signature before adding one to it.

Each run writes `/var/lib/dd-facts/dd_agent.prom` for the node exporter:

```
dd_agent_counter{box}                  the counter this box last applied
dd_agent_info{box,path,result}         what it runs; how the last run ended:
                                       ok, rollback, refused, absent, none
dd_agent_last_run_seconds{box}         when the agent last ran
```

The gate's Network page and `dd release status` read these.

## When a release does not land

Start with the status:

```
dd release status
  release 223  commit 512d508c95e9
  node1  counter 223  last run ok  current
    runs /nix/store/…-nixos-system-node1-…
  node2  counter 222  last run rollback  behind
    runs /nix/store/…-nixos-system-node2-…
```

Each box answers for itself, from its own Prometheus over ssh; a box that
is off says `unreachable`. `dd release show` prints the published release.

Then, on the box:

```
journalctl -u dd-agent                         what the agent did, and why it stopped
journalctl --since <then> | grep "Failed to start"   which unit failed the switch
cat /var/lib/dd-agent/refused                  releases this box will not try again
systemctl list-timers dd-agent-guard           a guard still armed (it should not be)
journalctl -u dd-build                         build orders (the app; not the boxes' release)
```

What the agent's last line means:

```
release refused: …                     not signed by the release key
release N is older than the M …        a counter going backwards
… applied N as something else          the same counter naming another system
release N has nothing for <box>        the box is not in the release
… was tried here and rolled back …     in refused; it will not try it again
… not switching to …: K of the 2 …    too few boxes of this fleet vouch for
                                       its closure; "attestation: …" lines say
                                       why each one did not count
… has nar hash X, the release says Y   the cache handed over something else
the closure under … is not the one …   the same, somewhere under the top path
… did not come up healthy; back on …   the probe or the reach failed;
                                       "probe: …" lines above say which
came up after a reboot and did not probe  the reboot case, rolled back
```

**A partial release** is normal for a while: each box moves on its own.
One box `ok` and one `rollback` means the release is bad for that box
only, and the next release is the fix; nothing needs undoing. A box never
retries a release it rolled back from, so a fix must be a new release.

**A unit that fails during the switch fails the switch**, and the box goes
back (the `switched.is_ok()` above). Release 212 rolled back on both
boxes for exactly this, `dd-build` running inside the switch that added
its timer; release 213 rolled back on node1 for `dd-ci-resume`, which
failed when the forge, restarting under the same switch, did not answer. The rules for a timer unit are in
[CI: when a release does not land](ci.md#when-a-release-does-not-land):
`OnActiveSec`, not `OnBootSec`; `restartIfChanged = false`; "not ready
yet" exits 0. `dd-build`'s timer is the example in
[nix/modules/box/agent.nix](../../nix/modules/box/agent.nix).

When publish itself stops, it says why before signing anything:

```
local main (…) is not what the forge has     push it
<box>: CI reported X, but main evaluates to Y  a status nobody should trust
<box>: N box(es) vouched (…), 2 needed       dd-attest's error is printed per box
the published release does not verify        current.json was tampered with
no current release could be read, yet …      the forge is not answering
```

## The bridges

Three environment variables exist for one-off crossings. None is for
everyday use.

- **`DD_ATTEST=off`**: publish without asking any box to vouch, for
  bootstrap only. The release says so under the release key
  (`"bootstrap": true`, no attestations). A fleet of one box needs it, and
  its agent takes it (fewer than two attesters). A box that has taken an
  attested release refuses it, whatever the fleet; on one that has not, it
  is taken and logged as bootstrap. It prints that no box vouched.
- **`DD_RELEASE_FORM=legacy`**: sign the old way and drop `closure`, the
  attestations and the bootstrap mark. For the
  one release that carried the v2-reading agent to boxes still running the
  old one. Any value but `legacy` or `v2` is an error.
- **`DD_BUILD_STORE=ssh-ng://admin@<box>`**: when publish has to build
  (CI's paths missing or not in the cache), build on that box rather than
  on the laptop. Unset, the laptop builds.

## The test

[nix/tests/release.nix](../../nix/tests/release.nix) and
[release.py](../../nix/tests/release.py), the `release` VM test in CI. A
`forge` machine is nginx serving one file and holds a release key made at
run time (`dd release init` with `DD_KEYRING_FILE`); the `box` gets only
its public half, with `probeSeconds = 20` and `reach = [ "forge:80" ]`.
Releases are signed with `dd release sign`. The box's specialisations are
what releases move it to:

```
release 1  the system it runs            nothing to do; counter 1
release 2  marked                        switched; no guard left; counter 2
release 1  again                         refused: older
release 5  with the counter edited       refused: not signed
release 3  broken (no sshd)              rolled back to marked; result rollback
release 4  stranded (port 80 dropped)    rolled back: could no longer reach the fleet
release 7  with the counter edited       refused; the status keeps counter 2
release 2  again, naming base            refused: one counter, one system
release 2  again, unchanged              taken: a re-publish
release 6  broken, agent killed          the guard: profile and system back on marked
```

The VM test's box has no attesters (`dd.agent.attesters` empty), so it
runs every case above as a fleet that cannot vouch. The attestations are
unit tests: in [lib.rs](../../servers/system/release/src/lib.rs) which
attestation counts (another box, path, closure, commit, a claimed
statement, another key) and the real `ssh-keygen` check (another key, the
wrong namespace, a changed statement, a name that would break the
allowed-signers line); in
[dd-agent.rs](../../servers/system/release/src/bin/dd-agent.rs) the rule
(`two_boxes_vouch_or_nothing_moves`) and a refusal writing nothing, two
real host keys being taken and `attested` written, the box alone not being
two, and a fleet file that does not parse stopping the agent rather than
turning the check off; in
[release_cmd.rs](../../client/cli/src/release_cmd.rs) that what publish
puts in the release is what a box checks and takes. The digest, the two
forms and the documents have unit tests in lib.rs too, including one that
plays the old agent against a bridging release; reach has one in
dd-agent.rs.

## Changing it

- **A new field in the release.** Add it to `Payload` or `BoxRelease` with
  `#[serde(default, skip_serializing_if = …)]`. Agents from v2 on verify a
  field they do not know; they just do not act on it. An agent must not
  *require* a new field until every box runs one that understands it, and
  releases from before it, still in `history/`, must still parse.
- **Changing what is signed** (the canonical form) strands every box the
  first time, unless a bridging release goes out in the old form first.
  Read the legacy story above and `form_tests` in lib.rs before touching
  `canonical_v2`.
- **The agent's checks.** Every new reason to roll back is a reason a
  healthy release can be refused on every box. Compare with what was true
  before the switch, as the failed units and reach do, so a fault already
  there does not block the release that fixes it. Add a case to the VM
  test.
- **probeSeconds or the guard.** The guard fires `2 × probeSeconds + 300`
  after it is armed: the switch, the probe window, then the reach window.
  It must stay longer than the agent can spend deciding, or it switches a
  box back under a release that was about to pass. If it does go off, the
  agent sees it before writing anything down and records a rollback, never
  an `ok` over the old system.
- **A new unit or timer on the boxes.** It must not fail during a switch
  (above). Run it in the VM test, and expect the box to roll back if it
  does.
- **The attest rule.** `ATTEST_NEED` (2, in lib.rs, for publish and the
  agent alike) and the accepted kinds of path are the whole of what stops
  one machine from choosing what the fleet runs. Loosening either needs
  the reason in the pull request. The agent's `judge` is the other half:
  never add a way for a release, or one box's setting, to switch it off.
- **A new box** gets an entry in `fleet/boxes.json` (with `net` and
  `hostKey`) and is in the next release; it is also asked to vouch from
  then on.
- **Never publish by hand around a refusal.** If publish refuses, the
  refusal is the finding. Fix the cause in a pull request, inside the
  release lock, like any change.
