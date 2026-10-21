package place

import (
	"strings"
	"testing"

	"commonty/control/state"
)

// node1 runs the writers (postgres, museum beside it with its bucket) and a
// stateless placed page; node2 and node3 carry them all
func releasing() (Inputs, *state.State) {
	can := map[string]float64{"postgres": 1, "museum": 1, "web": 1}
	in := Inputs{
		Boxes: map[string]Box{"node1": {Site: "a", Can: can}, "node2": {Site: "a", Can: can}, "node3": {Site: "b", Can: can}},
		Services: map[string]Service{
			"postgres": {Enabled: true, Placed: true, Lease: true, Instances: 1, MemoryGiB: 0.5},
			"museum":   {Enabled: true, Placed: true, Lease: true, Instances: 1, MemoryGiB: 0.1, Local: []string{"postgres"}},
			"web":      {Enabled: true, Placed: true, MemoryGiB: 0.1},
		},
		Buckets: map[string]Bucket{"ente": {Copies: 1, Service: "museum"}},
		Damping: Damping{Base: 0.3, PerGiB: 0.01},
	}
	s := state.New()
	s.Time = 1_000_000
	for _, name := range []string{"node1", "node2", "node3"} {
		f := map[string]float64{"ram_bytes": 32 * gib, "release": 6, "store_size_bytes": 1000 * gib, "store_free_bytes": 900 * gib}
		for svc := range in.Services {
			f["carries/"+svc] = 1
			if name == "node1" {
				f["runs/"+svc] = 1
			}
		}
		s.Boxes[name] = &state.BoxState{LastSeen: s.Time - 1000, Facts: f}
	}
	s.Boxes["node1"].Facts["store/ente/bytes"] = 300 * gib
	s.Leases["svc/museum"] = &state.Lease{Holder: "node1", Epoch: 1, Expires: s.Time + 10_000}
	return in, s
}

func TestNothingMovesWithoutADrain(t *testing.T) {
	in, s := releasing()
	p := Decide(in, s, 90_000, nil)
	for _, svc := range []string{"postgres", "museum", "web"} {
		if !runs(p, "node1", svc) {
			t.Fatalf("%s left node1: %+v", svc, p.Boxes)
		}
	}
}

func TestAReleasingBoxHandsItsGroupToABoxOnTheNewRelease(t *testing.T) {
	in, s := releasing()
	s.Boxes["node1"].Facts["release_drain"] = 7
	s.Boxes["node3"].Facts["release"] = 7
	p := Decide(in, s, 90_000, nil)
	for _, svc := range []string{"postgres", "museum", "web"} {
		if runs(p, "node1", svc) || !runs(p, "node3", svc) {
			t.Fatalf("%s not handed to node3, on release 7: %+v", svc, p.Boxes)
		}
	}
	if !strings.Contains(why(p, "node3", "postgres"), "on the new release") {
		t.Fatalf("why: %s", why(p, "node3", "postgres"))
	}
	if len(p.Unplaced) != 0 {
		t.Fatalf("unplaced: %v", p.Unplaced)
	}
}

// node2's store has no room for the photos (its old pool not yet retired):
// the whole group stays and restarts in place; the page still moves
func TestAGroupThatFitsNowhereRestartsInPlace(t *testing.T) {
	in, s := releasing()
	delete(in.Boxes, "node3")
	delete(s.Boxes, "node3")
	s.Boxes["node1"].Facts["release_drain"] = 7
	s.Boxes["node2"].Facts["store_free_bytes"] = 150 * gib
	p := Decide(in, s, 90_000, nil)
	if !runs(p, "node1", "postgres") || !runs(p, "node1", "museum") || runs(p, "node2", "postgres") {
		t.Fatalf("the group split or left: %+v", p.Boxes)
	}
	if !strings.Contains(why(p, "node1", "postgres"), "restarts in place") {
		t.Fatalf("why: %s", why(p, "node1", "postgres"))
	}
	if runs(p, "node1", "web") || !runs(p, "node2", "web") {
		t.Fatalf("the page stayed: %+v", p.Boxes)
	}
	if len(p.Unplaced) != 0 {
		t.Fatalf("a drain left something with nowhere to run: %v", p.Unplaced)
	}
}

// wave 1 (two boxes): nobody is on the new release yet, so the group goes
// to a held box rather than nowhere
func TestWithNoneOnTheReleaseAHeldBoxTakesIt(t *testing.T) {
	in, s := releasing()
	delete(in.Boxes, "node3")
	delete(s.Boxes, "node3")
	s.Boxes["node1"].Facts["release_drain"] = 7
	p := Decide(in, s, 90_000, nil)
	for _, svc := range []string{"postgres", "museum", "web"} {
		if !runs(p, "node2", svc) {
			t.Fatalf("%s not on node2: %+v", svc, p.Boxes)
		}
	}
}

// once the switch is over and the fact gone, what moved stays where it is
// by the ordinary margin: nothing moves back on its own
func TestAfterTheSwitchNothingMovesBack(t *testing.T) {
	in, s := releasing()
	for svc := range in.Services {
		s.Boxes["node1"].Facts["runs/"+svc] = 0
		s.Boxes["node3"].Facts["runs/"+svc] = 1
	}
	s.Boxes["node3"].Facts["store/ente/bytes"] = 300 * gib
	s.Leases["svc/museum"] = &state.Lease{Holder: "node3", Epoch: 2, Expires: s.Time + 10_000}
	p := Decide(in, s, 90_000, nil)
	for _, svc := range []string{"postgres", "museum", "web"} {
		if !runs(p, "node3", svc) {
			t.Fatalf("%s moved back: %+v", svc, p.Boxes)
		}
	}
}
