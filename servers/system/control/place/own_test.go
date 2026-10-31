package place

import (
	"encoding/json"
	"reflect"
	"testing"

	"commonty/control/state"
	"commonty/control/state/ident"
)

// a fleet with node1 of the release, and joined boxes that say they can run,
// carry and run everything asked of them
func ownFleet(joined map[string]string, svcs ...string) (Inputs, *state.State) {
	in := Inputs{
		Owner: "david",
		Boxes: map[string]Box{"node1": {Site: "home", Owner: "david", Can: map[string]float64{"games": 1, "dns": 1, "llm": 1}}},
		Services: map[string]Service{
			"games": {Enabled: true, Placed: true, Instances: 1, Moves: "on-failure", MemoryGiB: 1},
			"dns":   {Enabled: true, Placed: true, PerHouse: 1, MemoryGiB: 0.1},
			"llm":   {Enabled: true, Placed: true, Instances: 1, MemoryGiB: 1},
			"web":   {Enabled: true, Placed: true, Spread: true, MemoryGiB: 0.1},
		},
		Joined: &JoinedRules{Attested: true, Own: []string{"games", "dns"}, Services: []string{"web"}},
	}
	s := state.New()
	s.Time = 1_000_000
	s.Boxes["node1"] = &state.BoxState{LastSeen: s.Time, Facts: map[string]float64{
		"ram_bytes": 64 * gib, "carries/games": 1, "carries/dns": 1, "carries/llm": 1, "runs/games": 1, "runs/dns": 1, "runs/llm": 1,
	}}
	s.Joined = map[string]*state.Joined{}
	for name, house := range joined {
		s.Joined[name] = &state.Joined{Key: name, House: house}
		f := map[string]float64{"ram_bytes": 32 * gib, "can_live": 1}
		for _, svc := range svcs {
			f["can/"+svc], f["carries/"+svc] = 9, 1
		}
		s.Boxes[name] = &state.BoxState{LastSeen: s.Time, Facts: f}
	}
	return in, s
}

func TestOwnersAreOnlyMembersWithEnlistmentsThatStand(t *testing.T) {
	v := &ident.View{Members: map[string]*ident.Member{
		"ann": {Status: "member", Enlisted: []ident.BoxView{{Box: "ja"}, {Box: "jb"}}},
		"bea": {Status: "suspended", Enlisted: []ident.BoxView{{Box: "jc"}}},
		"cat": {Status: "left", Enlisted: []ident.BoxView{{Box: "jd"}}},
		"dan": {Status: "revoked", Enlisted: []ident.BoxView{{Box: "je"}}},
		"eve": {Status: "none", Enlisted: []ident.BoxView{{Box: "jf"}}},
	}}
	if got, want := Owners(v), (map[string]string{"ja": "ann", "jb": "ann"}); !reflect.DeepEqual(got, want) {
		t.Fatalf("owners %v, want %v", got, want)
	}
	if len(Owners(nil)) != 0 {
		t.Fatal("no view, and someone owns a box")
	}
}

func TestAStrangerRunsNothingWhatItClaims(t *testing.T) {
	in, s := ownFleet(map[string]string{"ja": "joined"}, "games", "dns", "web", "llm")
	s.Boxes["ja"].Facts["runs/games"] = 1
	p := Decide(in, s, 90_000, nil)
	if len(p.Boxes["ja"]) != 0 {
		t.Fatalf("a box nobody has enlisted was given %+v", p.Boxes["ja"])
	}
}

func TestAMembersOwnGoOnTheirBoxAndNeverTakeTheFleets(t *testing.T) {
	in, s := ownFleet(map[string]string{"ja": "joined", "jb": "joined"}, "games", "web", "llm")
	s.Boxes["ja"].Facts["runs/games"] = 1
	own := map[string]string{"ja": "ann", "jb": "bea"}
	p := Decide(in, s, 90_000, own)
	if !isOwn(p, "ja", "games") || !isOwn(p, "jb", "games") {
		t.Fatalf("each member has their own: %+v", p.Boxes)
	}
	if !runs(p, "node1", "games") || isOwn(p, "node1", "games") {
		t.Fatalf("the fleet's stays on node1: %+v", p.Boxes["node1"])
	}
	if got := Placed(in, p)["games"]; !reflect.DeepEqual(got, []string{"node1"}) {
		t.Fatalf("the fleet's name follows %v", got)
	}
	// a service outside the tier stays off, whatever the box says
	if runs(p, "ja", "llm") || !runs(p, "ja", "web") {
		t.Fatalf("what ann's box may run: %+v", p.Boxes["ja"])
	}
	// node1 gone: nobody's own becomes the fleet's
	s.Boxes["node1"].LastSeen = 0
	p = Decide(in, s, 90_000, own)
	if len(Placed(in, p)["games"]) != 0 || p.Unplaced["games"] == "" {
		t.Fatalf("a member's box took the fleet's games: %+v", p)
	}
	// one instance for each member, however many boxes
	in, s = ownFleet(map[string]string{"ja": "joined", "jb": "joined"}, "games")
	p = Decide(in, s, 90_000, map[string]string{"ja": "ann", "jb": "ann"})
	if runs(p, "ja", "games") == runs(p, "jb", "games") {
		t.Fatalf("not one for ann: %+v", p.Boxes)
	}
}

func TestAnOwnStaysWhereItRunsAndGoesWithTheBox(t *testing.T) {
	in, s := ownFleet(map[string]string{"ja": "joined", "jb": "joined"}, "games")
	s.Boxes["jb"].Facts["runs/games"] = 1
	own := map[string]string{"ja": "ann", "jb": "ann"}
	if p := Decide(in, s, 90_000, own); !runs(p, "jb", "games") || runs(p, "ja", "games") {
		t.Fatalf("it moved: %+v", p.Boxes)
	}
	s.Boxes["jb"].Facts["draining"] = 1
	if p := Decide(in, s, 90_000, own); !runs(p, "ja", "games") {
		t.Fatalf("a box draining kept it: %+v", p.Boxes)
	}
	s.Boxes["jb"].LastSeen = 0
	if p := Decide(in, s, 90_000, own); !runs(p, "ja", "games") {
		t.Fatalf("gone with its box: %+v", p.Boxes)
	}
}

// whatever the box says or its build carries, a loss is in the very next plan
func TestALossIsInThePlanAtOnce(t *testing.T) {
	in, s := ownFleet(map[string]string{"ja": "joined"}, "games", "dns", "web")
	s.Boxes["ja"].Facts["runs/games"] = 1
	s.Boxes["ja"].Facts["runs/web"] = 1
	if p := Decide(in, s, 90_000, map[string]string{"ja": "ann"}); !runs(p, "ja", "games") || !runs(p, "ja", "web") {
		t.Fatalf("its owner's box: %+v", p.Boxes["ja"])
	}
	p := Decide(in, s, 90_000, map[string]string{})
	if len(p.Boxes["ja"]) != 0 {
		t.Fatalf("ann's enlistment ended and it kept %+v", p.Boxes["ja"])
	}
	if _, in := p.Boxes["ja"]; !in {
		t.Fatal("a box not in the plan changes nothing: it must be there, with nothing")
	}
}

func TestTheFleetOwnersImagedBoxIsTheirs(t *testing.T) {
	in, s := ownFleet(map[string]string{"ja": "joined", "jb": "joined"}, "llm", "games")
	s.Boxes["node1"].LastSeen = 0
	s.Leases["svc/llm"] = nil
	p := Decide(in, s, 90_000, map[string]string{"ja": "david", "jb": "ann"})
	if !runs(p, "ja", "llm") || isOwn(p, "ja", "llm") {
		t.Fatalf("what they keep to their own boxes, on their box: %+v", p.Boxes["ja"])
	}
	if !runs(p, "ja", "games") || isOwn(p, "ja", "games") {
		t.Fatalf("the fleet's games, on the fleet owner's box: %+v", p.Boxes["ja"])
	}
	if runs(p, "jb", "llm") {
		t.Fatalf("what is theirs on another's box: %+v", p.Boxes["jb"])
	}
	// a writer stays off it: leases are not for such boxes yet
	sv := in.Services["llm"]
	sv.Lease = true
	in.Services["llm"] = sv
	if p := Decide(in, s, 90_000, map[string]string{"ja": "david"}); runs(p, "ja", "llm") {
		t.Fatalf("a writer on a joined box: %+v", p.Boxes["ja"])
	}
	// no switch, nothing
	in.Joined.Attested = false
	if p := Decide(in, s, 90_000, map[string]string{"ja": "david"}); len(p.Boxes["ja"]) != 0 {
		t.Fatalf("before the switch: %+v", p.Boxes["ja"])
	}
}

func TestHousesAreEachOwnersOwn(t *testing.T) {
	in, s := ownFleet(map[string]string{"ja": "h1", "jb": "h1", "jc": "h1", "jd": "joined", "je": "joined"}, "dns")
	own := map[string]string{"ja": "ann", "jb": "ann", "jc": "bea", "jd": "ann", "je": "ann"}
	for _, c := range []struct{ a, b string }{{"ja", "jc"}, {"jd", "je"}} {
		if siteOf(in, s, own, c.a) == siteOf(in, s, own, c.b) {
			t.Fatalf("%s and %s are one house", c.a, c.b)
		}
	}
	if siteOf(in, s, own, "ja") != siteOf(in, s, own, "jb") {
		t.Fatal("two boxes of one member saying one house are not in it")
	}
	if siteOf(in, s, own, "node1") != "home" {
		t.Fatal("a box of the release is in its own site")
	}
	p := Decide(in, s, 90_000, own)
	n := map[string]int{}
	for _, b := range []string{"ja", "jb", "jc", "jd", "je"} {
		if runs(p, b, "dns") {
			n[siteOf(in, s, own, b)]++
		}
	}
	if len(n) != 4 || n["ann/h1"] != 1 || n["bea/h1"] != 1 {
		t.Fatalf("a house's DNS by house: %v, %+v", n, p.Boxes)
	}
	if !runs(p, "node1", "dns") {
		t.Fatalf("node1's house lost its DNS: %+v", p.Boxes["node1"])
	}
}

// the real inputs, with members' and strangers' boxes saying they run all of it
func TestNode1AndNode2DecideAsBeforeBesideOtherBoxes(t *testing.T) {
	in := fleetInputs(t)
	raw, err := readGolden()
	if err != nil {
		t.Fatal(err)
	}
	everyService := []string{}
	for svc := range in.Services {
		everyService = append(everyService, svc)
	}
	for name, scene := range fleetScenes() {
		for _, attested := range []bool{false, true} {
			i := in
			i.Owner = "david"
			i.Joined = &JoinedRules{}
			if attested {
				i.Joined = &JoinedRules{Attested: true, Own: everyService}
			}
			s := fleetState(i)
			s.Joined = map[string]*state.Joined{}
			for _, j := range []string{"jann", "jbea", "jstranger"} {
				s.Joined[j] = &state.Joined{Key: j, House: "joined"}
				f := map[string]float64{
					"ram_bytes": 64 * gib, "can_live": 1, "kvm": 1, "x86_64": 1,
					"store_size_bytes": 10 * gib, "store_free_bytes": 1 * gib,
				}
				for _, svc := range everyService {
					f["can/"+svc], f["carries/"+svc], f["runs/"+svc], f["svc/"+svc+"/mem"] = 99, 1, 1, 1000*gib
				}
				s.Boxes[j] = &state.BoxState{LastSeen: s.Time, Facts: f}
			}
			scene(s)
			p := Decide(i, s, 90_000, map[string]string{"jann": "ann", "jbea": "bea"})
			for _, b := range []string{"node1", "node2"} {
				got, _ := json.Marshal(p.Boxes[b])
				want, _ := json.Marshal(raw[name].Boxes[b])
				if string(got) != string(want) {
					t.Errorf("%s, attested %v: %s changed:\n got %s\nwant %s", name, attested, b, got, want)
				}
			}
			if attested {
				for _, d := range p.Boxes["jstranger"] {
					if !i.Services[d.Service].EveryBox {
						t.Errorf("%s: a stranger was given %+v", name, d)
					}
				}
			}
		}
	}
}
