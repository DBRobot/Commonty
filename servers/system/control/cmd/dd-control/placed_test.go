package main

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"commonty/control/place"
	"commonty/control/state"
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
