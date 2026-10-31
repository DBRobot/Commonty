package place

import (
	"encoding/json"
	"os"
	"sort"
	"testing"

	"commonty/control/state"
)

// testdata/fleet-inputs.json is the release's placement-inputs for node1 and
// node2; testdata/fleet-plans.json what Decide gave for each state below, made
// by the code before ownership reached it (PLACE_GOLDEN=1 writes it again)
func fleetInputs(t *testing.T) Inputs {
	t.Helper()
	raw, err := os.ReadFile("testdata/fleet-inputs.json")
	if err != nil {
		t.Fatal(err)
	}
	in := Inputs{}
	if err := json.Unmarshal(raw, &in); err != nil {
		t.Fatal(err)
	}
	return in
}

// each listed box alive, carrying and running what the release gives it
func fleetState(in Inputs) *state.State {
	s := state.New()
	s.Time = 10_000_000
	for name, b := range in.Boxes {
		f := map[string]float64{
			"ram_bytes": b.MemoryGiB * gib, "net_up_mbps": 100, "net_down_mbps": 500,
			"store_size_bytes": 1000 * gib, "store_free_bytes": 600 * gib, "kvm": 1, "x86_64": 1, "release": 7,
		}
		for svc := range b.Can {
			if in.Services[svc].Placed {
				f["carries/"+svc] = 1
			}
		}
		s.Boxes[name] = &state.BoxState{LastSeen: s.Time - 1000, Facts: f}
	}
	for svc, sv := range in.Services {
		if sv.Placed && !sv.Lease {
			if _, ok := in.Boxes["node1"].Can[svc]; ok {
				s.Boxes["node1"].Facts["runs/"+svc] = 1
			}
		}
		if sv.Lease {
			s.Leases["svc/"+svc] = &state.Lease{Holder: "node1", Epoch: 3, Expires: s.Time + 60_000}
		}
	}
	return s
}

func readGolden() (map[string]Plan, error) {
	raw, err := os.ReadFile("testdata/fleet-plans.json")
	if err != nil {
		return nil, err
	}
	out := map[string]Plan{}
	return out, json.Unmarshal(raw, &out)
}

func fleetScenes() map[string]func(*state.State) {
	return map[string]func(*state.State){
		"both up":         func(*state.State) {},
		"node2 draining":  func(s *state.State) { s.Boxes["node2"].Facts["draining"] = 1 },
		"node1 gone":      func(s *state.State) { s.Boxes["node1"].LastSeen = 0 },
		"node2 gone":      func(s *state.State) { s.Boxes["node2"].LastSeen = 0 },
		"node1 switching": func(s *state.State) { s.Boxes["node1"].Facts["release_drain"] = 8 },
		"node1 judges itself": func(s *state.State) {
			f := s.Boxes["node1"].Facts
			f["can_live"] = 1
			for k := range f {
				if len(k) > 8 && k[:8] == "carries/" {
					f["can/"+k[8:]] = 1
				}
			}
		},
	}
}

func TestNode1AndNode2DecideAsBefore(t *testing.T) {
	in := fleetInputs(t)
	got := map[string]Plan{}
	for name, scene := range fleetScenes() {
		s := fleetState(in)
		scene(s)
		got[name] = Decide(in, s, 90_000, nil)
	}
	raw, _ := json.MarshalIndent(got, "", " ")
	if os.Getenv("PLACE_GOLDEN") != "" {
		if err := os.WriteFile("testdata/fleet-plans.json", append(raw, '\n'), 0o644); err != nil {
			t.Fatal(err)
		}
		return
	}
	want, err := os.ReadFile("testdata/fleet-plans.json")
	if err != nil {
		t.Fatal(err)
	}
	if string(append(raw, '\n')) != string(want) {
		var w map[string]Plan
		_ = json.Unmarshal(want, &w)
		names := []string{}
		for n := range got {
			names = append(names, n)
		}
		sort.Strings(names)
		for _, n := range names {
			a, _ := json.Marshal(got[n])
			b, _ := json.Marshal(w[n])
			if string(a) != string(b) {
				t.Errorf("%s: node1 and node2's plan changed:\n got %s\nwant %s", n, a, b)
			}
		}
	}
}
