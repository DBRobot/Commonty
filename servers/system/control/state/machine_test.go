package state

import (
	"encoding/json"
	"strings"
	"testing"
)

// Until version 2 is agreed, the state serializes as release 359's did:
// nothing of the machine, the config or a rollout in its bytes, so its
// hash is the one a validator of 359 computes
func TestAStateBeforeVersion2HasNothingNew(t *testing.T) {
	f := rolloutFleet(t)
	f.s.Machine = 0
	f.beat("node1", 1000)
	b, _ := json.Marshal(f.s)
	for _, k := range []string{`"machine"`, `"config"`, `"rollout"`} {
		if strings.Contains(string(b), k) {
			t.Fatalf("%s in a version 1 state: %s", k, b)
		}
	}
	if f.s.Version() != 1 {
		t.Fatalf("version %d", f.s.Version())
	}
}

// A rollout request under version 1 is refused as release 359 refused it,
// so its sequence is not taken either; under version 2 it begins one
func TestRolloutRequestsWaitForVersion2(t *testing.T) {
	f := rolloutFleet(t)
	f.s.Machine = 0
	f.beat("node1", 1000)
	seq := f.s.Boxes["node1"].Seq
	r := f.req("node1", "rollout", RolloutOp{Op: "begin", Release: release(t, 5, relKey, "node1", "node2")})
	if err := f.s.Check(f.c, fakeVerify, r, 2000); err == nil || !strings.Contains(err.Error(), "no such request") {
		t.Fatalf("version 1 took a rollout request: %v", err)
	}
	if f.block(2000, r) != 0 || f.s.Rollout != nil || f.s.Boxes["node1"].Seq != seq {
		t.Fatal("a rollout request changed a version 1 state")
	}
	f.s.Machine = 2
	if !f.roll("node1", 3000, RolloutOp{Op: "begin", Release: release(t, 5, relKey, "node1", "node2")}) || f.s.Rollout == nil {
		t.Fatal("version 2 did not begin the rollout")
	}
}

// A block applies under the config before this release's until one is
// agreed, and under the agreed one after: never under this release's own
// while it is not agreed
func TestTheConfigInForce(t *testing.T) {
	own := cfg()
	before := cfg()
	own.Before = before
	s := New()
	if s.In(own) != before {
		t.Fatal("not the config before, with none agreed")
	}
	if c := cfg(); s.In(c) != c {
		t.Fatal("not its own, with none agreed and none before")
	}
	agreedRaw, d := own.Canonical()
	if strings.Contains(string(agreedRaw), `"before"`) {
		t.Fatal("the agreed config carries the one before")
	}
	s.Machine, s.Config = 2, agreedRaw
	got := s.In(own)
	if got == own || got == before || s.ConfigDigest() != d {
		t.Fatal("not the agreed config")
	}
	if s.In(own) != got {
		t.Fatal("one agreed config read twice: its computer-lease replay would start over")
	}
	if _, d2 := got.Canonical(); d2 != d {
		t.Fatal("the agreed config read back is another")
	}
}

// Version 2 waits for every live box to say it runs it; one long gone
// does not hold it back
func TestBehindIsTheLiveBoxesBelow(t *testing.T) {
	f := newFleet(t)
	f.beat("node1", 1000)
	f.beatFacts("node2", 1000, map[string]float64{MachineFact: 2})
	if b := f.s.Behind(f.c, f.c, 2, 2000); len(b) != 1 || b[0] != "node1" {
		t.Fatalf("behind %v", b)
	}
	f.beatFacts("node1", 3000, map[string]float64{MachineFact: 2})
	if b := f.s.Behind(f.c, f.c, 2, 4000); len(b) != 0 {
		t.Fatalf("behind %v", b)
	}
	f.beat("node1", 5000)
	if b := f.s.Behind(f.c, f.c, 2, 5000+f.c.Rules.GoneAfter+1); len(b) != 0 {
		t.Fatalf("a box gone holds it back: %v", b)
	}
}

// A box the next config drops, or knows by another key, never holds a
// version back, whatever it says
func TestBehindCountsOnlyBoxesBothConfigsKnowAlike(t *testing.T) {
	f := newFleet(t)
	f.beat("node1", 1000)
	f.beat("node2", 1000)
	next := cfg()
	delete(next.Boxes, "node1")
	b := next.Boxes["node2"]
	b.Key = "another"
	next.Boxes["node2"] = b
	if got := f.s.Behind(f.c, next, 2, 2000); len(got) != 0 {
		t.Fatalf("a dropped or rekeyed box holds it back: %v", got)
	}
	if got := f.s.Behind(f.c, f.c, 2, 2000); len(got) != 2 {
		t.Fatalf("behind %v", got)
	}
}

// Across a config change a request counts only under one key in both
func TestSameKey(t *testing.T) {
	f := newFleet(t)
	next := cfg()
	b := next.Boxes["node2"]
	b.Key = "another"
	next.Boxes["node2"] = b
	delete(next.Boxes, "node1")
	for _, c := range []struct {
		r    Request
		want bool
	}{
		{f.req("node1", "heartbeat", Heartbeat{}), false}, // dropped
		{f.req("node2", "heartbeat", Heartbeat{}), false}, // rekeyed
		{Request{Kind: "join", Box: "node3"}, false},
	} {
		if f.s.SameKey(f.c, next, c.r) != c.want {
			t.Fatalf("%s %s", c.r.Kind, c.r.Box)
		}
	}
	if !f.s.SameKey(f.c, f.c, f.req("node2", "heartbeat", Heartbeat{})) {
		t.Fatal("one config refused its own box")
	}
}
