# A box joining the fleet

Status: phase 1 of 3 built: a box joins **as a stranger**. Phase 2 (a
member's box, with a one-use pass from the member's account) and phase 3
(a box building its own config from its own report) are next.

A box the release does not list needs no one to add it. It joins the
control plane's agreed state on its own word, and joining gives it nothing
anyone needs to trust it with.

## As a stranger

- **Its name comes from its key.** `dd-control -join-key` takes the box's
  TPM-held key and joins under `j` and ten hex of the key's hash
  ([`state/join.go`](../../servers/system/control/state/join.go)). No
  other key can take that name, a box of the release's can't join a second
  time, and the relay refuses a hello for that address signed by any other
  key.
- **It holds no lease**, so it runs no service. The plan only gives it
  **copies**, where its store has room. A bucket is always kept where its
  service runs, on a box of the release, and a copy goes first to a box
  able to take the service over: a stranger's copy is one more place the
  ciphertext is, never the only one. A stranger saying it keeps a copy
  never pushes out a box of the release.
- **The network**: the gate hands it a key tagged `tag:joined`. A joined box
  reaches the other boxes' stores on port 3443 and nothing else. The
  boxes' gates, services and pages, all on 443, are out of its reach.
- **Its address** on the network is the one headscale gave it, said into
  the agreed state by the box running headscale (`netaddr`), never by the
  joined box itself. The DNS Worker points `*.<its name>.<domain>` there,
  and only at an address on the fleet's network.
- **Its store** gets a key at each other box's store, sealed to the age
  recipient it joined with (`dd-store-joined`). When it is gone from the
  agreed state, so is its key; what it copied there stays.
- **Bounds:** at most 32 strangers at once, 4 joins an hour across the
  fleet, 8 joining at once at the relay. One unheard for 30 days is
  dropped from the agreed state.

## What a stranger can do

Hold ciphertext it can't read, and fail to keep it: the box writing a
bucket reads a copy back, and a copy found missing counts for nothing.
Waste a copy slot that a box with more room would have had.

What it can't do: run anything, see any plaintext, reach any service, take
another box's name, or change what the plan does with the release's boxes.
