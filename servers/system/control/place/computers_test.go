package place

import (
	"strings"
	"testing"

	"commonty/control/state"
	"commonty/control/state/computers"
)

const (
	idA = "aaaaaaaaaaaaaaaa"
	idB = "bbbbbbbbbbbbbbbb"
	idC = "cccccccccccccccc"
)

// three live boxes with KVM, their whole room for computers, and one gone
func compute() (Inputs, *state.State) {
	in := Inputs{
		Boxes: map[string]Box{
			"node1": {Site: "a", Owner: "david", Can: map[string]float64{ComputersService: 1}, MemoryGiB: 64, Threads: 16, PoolGiB: 4000},
			"node2": {Site: "a", Owner: "ann", Can: map[string]float64{ComputersService: 1}, MemoryGiB: 16, Threads: 4, PoolGiB: 500},
			"pi":    {Site: "b", Owner: "david", Can: map[string]float64{ComputersService: 1}, MemoryGiB: 64, Threads: 8, PoolGiB: 1000},
			"gone":  {Site: "c", Owner: "david", Can: map[string]float64{ComputersService: 1}, MemoryGiB: 128, Threads: 32, PoolGiB: 8000},
		},
		Services: map[string]Service{},
		Damping:  Damping{Base: 0.3, PerGiB: 0.01},
	}
	s := state.New()
	s.Time = 1_000_000
	room := func(memG, threads, diskG float64) map[string]float64 {
		return map[string]float64{"kvm": 1, "x86_64": 1, "computer_max_mem_bytes": memG * gib, "computer_max_threads": threads, "computer_max_disk_bytes": diskG * gib}
	}
	for name, f := range map[string]map[string]float64{
		"node1": room(32, 15, 1000),
		"node2": room(8, 3, 100),
		// no KVM, and an arm box: room said, but no computer there
		"pi": {"x86_64": 0, "kvm": 1, "computer_max_mem_bytes": 64 * gib, "computer_max_threads": 8, "computer_max_disk_bytes": 1000 * gib},
	} {
		s.Boxes[name] = &state.BoxState{LastSeen: s.Time - 1000, Facts: f}
	}
	s.Boxes["gone"] = &state.BoxState{LastSeen: 0, Facts: room(64, 31, 4000)}
	return in, s
}

func computer(id, owner string, memMiB, threads, diskGiB uint64, consent string, boxes ...string) *computers.Listed {
	if boxes == nil {
		boxes = []string{}
	}
	return &computers.Listed{Owner: owner, ID: id, Name: id[:4], Memory: memMiB, Threads: threads, Disk: diskGiB, Consent: consent, Anywhere: consent == "any", Asked: true, Up: true, Boxes: boxes}
}

func holder(p Plan, id string) string {
	for name, ds := range p.Boxes {
		for _, d := range ds {
			if d.Service == ComputerService(id) {
				return name
			}
		}
	}
	return ""
}

func TestAComputerIsPlacedOnlyWhereItFits(t *testing.T) {
	in, s := compute()
	// too big for node2 in memory, threads or disk; node1 holds it
	in.Computers = []*computers.Listed{
		computer(idA, "ann", 16*1024, 2, 50, "any"),
		computer(idB, "ann", 4*1024, 8, 50, "any"),
		computer(idC, "ann", 4*1024, 2, 500, "any"),
	}
	p := Decide(in, s, 90_000, nil)
	for _, id := range []string{idA, idB, idC} {
		if h := holder(p, id); h != "node1" {
			t.Fatalf("%s on %q, want node1: %+v", id, h, p)
		}
	}
	// past every box's room: nowhere, and said why
	in.Computers = []*computers.Listed{computer(idA, "ann", 64*1024, 2, 50, "any")}
	p = Decide(in, s, 90_000, nil)
	if holder(p, idA) != "" || p.Unplaced[ComputerService(idA)] == "" {
		t.Fatalf("placed past every box's room: %+v", p)
	}
}

func TestNoKVMOrNotX86NoComputer(t *testing.T) {
	in, s := compute()
	s.Boxes["node1"].Facts["kvm"] = 0
	s.Boxes["node2"].Facts["computer_max_mem_bytes"] = 0 // what a box with no KVM writes
	in.Computers = []*computers.Listed{computer(idA, "ann", 1024, 1, 10, "any")}
	if p := Decide(in, s, 90_000, nil); holder(p, idA) != "" {
		t.Fatalf("placed on %s", holder(p, idA))
	}
}

func TestConsentOwnStaysOnTheOwnersBoxes(t *testing.T) {
	in, s := compute()
	// node1 suits it better; ann's own box is node2
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 1, 10, "own")}
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "node2" {
		t.Fatalf("own: on %q, want node2", h)
	}
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 1, 10, "any")}
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "node1" {
		t.Fatalf("any: on %q, want the roomier node1", h)
	}
	// own, and the owner's only box has no room: it waits
	in.Computers = []*computers.Listed{computer(idA, "ann", 16*1024, 1, 10, "own", "node2")}
	p := Decide(in, s, 90_000, nil)
	if holder(p, idA) != "" || !strings.Contains(p.Unplaced[ComputerService(idA)], "owner") {
		t.Fatalf("own with no room: %+v", p)
	}
}

func TestAComputerStaysWhileItsHolderHeartbeats(t *testing.T) {
	in, s := compute()
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 1, 10, "any")}
	s.Boxes["node2"].Facts["runs/"+ComputerService(idA)] = 1
	p := Decide(in, s, 90_000, nil)
	if h := holder(p, idA); h != "node2" {
		t.Fatalf("moved off a live holder to %q", h)
	}
	if !strings.Contains(why(p, "node2", ComputerService(idA)), "stays") || len(p.DoesNotFit) != 0 {
		t.Fatalf("why %q, doesNotFit %v", why(p, "node2", ComputerService(idA)), p.DoesNotFit)
	}
}

func TestAComputerMovesWhenItsHolderDiesToABoxThatFits(t *testing.T) {
	in, s := compute()
	in.Computers = []*computers.Listed{computer(idA, "ann", 4096, 2, 50, "any")}
	s.Boxes["gone"].Facts["runs/"+ComputerService(idA)] = 1
	s.Boxes["gone"].Facts["computer/"+idA+"/used_bytes"] = 40 * gib
	// node2 kept an older copy of its disk: carrying less, it wins over the
	// roomier node1
	s.Boxes["node2"].Facts["computer/"+idA+"/used_bytes"] = 39 * gib
	p := Decide(in, s, 90_000, nil)
	if h := holder(p, idA); h != "node2" {
		t.Fatalf("on %q, want node2, which has most of it: %+v", h, p)
	}
	// with none kept anywhere, the roomier box
	delete(s.Boxes["node2"].Facts, "computer/"+idA+"/used_bytes")
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "node1" {
		t.Fatalf("on %q, want node1", h)
	}
}

func TestAComputerThatNoLongerFitsStaysAndIsFlagged(t *testing.T) {
	in, s := compute()
	in.Computers = []*computers.Listed{computer(idA, "ann", 6*1024, 2, 50, "any")}
	s.Boxes["node2"].Facts["runs/"+ComputerService(idA)] = 1
	s.Boxes["node2"].Facts["computer_max_mem_bytes"] = 4 * gib
	p := Decide(in, s, 90_000, nil)
	if holder(p, idA) != "node2" {
		t.Fatalf("moved: %+v", p)
	}
	if got := p.DoesNotFit["node2"]; len(got) != 1 || got[0] != ComputerService(idA) {
		t.Fatalf("doesNotFit %v", p.DoesNotFit)
	}
}

func TestADrainingBoxGetsNoComputerAndKeepsItsOwn(t *testing.T) {
	in, s := compute()
	s.Boxes["node1"].Facts["draining"] = 1
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 1, 10, "any"), computer(idB, "ann", 2048, 1, 10, "any")}
	s.Boxes["node1"].Facts["runs/"+ComputerService(idB)] = 1
	p := Decide(in, s, 90_000, nil)
	if h := holder(p, idA); h != "node2" {
		t.Fatalf("new one on %q, want node2", h)
	}
	if h := holder(p, idB); h != "node1" || len(p.DoesNotFit) != 0 {
		t.Fatalf("running one on %q (doesNotFit %v), want it kept on draining node1", h, p.DoesNotFit)
	}
}

func TestAComputerThatIsOffIsNotPlaced(t *testing.T) {
	in, s := compute()
	c := computer(idA, "ann", 2048, 1, 10, "any")
	c.Up = false
	in.Computers = []*computers.Listed{c}
	s.Boxes["node1"].Facts["runs/"+ComputerService(idA)] = 1
	p := Decide(in, s, 90_000, nil)
	if holder(p, idA) != "" || p.Unplaced[ComputerService(idA)] != "" {
		t.Fatalf("an off computer was placed: %+v", p)
	}
}

func TestABoxsRoomLessWhatIsPlacedThere(t *testing.T) {
	in, s := compute()
	// node2 alone (own): 8G, 3 threads, 100G; one running there holds 6G of it
	running := computer(idA, "ann", 6*1024, 1, 40, "own", "node2")
	s.Boxes["node2"].Facts["runs/"+ComputerService(idA)] = 1
	s.Boxes["node2"].Facts["computer/"+idA+"/used_bytes"] = 30 * gib
	// 2G fits beside it, 4G more does not
	small := computer(idB, "ann", 2*1024, 1, 40, "own", "node2")
	big := computer(idC, "ann", 4*1024, 1, 10, "own", "node2")
	in.Computers = []*computers.Listed{big, running, small}
	p := Decide(in, s, 90_000, nil)
	if holder(p, idA) != "node2" || holder(p, idB) != "node2" || holder(p, idC) != "" {
		t.Fatalf("a=%q b=%q c=%q", holder(p, idA), holder(p, idB), holder(p, idC))
	}
	// disk: 100G free, less the 10G the running one may still grow into
	small.Disk = 91
	if p := Decide(in, s, 90_000, nil); holder(p, idB) != "" {
		t.Fatalf("placed past the disk left")
	}
	small.Disk = 90
	if p := Decide(in, s, 90_000, nil); holder(p, idB) != "node2" {
		t.Fatalf("not placed in the disk left")
	}
}

func TestComputersPlanTheSameEveryTime(t *testing.T) {
	in, s := compute()
	in.Computers = []*computers.Listed{computer(idC, "ann", 2048, 1, 10, "any"), computer(idA, "bob", 2048, 1, 10, "any"), computer(idB, "cy", 2048, 1, 10, "any")}
	a := Decide(in, s, 90_000, nil)
	in.Computers[0], in.Computers[2] = in.Computers[2], in.Computers[0]
	b := Decide(in, s, 90_000, nil)
	for _, id := range []string{idA, idB, idC} {
		if holder(a, id) != holder(b, id) {
			t.Fatalf("%s: %q then %q", id, holder(a, id), holder(b, id))
		}
	}
}

// a box that joined on its own key, or any box the release does not list,
// is a stranger: it holds ciphertext and runs nothing, whatever room it says
func TestAStrangerGetsNoComputer(t *testing.T) {
	in, s := compute()
	s.Boxes["stranger"] = &state.BoxState{LastSeen: s.Time - 1000, Facts: map[string]float64{"kvm": 1, "x86_64": 1, "can_live": 1, "can/" + ComputersService: 9, "computer_max_mem_bytes": 1 << 50, "computer_max_threads": 1024, "computer_max_disk_bytes": 1 << 60}}
	in.Computers = []*computers.Listed{computer(idA, "ann", 64*1024, 2, 50, "any")}
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "" {
		t.Fatalf("placed on %q", h)
	}
}

func TestOnlyABoxTheReleaseLetsHoldComputersGetsOne(t *testing.T) {
	in, s := compute()
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 1, 10, "any")}
	// node1's release table does not name computers: its room does not count
	b := in.Boxes["node1"]
	b.Can = map[string]float64{}
	in.Boxes["node1"] = b
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "node2" {
		t.Fatalf("on %q, want node2", h)
	}
	// nor where its owner's box.json or the policy bar them
	b = in.Boxes["node2"]
	b.Barred = []string{ComputersService}
	in.Boxes["node2"] = b
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "" {
		t.Fatalf("on %q, want nowhere", h)
	}
}

func TestABoxsOwnWordNeverMakesItAHostForComputers(t *testing.T) {
	in, s := compute()
	b := in.Boxes["node1"]
	b.Can = map[string]float64{}
	in.Boxes["node1"] = b
	s.Boxes["node1"].Facts["can_live"] = 1
	s.Boxes["node1"].Facts["can/"+ComputersService] = 5
	delete(in.Boxes, "node2")
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 1, 10, "any")}
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "" {
		t.Fatalf("on %q, which only its own heartbeat says can", h)
	}
}

func TestOwnGoesOnlyOnABoxTheReleaseSaysIsTheOwners(t *testing.T) {
	in, s := compute()
	// ann named node1 hers by her word alone; the release says it is david's
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 1, 10, "own", "node1")}
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "node2" {
		t.Fatalf("on %q, want ann's node2 by the release", h)
	}
	in.Computers = []*computers.Listed{computer(idA, "david", 2048, 1, 10, "own")}
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "node1" {
		t.Fatalf("on %q, want david's node1", h)
	}
}

// saying it runs one keeps a computer only on a box that may hold it
func TestAStrangerSayingItRunsAComputerDoesNotKeepIt(t *testing.T) {
	in, s := compute()
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 1, 10, "any")}
	s.Boxes["stranger"] = &state.BoxState{LastSeen: s.Time - 1000, Facts: map[string]float64{"runs/" + ComputerService(idA): 1}}
	s.Boxes["node2"].Facts["runs/"+ComputerService(idA)] = 1
	b := in.Boxes["node2"]
	b.Barred = []string{ComputersService}
	in.Boxes["node2"] = b
	in.Boxes["stranger"] = Box{Site: "z"} // listed, but not for computers
	p := Decide(in, s, 90_000, nil)
	if h := holder(p, idA); h != "node1" {
		t.Fatalf("on %q, want node1: %+v", h, p)
	}
}

// a box's room is its word, never past the hardware its signed report gives
// the release, nor what its heartbeat measures
func TestABoxsRoomIsNeverPastItsHardware(t *testing.T) {
	for name, lie := range map[string]func(in *Inputs, s *state.State){
		"memory":  func(_ *Inputs, s *state.State) { s.Boxes["node2"].Facts["computer_max_mem_bytes"] = 1 << 50 },
		"threads": func(_ *Inputs, s *state.State) { s.Boxes["node2"].Facts["computer_max_threads"] = 1024 },
		"disk":    func(_ *Inputs, s *state.State) { s.Boxes["node2"].Facts["computer_max_disk_bytes"] = 1 << 60 },
		"memory, by its heartbeat": func(in *Inputs, s *state.State) {
			b := in.Boxes["node2"]
			b.MemoryGiB = 1024
			in.Boxes["node2"] = b
			s.Boxes["node2"].Facts["ram_bytes"] = 16 * gib
			s.Boxes["node2"].Facts["computer_max_mem_bytes"] = 1 << 50
		},
		"no hardware in the release": func(in *Inputs, s *state.State) {
			b := in.Boxes["node2"]
			b.MemoryGiB, b.Threads, b.PoolGiB = 0, 0, 0
			in.Boxes["node2"] = b
			s.Boxes["node2"].Facts["computer_max_mem_bytes"] = 1 << 50
			s.Boxes["node2"].Facts["computer_max_threads"] = 1024
			s.Boxes["node2"].Facts["computer_max_disk_bytes"] = 1 << 60
		},
	} {
		in, s := compute()
		// ann's own node2: 16G, 4 threads, 500G by its report
		c := computer(idA, "ann", 2048, 1, 10, "own")
		switch name {
		case "memory", "memory, by its heartbeat":
			c.Memory = 32 * 1024
		case "threads":
			c.Threads = 8
		case "disk":
			c.Disk = 600
		}
		in.Computers = []*computers.Listed{c}
		lie(&in, s)
		if h := holder(Decide(in, s, 90_000, nil), idA); h != "" {
			t.Fatalf("%s: placed on %q past its hardware", name, h)
		}
	}
}

// what a move carries is what its last box said it kept, less what the
// candidate keeps: a third box's word moves nothing
func TestOnlyTheHolderAndTheCandidateSayWhatAMoveCarries(t *testing.T) {
	in, s := compute()
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 1, 10, "any")}
	s.Boxes["gone"].Facts["runs/"+ComputerService(idA)] = 1
	s.Boxes["node2"].Facts["computer/"+idA+"/used_bytes"] = 39 * gib
	// pi, which cannot take it, says it keeps 40G of it
	s.Boxes["pi"].Facts["computer/"+idA+"/used_bytes"] = 40 * gib
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "node1" {
		t.Fatalf("on %q: a third box's word steered it", h)
	}
	// nor a box not of the release that says it ran it
	s.Boxes["stranger"] = &state.BoxState{LastSeen: 0, Facts: map[string]float64{"runs/" + ComputerService(idA): 1, "computer/" + idA + "/used_bytes": 40 * gib}}
	if h := holder(Decide(in, s, 90_000, nil), idA); h != "node1" {
		t.Fatalf("on %q: a stranger's word steered it", h)
	}
}

// ann's box by enlistment, and bea's and a stranger's beside it, all saying
// they have the room; the release's node1 as ever
func computeOwned() (Inputs, *state.State, map[string]string) {
	in, s := compute()
	in.Joined = &JoinedRules{Attested: true}
	s.Joined = map[string]*state.Joined{}
	for _, name := range []string{"jann", "jbea", "jstranger"} {
		s.Joined[name] = &state.Joined{Key: name}
		s.Boxes[name] = &state.BoxState{LastSeen: s.Time, Facts: map[string]float64{
			"kvm": 1, "x86_64": 1, "can_live": 1, "can/" + ComputersService: 1,
			"computer_max_mem_bytes": 16 * gib, "computer_max_threads": 8, "computer_max_disk_bytes": 200 * gib,
		}}
	}
	return in, s, map[string]string{"jann": "ann", "jbea": "bea"}
}

func TestAMembersComputerGoesOnTheirOwnBox(t *testing.T) {
	for _, consent := range []string{"own", "any"} {
		in, s, own := computeOwned()
		in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 2, 20, consent)}
		if h := holder(Decide(in, s, 90_000, own), idA); h != "jann" {
			t.Fatalf("%s: on %q, not ann's own box", consent, h)
		}
	}
}

func TestAMemberWithoutABoxIsHostedAsBefore(t *testing.T) {
	in, s, own := computeOwned()
	in.Computers = []*computers.Listed{computer(idA, "cat", 2048, 2, 20, "any")}
	if h := holder(Decide(in, s, 90_000, own), idA); h != "node1" {
		t.Fatalf("on %q", h)
	}
	in.Computers = []*computers.Listed{computer(idA, "cat", 2048, 2, 20, "own")}
	if h := holder(Decide(in, s, 90_000, own), idA); h != "" {
		t.Fatalf("own, and no box of theirs: on %q", h)
	}
}

func TestAMembersComputerStaysWhereItRunsOnAListedBox(t *testing.T) {
	in, s, own := computeOwned()
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 2, 20, "any")}
	s.Boxes["node1"].Facts["runs/"+ComputerService(idA)] = 1
	if h := holder(Decide(in, s, 90_000, own), idA); h != "node1" {
		t.Fatalf("its disk is on node1, and it went to %q", h)
	}
	// and with its host gone, their own box takes it
	s.Boxes["node1"].LastSeen = 0
	if h := holder(Decide(in, s, 90_000, own), idA); h != "jann" {
		t.Fatalf("on %q", h)
	}
}

func TestAnothersBoxNeverHostsAComputerWhateverItSays(t *testing.T) {
	in, s, own := computeOwned()
	in.Computers = []*computers.Listed{computer(idA, "ann", 2048, 2, 20, "any")}
	for _, b := range []string{"jbea", "jstranger"} {
		s.Boxes[b].Facts["runs/"+ComputerService(idA)] = 1
		s.Boxes[b].Facts["computer_max_mem_bytes"] = 1000 * gib
	}
	if h := holder(Decide(in, s, 90_000, own), idA); h != "jann" {
		t.Fatalf("on %q", h)
	}
	// ann's own box too, once it is not hers: at once, whatever runs there
	s.Boxes["jann"].Facts["runs/"+ComputerService(idA)] = 1
	if h := holder(Decide(in, s, 90_000, map[string]string{"jbea": "bea"}), idA); h != "node1" {
		t.Fatalf("not hers, and it stayed on %q", h)
	}
	// and with no switch, as before
	in.Joined = &JoinedRules{}
	if h := holder(Decide(in, s, 90_000, own), idA); h != "node1" {
		t.Fatalf("not attested: on %q", h)
	}
}
