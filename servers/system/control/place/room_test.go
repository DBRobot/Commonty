package place

import (
	"encoding/json"
	"os"
	"strings"
	"testing"

	"commonty/control/state"
)

// the agreed state of 2026-10-24 (release 359) as the next release's first
// round sees it (scripts/plan-check): no placed service up yet, each box
// carrying what that release builds it with, and that release's inputs
func liveOct24(t *testing.T) (Inputs, *state.State) {
	raw, err := os.ReadFile("testdata/live-2026-10-24.json")
	if err != nil {
		t.Fatal(err)
	}
	var f struct {
		Inputs Inputs       `json:"inputs"`
		State  *state.State `json:"state"`
	}
	f.State = state.New()
	if err := json.Unmarshal(raw, &f); err != nil {
		t.Fatal(err)
	}
	return f.Inputs, f.State
}

// node1's sum of what its services measured or reserve (llama-swap's 25G,
// the CI runner's 13.5G peak, members-runner's 4G estimate now that it
// spreads) left no room for vaultwarden and the breach list beside it, with
// 45G of its 62G free. Passwords went unplaced, which stops them.
func TestPasswordsArePlacedWhenTheSumOfEstimatesIsFull(t *testing.T) {
	in, s := liveOct24(t)
	p := Decide(in, s, 90_000, nil)
	for _, svc := range []string{"vaultwarden", "vaultwarden-send", "pwned-list"} {
		if why, ok := p.Unplaced[svc]; ok {
			t.Fatalf("%s unplaced: %s", svc, why)
		}
		if !runs(p, "node1", svc) {
			t.Fatalf("%s not on node1, where it runs now: node1 %+v, node2 %+v", svc, p.Boxes["node1"], p.Boxes["node2"])
		}
	}
	// the two go together: whichever is decided first brings the other
	if w := why(p, "node1", "vaultwarden") + why(p, "node1", "pwned-list"); !strings.Contains(w, "no box has room") {
		t.Fatalf("why %q", w)
	}
	for _, svc := range []string{"vaultwarden", "pwned-list"} {
		found := false
		for _, x := range p.DoesNotFit["node1"] {
			found = found || x == svc
		}
		if !found {
			t.Fatalf("%s not flagged as past node1's room: %v", svc, p.DoesNotFit)
		}
	}
	for svc, why := range p.Unplaced {
		if !strings.HasPrefix(svc, "copies/") {
			t.Fatalf("%s unplaced: %s", svc, why)
		}
	}
}

// with no box that has room, a service goes where it fits least badly,
// flagged; the box's own check of its free memory (cmd/dd-control/placed.go)
// is what refuses it
func TestNoBoxWithRoomTheLeastFullTakesIt(t *testing.T) {
	in, s := fleet()
	in.Services["vault"] = Service{Enabled: true, Instances: 1, MemoryGiB: 60}
	in.Boxes["big"].Can["vault"], in.Boxes["nas"].Can["vault"] = 1, 2
	p := Decide(in, s, 90_000, nil)
	if !runs(p, "big", "vault") || runs(p, "nas", "vault") {
		t.Fatalf("vault not on the box nearest to holding it: %+v", p.Boxes)
	}
	if got := p.DoesNotFit["big"]; len(got) != 1 || got[0] != "vault" {
		t.Fatalf("doesNotFit %v", p.DoesNotFit)
	}
	// a box that refused it for want of memory is not given it again
	s.Boxes["big"].Facts["refused/vault"] = 1
	in.Services["vault"] = Service{Enabled: true, Instances: 1, MemoryGiB: 60, Placed: true}
	s.Boxes["big"].Facts["carries/vault"], s.Boxes["nas"].Facts["carries/vault"] = 1, 1
	p = Decide(in, s, 90_000, nil)
	if runs(p, "big", "vault") || !runs(p, "nas", "vault") {
		t.Fatalf("given to the box that refused it: %+v", p.Boxes)
	}
}

// what has no room in its store is still not placed: a store is not
// checked again by the box
func TestNoStoreRoomStillNoPlace(t *testing.T) {
	in, s := stores()
	in.Boxes["nas"].Can["photos"], in.Boxes["nas"].Can["postgres"] = 3, 3
	s.Boxes["big"].Facts["store/ente/bytes"] = 0
	s.Boxes["big"].Facts["store_free_bytes"] = 1 * gib
	s.Boxes["nas"].Facts["store_free_bytes"] = 1 * gib
	s.Boxes["dead"].Facts["store/ente/bytes"] = 40 * gib
	s.Boxes["dead"].LastSeen = s.Time - 1000
	s.Boxes["dead"].Facts["store_size_bytes"], s.Boxes["dead"].Facts["store_free_bytes"] = 100*gib, 10*gib
	delete(in.Boxes["dead"].Can, "photos")
	p := Decide(in, s, 90_000, nil)
	if runs(p, "nas", "photos") || runs(p, "big", "photos") {
		t.Fatalf("photos placed where its 40 GiB does not fit: %+v", p.Boxes)
	}
}
