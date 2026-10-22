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
			"postgres": {Enabled: true, Instances: 1, MemoryGiB: 0.5},
			"games":    {Enabled: true, Class: "high", UpMbps: 10, MemoryGiB: 8, StorageGiB: 100},
		},
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

// built: the release builds box with svc, which runs there unswitched
func built(in Inputs, box string, svc ...string) {
	b := in.Boxes[box]
	b.Built = append(b.Built, svc...)
	in.Boxes[box] = b
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
	p := Decide(in, s, 90_000, nil)
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
	p := Decide(in, s, 90_000, nil)
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
	p := Decide(in, s, 90_000, nil)
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
	built(in, "nas", "games")
	p := Decide(in, s, 90_000, nil)
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
	p = Decide(in, s, 90_000, nil)
	if !runs(p, "big", "games") {
		t.Fatalf("games should move to big when nothing is carried: %+v", p)
	}
}

func TestNoRoomNoPlace(t *testing.T) {
	in, s := fleet()
	in.Boxes["small"].Can["games"] = 9 // small suits them best, but holds 8G * 0.85
	p := Decide(in, s, 90_000, nil)
	if runs(p, "small", "games") {
		t.Fatalf("games on a box without the room: %+v", p.Boxes["small"])
	}
}

func TestTheSameStateTheSamePlan(t *testing.T) {
	in, s := fleet()
	a, b := Decide(in, s, 90_000, nil), Decide(in, s, 90_000, nil)
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
	p := Decide(in, s, 90_000, nil)
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
	built(in, "big", "pihole")
	p := Decide(in, s, 90_000, nil)
	if !runs(p, "big", "pihole") || runs(p, "small", "pihole") {
		t.Fatalf("pihole should stay on big: big %+v small %+v", p.Boxes["big"], p.Boxes["small"])
	}
}

func TestAHouseMovesItsDNSWhenFarBetter(t *testing.T) {
	in, s := fleet()
	in.Boxes["small"].Can["pihole"] = 2
	built(in, "big", "pihole")
	p := Decide(in, s, 90_000, nil)
	if runs(p, "big", "pihole") || !runs(p, "small", "pihole") {
		t.Fatalf("pihole should move to small: big %+v small %+v", p.Boxes["big"], p.Boxes["small"])
	}
}

func TestWhatABoxMeasuredOverridesTheEstimate(t *testing.T) {
	in, s := fleet()
	// the estimate fits the nas; what games really hold does not
	in.Boxes["nas"].Can["games"] = 9
	s.Boxes["big"].Facts["svc/games/mem"] = 15 * gib
	p := Decide(in, s, 90_000, nil)
	if runs(p, "nas", "games") {
		t.Fatalf("games on the nas, which cannot hold the 15G measured: %+v", p.Boxes["nas"])
	}
	// but never below what it keeps aside
	delete(s.Boxes["big"].Facts, "svc/games/mem")
	s.Boxes["big"].Facts["svc/games/mem"] = 0.1 * gib
	sv := in.Services["games"]
	sv.ReserveGiB = 15
	in.Services["games"] = sv
	p = Decide(in, s, 90_000, nil)
	if runs(p, "nas", "games") {
		t.Fatalf("games on the nas past what they reserve: %+v", p.Boxes["nas"])
	}
}

func TestOneDatabaseItsUsersFollow(t *testing.T) {
	in, s := fleet()
	in.Boxes["nas"].Can["vault"], in.Boxes["nas"].Can["asend"], in.Boxes["nas"].Can["postgres"] = 1, 5, 1
	in.Boxes["big"].Can["vault"], in.Boxes["big"].Can["asend"] = 3, 1
	in.Services["vault"] = Service{Enabled: true, Instances: 1, MemoryGiB: 0.2}
	// asend suits the nas far better, and is decided first by name, but it
	// must be beside the one vault, which suits big
	in.Services["asend"] = Service{Enabled: true, MemoryGiB: 0.1, Local: []string{"vault"}}
	p := Decide(in, s, 90_000, nil)
	if !runs(p, "big", "vault") || !runs(p, "big", "asend") || runs(p, "nas", "vault") || runs(p, "nas", "asend") {
		t.Fatalf("vault and send together on big: big %+v nas %+v", p.Boxes["big"], p.Boxes["nas"])
	}
	// and with its database, the one postgres stays one
	sv := in.Services["vault"]
	sv.Local = []string{"postgres"}
	in.Services["vault"] = sv
	in.Boxes["nas"].Can["vault"] = 9
	p = Decide(in, s, 90_000, nil)
	if runs(p, "nas", "postgres") || runs(p, "nas", "vault") {
		t.Fatalf("a second postgres: %+v", p.Boxes["nas"])
	}
}

func TestAPlacedServiceGoesOnlyWhereItIsCarried(t *testing.T) {
	in, s := fleet()
	sv := in.Services["photos"]
	sv.Placed = true
	in.Services["photos"] = sv
	// big suits it, but has not got it
	if p := Decide(in, s, 90_000, nil); runs(p, "big", "photos") {
		t.Fatalf("photos placed where it is not carried: %+v", p)
	}
	s.Boxes["big"].Facts["carries/photos"] = 1
	if p := Decide(in, s, 90_000, nil); !runs(p, "big", "photos") {
		t.Fatalf("photos not placed where it is carried: %+v", p)
	}
}

func TestAPlacedServiceRunsWhereItsBoxSaysItDoes(t *testing.T) {
	in, s := fleet()
	in.Boxes["small"].Can["web"] = 0.5
	in.Boxes["nas"].Can["web"] = 1
	in.Services["web"] = Service{Enabled: true, Instances: 1, Class: "low", Placed: true}
	for _, b := range []string{"small", "nas"} {
		s.Boxes[b].Facts["carries/web"] = 1
	}
	// small already runs it: the damping keeps it there over a small gain
	in.Damping = Damping{Base: 1}
	s.Boxes["small"].Facts["runs/web"] = 1
	p := Decide(in, s, 90_000, nil)
	if !runs(p, "small", "web") || runs(p, "nas", "web") {
		t.Fatalf("web moved off the box running it: %+v", p)
	}
	// and being built with it does not count: a placed service runs where
	// the box's own report says, under its flag
	s.Boxes["small"].Facts["runs/web"] = 0
	built(in, "small", "web")
	if p := Decide(in, s, 90_000, nil); !runs(p, "nas", "web") {
		t.Fatalf("web stayed by what small is built with: %+v", p)
	}
}

// stores on big (room), nas (room) and small (nearly full); photos writes
// the bucket "ente", 40 GiB, kept on big so far
func stores() (Inputs, *state.State) {
	in, s := fleet()
	in.Buckets = map[string]Bucket{"ente": {Copies: 2, Service: "photos"}}
	set := func(name string, size, free, ente float64) {
		f := s.Boxes[name].Facts
		f["store_size_bytes"], f["store_free_bytes"] = size*gib, free*gib
		if ente > 0 {
			f["store/ente/bytes"] = ente * gib
		}
	}
	set("big", 1000, 900, 40)
	set("nas", 500, 400, 0)
	set("small", 100, 50, 0)
	return in, s
}

func TestABucketIsKeptWhereItIsWrittenAndWhereItFits(t *testing.T) {
	in, s := stores()
	p := Decide(in, s, 90_000, nil)
	if got := strings.Join(p.Copies["ente"], ","); got != "big,nas" {
		t.Fatalf("copies %q, unplaced %v", got, p.Unplaced)
	}
}

func TestACopyStaysWhereItIs(t *testing.T) {
	in, s := stores()
	s.Boxes["small"].Facts["store_free_bytes"] = 60 * gib
	s.Boxes["small"].Facts["store/ente/bytes"] = 40 * gib
	p := Decide(in, s, 90_000, nil)
	if got := strings.Join(p.Copies["ente"], ","); got != "big,small" {
		t.Fatalf("a copy moved to the roomier box: %q", got)
	}
}

func TestNoStoreWithRoomNoCopy(t *testing.T) {
	in, s := stores()
	s.Boxes["nas"].Facts["store_free_bytes"] = 80 * gib   // a tenth kept back: 30 to give
	s.Boxes["small"].Facts["store_free_bytes"] = 45 * gib // 35 to give
	p := Decide(in, s, 90_000, nil)
	if got := strings.Join(p.Copies["ente"], ","); got != "big" || p.Unplaced["copies/ente"] == "" {
		t.Fatalf("copied where it does not fit: %q %v", got, p.Unplaced)
	}
}

func TestAServiceGoesOnlyWhereItsBucketsFit(t *testing.T) {
	in, s := stores()
	// photos suits dead best, but dead is gone; big keeps the bucket. With
	// big full, photos may not move to nas, which has no room for 40 GiB
	in.Boxes["nas"].Can["photos"], in.Boxes["nas"].Can["postgres"] = 3, 3
	s.Boxes["big"].Facts["store/ente/bytes"] = 0
	s.Boxes["nas"].Facts["store_free_bytes"] = 60 * gib
	s.Boxes["dead"].Facts["store/ente/bytes"] = 40 * gib
	s.Boxes["dead"].LastSeen = s.Time - 1000
	s.Boxes["dead"].Facts["store_size_bytes"], s.Boxes["dead"].Facts["store_free_bytes"] = 100*gib, 10*gib
	p := Decide(in, s, 90_000, nil)
	if runs(p, "nas", "photos") {
		t.Fatalf("photos placed where its 40 GiB does not fit: %+v", p.Boxes)
	}
}

func TestABoxThatRefusedAServiceIsNotGivenIt(t *testing.T) {
	in, s := fleet()
	in.Services["web"] = Service{Enabled: true, Placed: true, MemoryGiB: 1}
	in.Boxes["big"].Can["web"], in.Boxes["nas"].Can["web"] = 5, 1
	s.Boxes["big"].Facts["carries/web"], s.Boxes["nas"].Facts["carries/web"] = 1, 1
	s.Boxes["big"].Facts["refused/web"] = 1
	p := Decide(in, s, 90_000, nil)
	if runs(p, "big", "web") || !runs(p, "nas", "web") {
		t.Fatalf("given to the box that refused it: %+v", p.Boxes)
	}
}

func TestABoxClaimingABigBucketCannotMakeItFitNowhere(t *testing.T) {
	in, s := stores()
	// small says it keeps 100 TB of it, and has no store to keep a copy
	delete(s.Boxes["small"].Facts, "store_size_bytes")
	s.Boxes["small"].Facts["store/ente/bytes"] = 100_000 * gib
	p := Decide(in, s, 90_000, nil)
	if got := strings.Join(p.Copies["ente"], ","); got != "big,nas" {
		t.Fatalf("one box's claim moved the copies: %q %v", got, p.Unplaced)
	}
}

func TestACopyFoundMissingIsNotKeptThere(t *testing.T) {
	in, s := stores()
	s.Boxes["nas"].Facts["store/ente/bytes"] = 40 * gib
	// small says nas lost it: small does not write it, so nothing changes
	s.Boxes["small"].Facts["store/ente/lost/nas"] = 1
	if got := strings.Join(Decide(in, s, 90_000, nil).Copies["ente"], ","); got != "big,nas" {
		t.Fatalf("a box that does not write it moved its copy: %q", got)
	}
	// big runs photos, so writes it, and found nas without it
	built(in, "big", "photos")
	s.Boxes["big"].Facts["store/ente/lost/nas"] = 1
	p := Decide(in, s, 90_000, nil)
	if got := strings.Join(p.Copies["ente"], ","); got != "big,small" {
		t.Fatalf("kept on the store that lost it: %q", got)
	}
}

func TestOnlyTheLeaseHolderSaysHowBigItsBucketIs(t *testing.T) {
	in, s := stores()
	sv := in.Services["photos"]
	sv.Lease, sv.Placed = true, true
	in.Services["photos"] = sv
	// nas says it runs photos and that ente is 100 TB; the lease is big's
	s.Boxes["nas"].Facts["runs/photos"] = 1
	s.Boxes["nas"].Facts["store/ente/bytes"] = 100_000 * gib
	s.Boxes["big"].Facts["runs/photos"] = 1
	s.Leases["svc/photos"] = &state.Lease{Holder: "big", Epoch: 1, Expires: s.Time + 15_000}
	p := Decide(in, s, 90_000, nil)
	if p.Unplaced["copies/ente"] != "" {
		t.Fatalf("a claim of the box without the lease sized the bucket: %v", p.Unplaced)
	}
}

func why(p Plan, box, svc string) string {
	for _, d := range p.Boxes[box] {
		if d.Service == svc {
			return d.Why
		}
	}
	return ""
}

// web, placed, moves on-failure, runs on small; nas suits it far better
func sticky() (Inputs, *state.State) {
	in, s := fleet()
	in.Boxes["small"].Can["web"] = 0.5
	in.Boxes["nas"].Can["web"] = 9
	in.Services["web"] = Service{Enabled: true, Instances: 1, Class: "low", Placed: true, Moves: "on-failure", MemoryGiB: 1}
	for _, b := range []string{"small", "nas"} {
		s.Boxes[b].Facts["carries/web"] = 1
	}
	s.Boxes["small"].Facts["runs/web"] = 1
	return in, s
}

func TestAHealthyHolderIsNeverMovedByABetterBox(t *testing.T) {
	in, s := sticky()
	p := Decide(in, s, 90_000, nil)
	if !runs(p, "small", "web") || runs(p, "nas", "web") || !strings.Contains(why(p, "small", "web"), "stays") {
		t.Fatalf("web moved off a live box: %+v", p)
	}
	if len(p.DoesNotFit) != 0 {
		t.Fatalf("flagged though it fits: %v", p.DoesNotFit)
	}
	// moving on a better score is still the default
	sv := in.Services["web"]
	sv.Moves = ""
	in.Services["web"] = sv
	if p := Decide(in, s, 90_000, nil); !runs(p, "nas", "web") {
		t.Fatalf("web stayed without on-failure: %+v", p)
	}
}

func TestADeadHolderIsMoved(t *testing.T) {
	in, s := sticky()
	s.Boxes["small"].LastSeen = 0
	p := Decide(in, s, 90_000, nil)
	if !runs(p, "nas", "web") {
		t.Fatalf("web not moved off a dead box: %+v", p)
	}
}

func TestAShrunkBoxKeepsItsServiceAndIsFlagged(t *testing.T) {
	in, s := sticky()
	s.Boxes["small"].Facts["ram_bytes"] = 1 * gib // 0.85G: garage fits, web beside it not
	in.Boxes["small"].Can["web"] = 0              // and it scores nothing
	p := Decide(in, s, 90_000, nil)
	if !runs(p, "small", "web") || runs(p, "nas", "web") {
		t.Fatalf("web moved off a live box that shrank: %+v", p)
	}
	if got := p.DoesNotFit["small"]; len(got) != 1 || got[0] != "web" || !strings.Contains(why(p, "small", "web"), "no longer fits") {
		t.Fatalf("doesNotFit %v, why %q", p.DoesNotFit, why(p, "small", "web"))
	}
	// a release that no longer lets it run there: it still stays
	delete(in.Boxes["small"].Can, "web")
	if p := Decide(in, s, 90_000, nil); !runs(p, "small", "web") {
		t.Fatalf("web moved once the box could not run it: %+v", p)
	}
}

func TestADrainingBoxIsGivenNothingNewAndWhatMovesLeaves(t *testing.T) {
	in, s := sticky()
	// games run on the nas, damped there (TestACostlyMoveIsDamped); web,
	// sticky, also runs there
	built(in, "nas", "games")
	s.Boxes["small"].Facts["runs/web"] = 0
	s.Boxes["nas"].Facts["runs/web"] = 1
	s.Boxes["nas"].Facts["draining"] = 1
	p := Decide(in, s, 90_000, nil)
	if runs(p, "nas", "games") || !runs(p, "big", "games") {
		t.Fatalf("games stayed on a draining box: %+v", p)
	}
	if runs(p, "nas", "garage") || runs(p, "nas", "pihole") {
		t.Fatalf("a draining box was given something new: %+v", p.Boxes["nas"])
	}
	if !runs(p, "nas", "web") {
		t.Fatalf("a sticky service left before its box did: %+v", p)
	}
	if _, ok := p.Unplaced["pihole@b"]; !ok {
		t.Fatalf("house b's only box drains, and its DNS is not said unplaced: %v", p.Unplaced)
	}
}

func TestADrainingStoreGetsNoNewCopyAndGivesItsOwnUp(t *testing.T) {
	in, s := stores()
	s.Boxes["nas"].Facts["draining"] = 1
	p := Decide(in, s, 90_000, nil)
	if got := strings.Join(p.Copies["ente"], ","); got != "big,small" {
		t.Fatalf("copies %q, unplaced %v", got, p.Unplaced)
	}
	// one it keeps already moves where there is room, and stays while
	// there is none
	s.Boxes["nas"].Facts["store/ente/bytes"] = 40 * gib
	if got := strings.Join(Decide(in, s, 90_000, nil).Copies["ente"], ","); got != "big,small" {
		t.Fatalf("copies %q", got)
	}
	s.Boxes["small"].Facts["store_free_bytes"] = 20 * gib
	if got := strings.Join(Decide(in, s, 90_000, nil).Copies["ente"], ","); got != "big,nas" {
		t.Fatalf("copies %q", got)
	}
}

func TestABoxWhoseHeartbeatGainsHardwareIsEligibleOnceTheReleaseSaysSo(t *testing.T) {
	in, s := fleet()
	in.Services["llm"] = Service{Enabled: true, Instances: 1, MemoryGiB: 7}
	p := Decide(in, s, 90_000, nil)
	if _, ok := p.Unplaced["llm"]; !ok {
		t.Fatalf("llm placed with no box able: %+v", p)
	}
	// nas got a GPU: its report, judged on the box, says it can now. Its
	// word alone adds nothing
	f := s.Boxes["nas"].Facts
	f["can_live"], f["can/llm"], f["can/garage"], f["can/pihole"] = 1, 2, 3, 1
	p = Decide(in, s, 90_000, nil)
	if runs(p, "nas", "llm") {
		t.Fatalf("llm on a box by its own word: %+v", p)
	}
	// its signed report, in the next release, does
	in.Boxes["nas"].Can["llm"] = 2
	p = Decide(in, s, 90_000, nil)
	if !runs(p, "nas", "llm") {
		t.Fatalf("llm not on the box the release says can now: %+v", p)
	}
	// and a box whose word leaves a service out cannot run it any more
	if runs(p, "nas", "games") {
		t.Fatalf("games on a box that no longer says it can: %+v", p.Boxes["nas"])
	}
	// small got more memory: 8G holds 0.85 * 8 = 6.8G, under llm; now 32G
	in.Boxes["small"].Can["llm"] = 5
	if p := Decide(in, s, 90_000, nil); runs(p, "small", "llm") {
		t.Fatalf("llm on a box without the room: %+v", p.Boxes["small"])
	}
	s.Boxes["small"].Facts["ram_bytes"] = 32 * gib
	if p := Decide(in, s, 90_000, nil); !runs(p, "small", "llm") {
		t.Fatalf("llm not on the box that grew: %+v", p)
	}
}

func TestABoxsOwnWordAddsNothingItsOwnerBarsNorALease(t *testing.T) {
	in, s := fleet()
	in.Services["llm"] = Service{Enabled: true, Instances: 1, MemoryGiB: 1}
	in.Services["db"] = Service{Enabled: true, Instances: 1, MemoryGiB: 1, Placed: true, Lease: true}
	b := in.Boxes["nas"]
	b.Barred = []string{"llm"}
	in.Boxes["nas"] = b
	f := s.Boxes["nas"].Facts
	f["can_live"], f["can/llm"], f["can/db"], f["carries/db"] = 1, 9, 9, 1
	p := Decide(in, s, 90_000, nil)
	if runs(p, "nas", "llm") || runs(p, "nas", "db") {
		t.Fatalf("a box's word added what the release keeps off it: %+v", p.Boxes["nas"])
	}
	// a lease the release lets it hold, it may be given
	in.Boxes["nas"].Can["db"] = 0.1
	if p := Decide(in, s, 90_000, nil); !runs(p, "nas", "db") {
		t.Fatalf("db: %+v", p)
	}
}

func TestAJoinedBoxKeepsCopiesAndRunsNothing(t *testing.T) {
	in, s := stores()
	// nas has no room; a stranger with a roomy store joins
	s.Boxes["nas"].Facts["store_free_bytes"] = 10 * gib
	s.Joined = map[string]*state.Joined{"jabc": {Key: "k"}}
	s.Boxes["jabc"] = &state.BoxState{LastSeen: s.Time - 1000, Facts: map[string]float64{
		"store_size_bytes": 1000 * gib, "store_free_bytes": 900 * gib, "ram_bytes": 64 * gib,
	}}
	p := Decide(in, s, 90_000, nil)
	if len(p.Boxes["jabc"]) != 0 {
		t.Fatalf("a stranger was given a service: %+v", p.Boxes["jabc"])
	}
	if got := strings.Join(p.Copies["ente"], ","); got != "big,jabc" {
		t.Fatalf("copies %q", got)
	}
}

func TestACopyGoesFirstToABoxThatCouldTakeTheServiceOver(t *testing.T) {
	in, s := stores()
	in.Boxes["small"].Can["photos"], in.Boxes["small"].Can["postgres"] = 0.5, 0.5
	// a stranger with far more room
	s.Joined = map[string]*state.Joined{"jabc": {Key: "k"}}
	s.Boxes["jabc"] = &state.BoxState{LastSeen: s.Time - 1000, Facts: map[string]float64{
		"store_size_bytes": 1000 * gib, "store_free_bytes": 900 * gib,
	}}
	p := Decide(in, s, 90_000, nil)
	if got := strings.Join(p.Copies["ente"], ","); got != "big,small" {
		t.Fatalf("copies %q: the box that could take photos over comes first", got)
	}
}

func TestAStrangerSayingItKeepsACopyPushesOutNoBoxOfTheRelease(t *testing.T) {
	in, s := stores()
	s.Boxes["nas"].Facts["store/ente/bytes"] = 40 * gib
	s.Joined = map[string]*state.Joined{"jabc": {Key: "k"}}
	s.Boxes["jabc"] = &state.BoxState{LastSeen: s.Time - 1000, Facts: map[string]float64{
		"store_size_bytes": 1000 * gib, "store_free_bytes": 900 * gib, "store/ente/bytes": 40 * gib,
	}}
	p := Decide(in, s, 90_000, nil)
	if got := strings.Join(p.Copies["ente"], ","); got != "big,nas" {
		t.Fatalf("copies %q: the stranger's claim took the release box's slot", got)
	}
}

func TestAMembersBoxKeepsItsCopyAsABoxOfTheReleaseDoes(t *testing.T) {
	in, s := stores()
	s.Boxes["nas"].Facts["store/ente/bytes"] = 40 * gib
	s.Joined = map[string]*state.Joined{"jabc": {Key: "k"}}
	s.Boxes["jabc"] = &state.BoxState{LastSeen: s.Time - 1000, Facts: map[string]float64{
		"store_size_bytes": 1000 * gib, "store_free_bytes": 900 * gib, "store/ente/bytes": 50 * gib,
	}}
	p := Decide(in, s, 90_000, map[string]bool{"jabc": true})
	if got := strings.Join(p.Copies["ente"], ","); got != "big,jabc" {
		t.Fatalf("copies %q: the member's box holding more lost its copy", got)
	}
	if len(p.Boxes["jabc"]) != 0 {
		t.Fatalf("a member's box was given a service: %+v", p.Boxes["jabc"])
	}
	// a stranger with the same holding does not
	if got := strings.Join(Decide(in, s, 90_000, nil).Copies["ente"], ","); got != "big,nas" {
		t.Fatalf("copies %q", got)
	}
}

func TestASmallBoxsOwnRunnerIsSizedByItsOwnMeasure(t *testing.T) {
	in, s := fleet()
	// a runner on every box, taking the jobs its box holds: big's ran
	// 12G, the nas's 3G
	in.Services["runner"] = Service{Enabled: true, Spread: true, MemoryGiB: 6}
	for _, b := range []string{"big", "nas"} {
		in.Boxes[b].Can["runner"] = 1
	}
	s.Boxes["big"].Facts["svc/runner/mem"] = 12 * gib
	s.Boxes["nas"].Facts["svc/runner/mem"] = 3 * gib
	s.Boxes["nas"].Facts["svc/garage/mem"] = 6 * gib
	p := Decide(in, s, 90_000, nil)
	// 6 + 3 fits the nas's 13.6G; 6 + 12 would not
	if !runs(p, "big", "runner") || !runs(p, "nas", "runner") {
		t.Fatalf("a runner each on big and the nas: %+v", p.Boxes)
	}
	// a box that never ran it is sized by the most any box measured
	in.Boxes["small"].Can["runner"] = 1
	p = Decide(in, s, 90_000, nil)
	if runs(p, "small", "runner") {
		t.Fatalf("small's 6.8G holds no 12G runner: %+v", p.Boxes["small"])
	}
}

// Placed names each placed service's boxes from the plan alone: a box's own
// runs/<service> fact gives it nothing the plan did not (the DNS Worker
// points names by this)
func TestPlacedIsThePlanNotABoxsWord(t *testing.T) {
	in, s := fleet()
	sv := in.Services["pihole"]
	sv.Placed = true
	in.Services["pihole"] = sv
	for _, b := range []string{"big", "small", "nas"} {
		s.Boxes[b].Facts["carries/pihole"] = 1
	}
	// small says it runs games, which it cannot, and the plan never gives it
	s.Boxes["small"].Facts["runs/games"] = 1
	p := Decide(in, s, 90_000, nil)
	got := Placed(in, p)
	if _, ok := got["games"]; ok {
		t.Fatalf("games is not placed, so it has no boxes here: %v", got)
	}
	if _, ok := got["garage"]; ok {
		t.Fatalf("garage is not placed: %v", got)
	}
	want := []string{}
	for _, b := range []string{"big", "nas", "small"} {
		if runs(p, b, "pihole") {
			want = append(want, b)
		}
	}
	if len(want) == 0 || strings.Join(got["pihole"], ",") != strings.Join(want, ",") {
		t.Fatalf("pihole: placed %v, the plan %v", got["pihole"], want)
	}
}

// a joined box that says it can run, carries and runs a service is given
// nothing, nor counted as running it, until the release says joined boxes
// are attested; and nothing then that the release's rules for them leave out
func joinedClaiming(svcs ...string) (Inputs, *state.State) {
	in, s := fleet()
	// web: placed, runnable by small alone in the release's table
	in.Services["web"] = Service{Enabled: true, Placed: true, Class: "low", MemoryGiB: 0.1, Moves: "on-failure"}
	in.Boxes["small"].Can["web"] = 0.1
	s.Boxes["small"].Facts["carries/web"] = 1
	s.Joined = map[string]*state.Joined{"jabc": {Key: "k"}}
	f := map[string]float64{"ram_bytes": 64 * gib, "net_up_mbps": 1000, "net_down_mbps": 1000, "can_live": 1}
	for _, svc := range svcs {
		f["can/"+svc], f["carries/"+svc], f["runs/"+svc] = 5, 1, 1
	}
	s.Boxes["jabc"] = &state.BoxState{LastSeen: s.Time - 1000, Facts: f}
	return in, s
}

func TestAJoinedBoxClaimingAServiceIsNotPlaced(t *testing.T) {
	for _, rules := range []*JoinedRules{nil, {Attested: false, Services: []string{"web", "photos"}}} {
		in, s := joinedClaiming("web", "photos", "postgres")
		in.Joined = rules
		p := Decide(in, s, 90_000, nil)
		if len(p.Boxes["jabc"]) != 0 {
			t.Fatalf("rules %+v: a joined box on its own word was given %+v", rules, p.Boxes["jabc"])
		}
		if !runs(p, "small", "web") {
			t.Fatalf("rules %+v: web, which it said it runs (and moves on failure), was kept off its real box: %+v", rules, p.Boxes)
		}
		if got := Placed(in, p)["web"]; strings.Join(got, ",") != "small" {
			t.Fatalf("rules %+v: web's names would go to %v", rules, got)
		}
	}
}

func TestAnAttestedJoinedBoxRunsOnlyWhatItsTierAllows(t *testing.T) {
	in, s := joinedClaiming("web", "photos", "postgres")
	// photos is the owner's alone, and postgres a writer: left out
	in.Joined = &JoinedRules{Attested: true, Services: []string{"web", "postgres"}}
	pg := in.Services["postgres"]
	pg.Lease, pg.Placed = true, true
	in.Services["postgres"] = pg
	p := Decide(in, s, 90_000, nil)
	if !runs(p, "jabc", "web") {
		t.Fatalf("web, allowed and claimed, sticky where it runs: %+v", p.Boxes)
	}
	for _, svc := range []string{"photos", "postgres"} {
		if runs(p, "jabc", svc) {
			t.Fatalf("%s on a member's box: %+v", svc, p.Boxes["jabc"])
		}
	}
	// and its word only narrows: not claimed, not given
	in2, s2 := joinedClaiming()
	in2.Joined = &JoinedRules{Attested: true, Services: []string{"web"}}
	if p := Decide(in2, s2, 90_000, nil); len(p.Boxes["jabc"]) != 0 {
		t.Fatalf("given what it did not say it can: %+v", p.Boxes["jabc"])
	}
}

// a release box's own word narrows the release's table, never widens it
func TestALiveWordNeverAddsAService(t *testing.T) {
	in, s := fleet()
	s.Boxes["small"].Facts["can_live"] = 1
	s.Boxes["small"].Facts["can/games"] = 9
	s.Boxes["small"].Facts["can/garage"] = 1
	p := Decide(in, s, 90_000, nil)
	if runs(p, "small", "games") {
		t.Fatalf("small said it can run games, which the release does not give it: %+v", p.Boxes["small"])
	}
}
