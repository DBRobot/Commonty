package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"commonty/control/place"
	"commonty/control/state"
	"commonty/control/state/computers"
)

func newPlacer(t *testing.T) (*placer, *[]string) {
	calls := []string{}
	seq := uint64(100)
	p := &placer{
		leased: map[string]bool{},
		since:  map[string]int64{},
		sent:   map[string]map[uint64]int64{},
		uptime: func() int64 { return 0 },
		ttl:    15 * time.Second,
		ask: func(op state.LeaseOp) (uint64, error) {
			calls = append(calls, "lease "+op.Op)
			seq++
			return seq, nil
		},
		box:     "node1",
		dir:     filepath.Join(t.TempDir(), "dd-placed"),
		carries: map[string][]string{"web": {"web.service"}, "photos": {"ente.service", "ente-web.service"}},
		run: func(args ...string) error {
			if args[0] == "is-active" {
				return errors.New("inactive") // nothing runs, unless a test says so
			}
			calls = append(calls, strings.Join(args, " "))
			return nil
		},
	}
	return p, &calls
}

func TestReconcileStartsAndStops(t *testing.T) {
	p, calls := newPlacer(t)
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: "photos"}, {Service: "games"}}}}, state.New())
	if !p.up("photos") || p.up("web") {
		t.Fatalf("flags: photos %v web %v", p.up("photos"), p.up("web"))
	}
	if want := []string{"start --no-block ente.service ente-web.service"}; !reflect.DeepEqual(*calls, want) {
		t.Fatalf("calls %v", *calls)
	}
	// the same plan again does nothing more
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: "photos"}}}}, state.New())
	if len(*calls) != 1 {
		t.Fatalf("calls %v", *calls)
	}
	if f := p.facts(); f["carries/photos"] != 1 || f["runs/photos"] != 1 || f["runs/web"] != 0 || f["carries/games"] != 0 {
		t.Fatalf("facts %v", f)
	}
	// moved away
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}, state.New())
	if p.up("photos") || (*calls)[1] != "stop --no-block ente.service ente-web.service" {
		t.Fatalf("not stopped: %v", *calls)
	}
}

func TestAPlanWithoutThisBoxChangesNothing(t *testing.T) {
	p, calls := newPlacer(t)
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: "web"}}}}, state.New())
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node2": {}}}, state.New())
	if !p.up("web") || len(*calls) != 1 {
		t.Fatalf("a plan this box is not in stopped it: %v", *calls)
	}
}

func TestNoCarriesFileIsNone(t *testing.T) {
	c, err := loadCarries(filepath.Join(t.TempDir(), "none.json"))
	if err != nil || len(c) != 0 {
		t.Fatalf("%v %v", c, err)
	}
	path := filepath.Join(t.TempDir(), "c.json")
	_ = os.WriteFile(path, []byte(`{"web":["web.service"]}`), 0o644)
	if c, err := loadCarries(path); err != nil || c["web"][0] != "web.service" {
		t.Fatalf("%v %v", c, err)
	}
}

// a writer: node1 is given "db" by the plan; the agreed state says who
// holds svc/db, and a test clock says how long since boot
func leasedPlacer(t *testing.T) (*placer, *[]string, *int64, *state.State) {
	p, calls := newPlacer(t)
	p.carries = map[string][]string{"db": {"postgresql.service"}}
	p.leased = map[string]bool{"db": true}
	clock := int64(1_000_000)
	p.uptime = func() int64 { return clock }
	s := state.New()
	s.Time = 5_000_000 // the agreed clock: nothing to do with this box's
	return p, calls, &clock, s
}

var wantDB = place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: "db"}}}}

// the lease as the agreed state has it after the last request this box sent
func grant(p *placer, s *state.State, epoch uint64) {
	last := uint64(0)
	for q := range p.sent["db"] {
		last = max(last, q)
	}
	s.Leases["svc/db"] = &state.Lease{Holder: "node1", Epoch: epoch, Expires: s.Time + 15_000, Seq: last}
}

func TestAWriterRunsOnlyWithItsLease(t *testing.T) {
	p, calls, clock, s := leasedPlacer(t)
	p.reconcile(wantDB, s)
	if p.up("db") || !reflect.DeepEqual(*calls, []string{"lease acquire"}) {
		t.Fatalf("started without its lease: %v", *calls)
	}
	// granted for the acquire sent at 1_000_000: not at once, since a
	// holder before may still be stopping
	grant(p, s, 4)
	*clock += 1000
	p.reconcile(wantDB, s)
	if p.up("db") {
		t.Fatal("started the moment the lease was granted")
	}
	*clock += leaseMargin.Milliseconds()
	p.reconcile(wantDB, s)
	// until: from when the request that set the lease was sent, not from
	// any wall clock
	if h := p.holding("db"); !p.up("db") || h.Epoch != 4 || h.Until != 1_000_000+15_000-3_000 {
		t.Fatalf("flag %v %+v", p.up("db"), h)
	}
	if (*calls)[len(*calls)-1] != "start --no-block postgresql.service" {
		t.Fatalf("calls %v", *calls)
	}
	// a renewal agreed moves when it stops, by when it was sent
	sentAt := *clock
	p.reconcile(wantDB, s) // sends a renewal at sentAt
	grant(p, s, 4)
	*clock += 2000
	p.reconcile(wantDB, s)
	if h := p.holding("db"); h.Until != sentAt+12_000 {
		t.Fatalf("not renewed: %+v want %d", h, sentAt+12_000)
	}
}

// running db at epoch 1, its lease set by a request sent at the clock now
func running(t *testing.T) (*placer, *[]string, *int64, *state.State) {
	p, calls, clock, s := leasedPlacer(t)
	p.reconcile(wantDB, s)
	grant(p, s, 1)
	p.since["db"] = *clock - 60_000
	p.reconcile(wantDB, s)
	if !p.up("db") {
		t.Fatal("not started")
	}
	return p, calls, clock, s
}

func TestAWriterCutOffStopsBeforeItsLeaseLapses(t *testing.T) {
	p, calls, clock, _ := running(t)
	// no agreement since: nothing reconciles, only the fence
	*clock += 11_000
	p.fence()
	if !p.up("db") {
		t.Fatal("stopped with time left")
	}
	*clock += 1000
	p.fence()
	if p.up("db") || (*calls)[len(*calls)-1] != "stop postgresql.service" {
		t.Fatalf("still running at its margin: %v", *calls)
	}
}

func TestAWriterStopsWhenTheLeaseIsAnothers(t *testing.T) {
	p, calls, _, s := running(t)
	s.Leases["svc/db"] = &state.Lease{Holder: "node2", Epoch: 2, Expires: s.Time + 15_000}
	p.reconcile(wantDB, s)
	if p.up("db") || (*calls)[len(*calls)-2] != "stop postgresql.service" || (*calls)[len(*calls)-1] != "lease acquire" {
		t.Fatalf("calls %v", *calls)
	}
}

func TestAWriterMovedStopsThenGivesItsLeaseBack(t *testing.T) {
	p, calls, _, s := running(t)
	n := len(*calls)
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}, s)
	if got := (*calls)[n:]; !reflect.DeepEqual(got, []string{"stop postgresql.service", "lease release"}) {
		t.Fatalf("calls %v", got)
	}
}

func TestARestartKeepsAWriterThatHasTime(t *testing.T) {
	p, _, _, s := running(t)
	// dd-control again, its memory gone: the flag says how long, and a
	// lease set by a request it has no record of changes nothing
	q, _ := newPlacer(t)
	q.dir, q.carries, q.leased, q.uptime = p.dir, p.carries, p.leased, p.uptime
	q.fence()
	before := q.holding("db")
	q.reconcile(wantDB, s)
	if !q.up("db") || q.holding("db") != before {
		t.Fatalf("a restart changed a writer whose lease had time left: %+v", q.holding("db"))
	}
}

func TestALeaseSetByARequestNotOnRecordStartsNothing(t *testing.T) {
	p, _, clock, s := leasedPlacer(t)
	s.Leases["svc/db"] = &state.Lease{Holder: "node1", Epoch: 1, Expires: s.Time + 15_000, Seq: 7}
	p.since["db"] = *clock - 60_000
	p.reconcile(wantDB, s)
	if p.up("db") {
		t.Fatal("started on a lease whose renewal it cannot time")
	}
}

func TestAWriterUnderAnOlderLeaseStartsAgain(t *testing.T) {
	p, calls, _, s := running(t)
	grant(p, s, 2)
	p.reconcile(wantDB, s)
	if p.up("db") || (*calls)[len(*calls)-1] != "stop postgresql.service" {
		t.Fatalf("a writer kept running under epoch 1 with the lease at 2: %v", *calls)
	}
}

func TestAServiceThatWouldNotFitIsRefusedAndTold(t *testing.T) {
	p, calls := newPlacer(t)
	p.memory, p.refused = map[string]float64{"web": 2}, map[string]int64{}
	free := float64(1 << 30)
	p.free = func() float64 { return free }
	plan := place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: "web"}}}}
	p.reconcile(plan, state.New())
	if p.up("web") || len(*calls) != 0 || p.facts()["refused/web"] != 1 {
		t.Fatalf("started with 1 GiB free for 2: %v %v", *calls, p.facts())
	}
	// what another box measured it holding counts too
	free = 4 << 30
	s := state.New()
	s.Boxes["node2"] = &state.BoxState{Facts: map[string]float64{"svc/web/mem": 6 << 30}}
	p.reconcile(plan, s)
	if p.up("web") {
		t.Fatal("started with 4 GiB free for what holds 6")
	}
	free = 8 << 30
	p.reconcile(plan, s)
	if !p.up("web") || p.facts()["refused/web"] != 0 {
		t.Fatalf("not started with room: %v", p.facts())
	}
}

func TestAWriterThatWouldNotFitAsksForNoLease(t *testing.T) {
	p, calls, _, s := leasedPlacer(t)
	p.memory, p.refused = map[string]float64{"db": 2}, map[string]int64{}
	p.free = func() float64 { return 1 << 30 }
	p.reconcile(wantDB, s)
	if len(*calls) != 0 {
		t.Fatalf("asked for a lease it could not use: %v", *calls)
	}
}

func TestThePlansCopiesAreWritten(t *testing.T) {
	p, _ := newPlacer(t)
	p.copies = filepath.Join(t.TempDir(), "copies.json")
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {}}, Copies: map[string][]string{"ente": {"node1", "node2"}}}, state.New())
	raw, err := os.ReadFile(p.copies)
	if err != nil || string(raw) != `{"ente":["node1","node2"]}` {
		t.Fatalf("%s %v", raw, err)
	}
}

// a placer with these computers agreed (by id; up unless said), within the
// policy's limits
func computerPlacer(t *testing.T, cs ...*computers.Listed) (*placer, *[]string, *int64, *state.State) {
	p, calls, clock, s := leasedPlacer(t)
	p.limits = &computers.Limits{Memory: 16384, Threads: 8, Disk: 200, PortBase: 27200, PortCount: 200}
	agreed := map[string]*computers.Listed{}
	for _, c := range cs {
		agreed["computer-"+c.ID] = c
	}
	p.agreed = func(*state.State) (map[string]*computers.Listed, error) { return agreed, nil }
	return p, calls, clock, s
}

func comp(id, owner string, mem, threads, port uint64) *computers.Listed {
	return &computers.Listed{ID: id, Owner: owner, Memory: mem, Threads: threads, Disk: 32, Port: port, Up: true}
}

func TestAComputerRunsUnderItsLeaseAndIsFenced(t *testing.T) {
	p, calls, clock, s := computerPlacer(t, comp("0123456789abcdef", "ann", 4096, 2, 27200))
	a, b := "computer-0123456789abcdef", "computer-fedcba9876543210"
	unit := "dd-computer@0123456789abcdef.service"
	plan := place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: a}, {Service: "computer-../x"}}}}
	p.reconcile(plan, s)
	if p.up(a) || !reflect.DeepEqual(*calls, []string{"lease acquire"}) {
		t.Fatalf("started without its lease: %v", *calls)
	}
	last := uint64(0)
	for q := range p.sent[a] {
		last = max(last, q)
	}
	s.Leases["svc/"+a] = &state.Lease{Holder: "node1", Epoch: 2, Expires: s.Time + 15_000, Seq: last}
	// not at once: a holder before may still be stopping
	p.reconcile(plan, s)
	*clock += leaseMargin.Milliseconds()
	p.reconcile(plan, s)
	if h := p.holding(a); !p.up(a) || h.Epoch != 2 || (*calls)[len(*calls)-1] != "start --no-block "+unit {
		t.Fatalf("flag %v %+v, calls %v", p.up(a), h, *calls)
	}
	if f := p.facts(); f["runs/"+a] != 1 || f["runs/"+b] != 0 {
		t.Fatalf("facts %v", f)
	}
	// cut off: the fence stops it, waiting for the stop, before the lease may lapse
	*clock += 15_000
	p.fence()
	if p.up(a) || (*calls)[len(*calls)-1] != "stop "+unit {
		t.Fatalf("still running past its lease: %v", *calls)
	}
}

func TestAComputerTheLeaseOfIsGivenBackWhenThePlanMovesIt(t *testing.T) {
	p, calls, _, s := computerPlacer(t, comp("0123456789abcdef", "ann", 4096, 2, 27200))
	a := "computer-0123456789abcdef"
	s.Leases["svc/"+a] = &state.Lease{Holder: "node1", Epoch: 1, Expires: s.Time + 15_000}
	// no flag and not in the plan: only its lease is this box's, and goes back
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}, s)
	if !reflect.DeepEqual(*calls, []string{"lease release"}) {
		t.Fatalf("calls %v", *calls)
	}
}

// planned here, but not agreed up, unread, or past what its owner may have
// here: no lease asked for
func TestAComputerIsAskedForOnlyAgreedUpAndWithinItsLimits(t *testing.T) {
	a, b := comp("0123456789abcdef", "ann", 12288, 4, 27200), comp("fedcba9876543210", "ann", 8192, 2, 27201)
	plan := place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: "computer-" + a.ID}, {Service: "computer-" + b.ID}}}}
	asked := func(p *placer) []string {
		out := []string{}
		for svc, sent := range p.sent {
			if len(sent) > 0 {
				out = append(out, svc)
			}
		}
		return out
	}
	// ann's two together are 20 GiB, over her 16: the first by id alone
	p, _, _, s := computerPlacer(t, a, b)
	p.reconcile(plan, s)
	if got := asked(p); !reflect.DeepEqual(got, []string{"computer-" + a.ID}) {
		t.Fatalf("asked for %v", got)
	}
	// not up
	down := *a
	down.Up = false
	p, _, _, s = computerPlacer(t, &down)
	p.reconcile(plan, s)
	if got := asked(p); len(got) != 0 {
		t.Fatalf("asked for %v, not up", got)
	}
	// a port outside the pool
	p, _, _, s = computerPlacer(t, comp("0123456789abcdef", "ann", 1024, 1, 22))
	p.reconcile(plan, s)
	if got := asked(p); len(got) != 0 {
		t.Fatalf("asked for %v, its port not the pool's", got)
	}
	// the agreed computers unread, or no limits in the release
	p, _, _, s = computerPlacer(t, a)
	p.agreed = func(*state.State) (map[string]*computers.Listed, error) { return nil, os.ErrNotExist }
	p.reconcile(plan, s)
	if got := asked(p); len(got) != 0 {
		t.Fatalf("asked for %v, nothing agreed read", got)
	}
	p, _, _, s = computerPlacer(t, a)
	p.limits = nil
	p.reconcile(plan, s)
	if got := asked(p); len(got) != 0 {
		t.Fatalf("asked for %v, no limits", got)
	}
}

// a computer running here that is powered off: stopped, waited for, then
// its lease given back
func TestAComputerPoweredOffStopsBeforeItsLeaseGoesBack(t *testing.T) {
	a := comp("0123456789abcdef", "ann", 4096, 2, 27200)
	p, calls, _, s := computerPlacer(t, a)
	svc := "computer-" + a.ID
	s.Leases["svc/"+svc] = &state.Lease{Holder: "node1", Epoch: 3, Expires: s.Time + 15_000}
	if err := p.raise(svc, []byte(`{"epoch":3,"until":99999999}`)); err != nil {
		t.Fatal(err)
	}
	a.Up = false
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: svc}}}}, s)
	want := []string{"stop dd-computer@" + a.ID + ".service", "lease release"}
	if p.up(svc) || !reflect.DeepEqual(*calls, want) {
		t.Fatalf("flag %v, calls %v", p.up(svc), *calls)
	}
}

// one computer and its lease from one state: whose box holds it by the release
func TestAComputersLeaseIsAnsweredFromOneState(t *testing.T) {
	s := state.New()
	s.Height, s.Time = 7, 5000
	s.Leases["svc/computer-0123456789abcdef"] = &state.Lease{Holder: "node1", Epoch: 2, Expires: 9000}
	cfg := &state.Config{Rules: state.Rules{ComputerHosts: map[string]string{"node1": "ann"}}}
	got := computerLease(s, cfg, "0123456789abcdef", []*computers.Listed{{ID: "0123456789abcdef", Owner: "ann", Up: true, Consent: "own"}})
	b, _ := json.Marshal(got)
	want := `{"computer":{"consent":"own","owner":"ann","up":true},"height":7,"hostOwner":"ann","lease":{"expires":9000,"holder":"node1"},"leaseHeight":7,"time":5000}`
	if string(b) != want {
		t.Fatalf("%s", b)
	}
	if got := computerLease(s, cfg, "fedcba9876543210", nil); got["computer"] != nil || got["lease"] != nil {
		t.Fatalf("%v", got)
	}
}

// a writer stands in for one whose final copy is watched: what it was given,
// and whether its flag was still up then
func withFinal(p *placer, calls *[]string) {
	p.final = func(svc string, within time.Duration) error {
		*calls = append(*calls, fmt.Sprintf("final %s up=%v within=%v", svc, p.up(svc), within))
		return nil
	}
}

func TestAHandoverCopiesOutBeforeTheFlagGoes(t *testing.T) {
	p, calls, clock, s := running(t)
	withFinal(p, calls)
	*clock += 2000
	n := len(*calls)
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}, s)
	want := []string{"stop postgresql.service", "final db up=true within=10s", "stop postgresql.service", "lease release"}
	if got := (*calls)[n:]; !reflect.DeepEqual(got, want) {
		t.Fatalf("calls %v", got)
	}
	if p.up("db") {
		t.Fatal("flag still up")
	}
}

func TestAFencedWriterCopiesNothingOut(t *testing.T) {
	p, calls, clock, _ := running(t)
	withFinal(p, calls)
	*clock += 12_000
	p.fence()
	// nor when the lease is another's
	q, qcalls, _, qs := running(t)
	withFinal(q, qcalls)
	qs.Leases["svc/db"] = &state.Lease{Holder: "node2", Epoch: 2, Expires: qs.Time + 15_000}
	q.reconcile(wantDB, qs)
	for _, c := range append(*calls, *qcalls...) {
		if strings.HasPrefix(c, "final") {
			t.Fatalf("a final copy off the handover: %v %v", *calls, *qcalls)
		}
	}
	if p.up("db") || q.up("db") {
		t.Fatal("not stopped")
	}
}

func TestAFinalCopyThatFailsKeepsTheFlagAndAsksAgain(t *testing.T) {
	p, calls, clock, s := running(t)
	fail := true
	p.final = func(string, time.Duration) error {
		*calls = append(*calls, "final")
		if fail {
			return fmt.Errorf("not done")
		}
		return nil
	}
	moved := place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}
	n := len(*calls)
	p.reconcile(moved, s)
	// what it started is stopped, the flag stays and the lease is renewed
	want := []string{"stop postgresql.service", "final", "stop postgresql.service", "lease renew"}
	if got := (*calls)[n:]; !reflect.DeepEqual(got, want) || !p.up("db") {
		t.Fatalf("calls %v up %v", got, p.up("db"))
	}
	// the renewal agreed, and the next try copies
	grant(p, s, 1)
	*clock += 2000
	fail = false
	n = len(*calls)
	p.reconcile(moved, s)
	if h := p.holding("db"); p.up("db") || h.Epoch != 0 {
		t.Fatalf("flag kept after its copy: %+v", h)
	}
	want = []string{"stop postgresql.service", "final", "stop postgresql.service", "lease release"}
	if got := (*calls)[n:]; !reflect.DeepEqual(got, want) {
		t.Fatalf("calls %v", got)
	}
}

func TestAHandoverThatNeverCopiesGivesUpInTime(t *testing.T) {
	p, calls, clock, s := running(t)
	p.final = func(string, time.Duration) error { return fmt.Errorf("not done") }
	moved := place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}
	p.reconcile(moved, s)
	for i := 0; i < 30 && p.up("db"); i++ {
		*clock += 5000
		grant(p, s, 1)
		p.reconcile(moved, s)
	}
	if p.up("db") || (*calls)[len(*calls)-1] != "lease release" {
		t.Fatalf("still handing over: %v", (*calls)[len(*calls)-3:])
	}
	if *clock-1_000_000 < handoverFor.Milliseconds() {
		t.Fatal("gave up early")
	}
}

func TestAWriterGivenBackMidHandoverStartsAgain(t *testing.T) {
	p, calls, _, s := running(t)
	p.final = func(string, time.Duration) error { return fmt.Errorf("not done") }
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}, s)
	grant(p, s, 1)
	p.reconcile(wantDB, s)
	if !p.up("db") || (*calls)[len(*calls)-1] != "start --no-block postgresql.service" || len(p.handing) != 0 {
		t.Fatalf("calls %v", *calls)
	}
}

// a writer stood in for: whether its flag says it is handed over when the
// final copy runs, and what the units' condition (box/placed.nix) makes of it
func TestAFinalCopyRunsWithTheFlagMarkedHandingOver(t *testing.T) {
	p, calls, _, s := running(t)
	var during held
	p.final = func(svc string, _ time.Duration) error {
		during = p.holding(svc)
		*calls = append(*calls, "final")
		return nil
	}
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}, s)
	if !during.Handing || during.Epoch != 1 {
		t.Fatalf("flag during the copy %+v", during)
	}
	raw, _ := json.Marshal(during)
	if !strings.Contains(string(raw), `"handing":true`) {
		t.Fatalf("the units' condition looks for \"handing\":true, the flag has %s", raw)
	}
}

func TestReleaseOnlyOnceNothingRuns(t *testing.T) {
	p, calls, _, s := running(t)
	active := true
	p.run = func(args ...string) error {
		if args[0] == "is-active" {
			if active {
				return nil // something started during the copy
			}
			return errors.New("inactive")
		}
		*calls = append(*calls, strings.Join(args, " "))
		return nil
	}
	p.final = func(string, time.Duration) error { return nil }
	moved := place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}
	p.reconcile(moved, s)
	for _, c := range *calls {
		if c == "lease release" {
			t.Fatalf("released while a unit runs: %v", *calls)
		}
	}
	if !p.up("db") || !p.holding("db").Handing {
		t.Fatalf("flag %v %+v", p.up("db"), p.holding("db"))
	}
	active = false
	grant(p, s, 1)
	p.reconcile(moved, s)
	if p.up("db") || (*calls)[len(*calls)-1] != "lease release" {
		t.Fatalf("calls %v", *calls)
	}
}

func TestARetriedHandoverKeepsTheFlagHandingOver(t *testing.T) {
	p, calls, clock, s := running(t)
	p.final = func(string, time.Duration) error { return fmt.Errorf("not done") }
	moved := place.Plan{Boxes: map[string][]place.Decision{"node1": {}}}
	n := len(*calls)
	p.reconcile(moved, s)
	for i := 0; i < 3; i++ {
		*clock += 5000
		grant(p, s, 1)
		p.reconcile(moved, s)
		if h := p.holding("db"); !h.Handing {
			t.Fatalf("round %d: the flag lets it start: %+v", i, h)
		}
	}
	for _, c := range (*calls)[n:] {
		if strings.HasPrefix(c, "start") {
			t.Fatalf("started mid-handover: %v", *calls)
		}
	}
}

// a computer has no buckets to copy out, but is handed over in the same
// order: stopped, and its lease given back only once it no longer runs
func TestAComputerHandedOverIsNotCopiedAndStopsFirst(t *testing.T) {
	a := comp("0123456789abcdef", "ann", 4096, 2, 27200)
	p, calls, _, s := computerPlacer(t, a)
	unit := "dd-computer@" + a.ID + ".service"
	running := true
	p.run = func(args ...string) error {
		if args[0] == "is-active" {
			if running {
				return nil
			}
			return errors.New("inactive")
		}
		*calls = append(*calls, strings.Join(args, " "))
		return nil
	}
	p.final = func(svc string, _ time.Duration) error {
		*calls = append(*calls, "final "+svc)
		return nil
	}
	svc := "computer-" + a.ID
	s.Leases["svc/"+svc] = &state.Lease{Holder: "node1", Epoch: 3, Expires: s.Time + 15_000}
	if err := p.raise(svc, []byte(`{"epoch":3,"until":99999999}`)); err != nil {
		t.Fatal(err)
	}
	a.Up = false
	plan := place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: svc}}}}
	p.reconcile(plan, s)
	if !p.up(svc) || p.holding(svc).Handing {
		t.Fatalf("flag %v %+v", p.up(svc), p.holding(svc))
	}
	for _, c := range *calls {
		if c == "lease release" || strings.HasPrefix(c, "final") {
			t.Fatalf("calls %v", *calls)
		}
	}
	running = false
	p.reconcile(plan, s)
	if p.up(svc) || (*calls)[len(*calls)-1] != "lease release" || (*calls)[len(*calls)-2] != "stop "+unit {
		t.Fatalf("calls %v", *calls)
	}
}
