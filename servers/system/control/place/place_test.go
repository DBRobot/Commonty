package place

import (
	"strings"
	"testing"

	"commonty/control/state"
)

func fleet() (Inputs, *state.State) {
	in := Inputs{
		Boxes: map[string]Box{
			"big":   {Site: "a", Can: map[string]float64{"garage": 2, "photos": 1, "postgres": 1, "pihole": 1, "games": 1}},
			"small": {Site: "a", Can: map[string]float64{"garage": 1, "pihole": 1}},
			"nas":   {Site: "b", Can: map[string]float64{"garage": 3, "pihole": 1, "games": 0.5}},
			"dead":  {Site: "c", Can: map[string]float64{"garage": 5, "photos": 5, "postgres": 5, "pihole": 5}},
		},
		Services: map[string]Service{
			"garage":   {Enabled: true, Spread: true, Class: "high", UpMbps: 50, MemoryGiB: 0.5},
			"pihole":   {Enabled: true, PerHouse: 1, MemoryGiB: 0.1},
			"photos":   {Enabled: true, Class: "medium", UpMbps: 10, MemoryGiB: 1, StorageGiB: 20, Local: []string{"postgres"}},
			"postgres": {Enabled: true, MemoryGiB: 0.5},
			"games":    {Enabled: true, Class: "high", UpMbps: 10, MemoryGiB: 8, StorageGiB: 100},
		},
		Roles:   map[string][]string{"games": {"games"}},
		Damping: Damping{Base: 0.3, PerGiB: 0.02},
	}
	s := state.New()
	s.Time = 1_000_000
	for name, f := range map[string]map[string]float64{
		"big":   {"ram_bytes": 64 * gib, "net_up_mbps": 50, "net_down_mbps": 500},
		"small": {"ram_bytes": 8 * gib, "net_up_mbps": 50, "net_down_mbps": 500},
		"nas":   {"ram_bytes": 16 * gib, "net_up_mbps": 1000, "net_down_mbps": 1000},
	} {
		s.Boxes[name] = &state.BoxState{LastSeen: s.Time - 1000, Facts: f}
	}
	s.Boxes["dead"] = &state.BoxState{LastSeen: 0, Facts: map[string]float64{"ram_bytes": 64 * gib}}
	return in, s
}

func runs(p Plan, box, svc string) bool {
	for _, d := range p.Boxes[box] {
		if d.Service == svc {
			return true
		}
	}
	return false
}

func TestSpreadGoesOnEveryLiveBoxThatCan(t *testing.T) {
	in, s := fleet()
	p := Decide(in, s, 90_000)
	for _, b := range []string{"big", "small", "nas"} {
		if !runs(p, b, "garage") {
			t.Fatalf("garage not on %s: %+v", b, p)
		}
	}
	if _, ok := p.Boxes["dead"]; ok {
		t.Fatalf("a box unheard past goneAfter got a plan: %+v", p.Boxes["dead"])
	}
}

func TestOnePerHouse(t *testing.T) {
	in, s := fleet()
	p := Decide(in, s, 90_000)
	n := 0
	for _, b := range []string{"big", "small"} {
		if runs(p, b, "pihole") {
			n++
		}
	}
	if n != 1 || !runs(p, "nas", "pihole") {
		t.Fatalf("pihole: one in house a and one in b, got %+v", p)
	}
}

func TestWhatMustShareABoxComesWithIt(t *testing.T) {
	in, s := fleet()
	p := Decide(in, s, 90_000)
	if !runs(p, "big", "photos") || !runs(p, "big", "postgres") {
		t.Fatalf("photos and its postgres together on the box that can: %+v", p.Boxes["big"])
	}
	count := 0
	for _, ds := range p.Boxes {
		for _, d := range ds {
			if d.Service == "postgres" {
				count++
			}
		}
	}
	if count != 1 {
		t.Fatalf("postgres once, beside photos, not again for its own count: %d", count)
	}
}

func TestACostlyMoveIsDamped(t *testing.T) {
	in, s := fleet()
	// games run on the nas now; big suits them more, but not by enough to
	// carry 100G
	s.Boxes["nas"].Roles = []string{"games"}
	p := Decide(in, s, 90_000)
	if !runs(p, "nas", "games") || runs(p, "big", "games") {
		t.Fatalf("games should stay on the nas: %+v", p)
	}
	var why string
	for _, d := range p.Boxes["nas"] {
		if d.Service == "games" {
			why = d.Why
		}
	}
	if !strings.HasPrefix(why, "stays:") {
		t.Fatalf("why: %q", why)
	}
	// with nothing to carry, the better box wins
	sv := in.Services["games"]
	sv.StorageGiB = 0
	in.Services["games"] = sv
	in.Boxes["big"].Can["games"] = 5
	p = Decide(in, s, 90_000)
	if !runs(p, "big", "games") {
		t.Fatalf("games should move to big when nothing is carried: %+v", p)
	}
}

func TestNoRoomNoPlace(t *testing.T) {
	in, s := fleet()
	in.Boxes["small"].Can["games"] = 9 // small suits them best, but holds 8G * 0.85
	p := Decide(in, s, 90_000)
	if runs(p, "small", "games") {
		t.Fatalf("games on a box without the room: %+v", p.Boxes["small"])
	}
}

func TestTheSameStateTheSamePlan(t *testing.T) {
	in, s := fleet()
	a, b := Decide(in, s, 90_000), Decide(in, s, 90_000)
	for box := range a.Boxes {
		if len(a.Boxes[box]) != len(b.Boxes[box]) {
			t.Fatalf("plans differ on %s", box)
		}
		for i := range a.Boxes[box] {
			if a.Boxes[box][i] != b.Boxes[box][i] {
				t.Fatalf("plans differ on %s: %+v vs %+v", box, a.Boxes[box][i], b.Boxes[box][i])
			}
		}
	}
}

func TestEveryBoxCountsAgainstRoom(t *testing.T) {
	in, s := fleet()
	in.Services["report"] = Service{Enabled: true, EveryBox: true, MemoryGiB: 6}
	in.Boxes["nas"].Can["games"] = 9
	p := Decide(in, s, 90_000)
	for _, b := range []string{"big", "small", "nas"} {
		if !runs(p, b, "report") {
			t.Fatalf("report not on %s", b)
		}
	}
	// 6 + 8 is past what 16G holds at 85%
	if runs(p, "nas", "games") {
		t.Fatalf("games on the nas past its room: %+v", p.Boxes["nas"])
	}
}

func TestAHouseKeepsItsDNSWhereItIs(t *testing.T) {
	in, s := fleet()
	in.Boxes["small"].Can["pihole"] = 1.1 // a little better than big, not by the margin
	s.Boxes["big"].Roles = []string{"dns"}
	in.Roles["dns"] = []string{"pihole"}
	p := Decide(in, s, 90_000)
	if !runs(p, "big", "pihole") || runs(p, "small", "pihole") {
		t.Fatalf("pihole should stay on big: big %+v small %+v", p.Boxes["big"], p.Boxes["small"])
	}
}

func TestAHouseMovesItsDNSWhenFarBetter(t *testing.T) {
	in, s := fleet()
	in.Boxes["small"].Can["pihole"] = 2
	s.Boxes["big"].Roles = []string{"dns"}
	in.Roles["dns"] = []string{"pihole"}
	p := Decide(in, s, 90_000)
	if runs(p, "big", "pihole") || !runs(p, "small", "pihole") {
		t.Fatalf("pihole should move to small: big %+v small %+v", p.Boxes["big"], p.Boxes["small"])
	}
}
