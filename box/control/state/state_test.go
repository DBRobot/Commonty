package state

import (
	"encoding/json"
	"strings"
	"testing"
)

// a signature here is "ok:" + the box's key: enough to test the rules
func fakeVerify(spki string, _ []byte, sig string) bool { return sig == "ok:"+spki }

func cfg() *Config {
	return &Config{
		Boxes: map[string]Box{
			"node1": {House: "a", Key: "k1"},
			"node2": {House: "a", Key: "k2"},
			"node3": {House: "b", Key: "k3"},
		},
		Rules: Rules{
			LeaseTTL:  15000,
			GoneAfter: 90000,
			Leases: map[string]LeaseRule{
				"postgres": {Eligible: []string{"node1", "node2"}},
				"anyone":   {},
			},
		},
	}
}

type fleet struct {
	t   *testing.T
	c   *Config
	s   *State
	seq map[string]uint64
}

func newFleet(t *testing.T) *fleet {
	return &fleet{t: t, c: cfg(), s: New(), seq: map[string]uint64{}}
}

func (f *fleet) req(box, kind string, body any) Request {
	f.seq[box]++
	b, _ := json.Marshal(body)
	return Request{Kind: kind, Box: box, Seq: f.seq[box], Body: b, Sig: "ok:" + f.c.Boxes[box].Key}
}

func (f *fleet) block(now int64, reqs ...Request) int {
	return f.s.Block(f.c, fakeVerify, reqs, now)
}

func (f *fleet) beat(box string, now int64) {
	if f.block(now, f.req(box, "heartbeat", Heartbeat{Facts: map[string]float64{"cores": 8}})) != 1 {
		f.t.Fatalf("%s heartbeat refused", box)
	}
}

func lease(role, op string, epoch uint64) LeaseOp { return LeaseOp{Role: role, Op: op, Epoch: epoch} }

func TestHeartbeatsAreTheBoxsOwnAndInOrder(t *testing.T) {
	f := newFleet(t)
	f.beat("node1", 1000)
	if !f.s.Alive(f.c, "node1", 1000) || f.s.Alive(f.c, "node2", 1000) {
		t.Fatal("alive should be node1 alone")
	}
	forged := f.req("node1", "heartbeat", Heartbeat{})
	forged.Sig = "ok:k2" // node2's signature on node1's heartbeat
	if f.block(2000, forged) != 0 {
		t.Fatal("a heartbeat signed by another box was taken")
	}
	r := f.req("node1", "heartbeat", Heartbeat{})
	if f.block(3000, r) != 1 || f.block(4000, r) != 0 {
		t.Fatal("a heartbeat replayed was taken twice")
	}
	stranger := Request{Kind: "heartbeat", Box: "evil", Seq: 1, Body: []byte(`{}`), Sig: "ok:"}
	if f.block(5000, stranger) != 0 {
		t.Fatal("a box not in the fleet was heard")
	}
	if f.s.Alive(f.c, "node1", 3000+90001) {
		t.Fatal("a box unheard past GoneAfter is still alive")
	}
}

func TestOneWriterAtATimeWithRisingEpochs(t *testing.T) {
	f := newFleet(t)
	f.beat("node1", 1000)
	f.beat("node2", 1000)
	if f.block(2000, f.req("node1", "lease", lease("postgres", "acquire", 0))) != 1 {
		t.Fatal("node1 could not take a free lease")
	}
	l := f.s.Leases["postgres"]
	if l.Holder != "node1" || l.Epoch != 1 || l.Expires != 17000 {
		t.Fatalf("lease %+v", l)
	}
	if f.block(3000, f.req("node2", "lease", lease("postgres", "acquire", 0))) != 0 {
		t.Fatal("node2 took a lease node1 holds")
	}
	if f.block(10000, f.req("node1", "lease", lease("postgres", "renew", 1))) != 1 || f.s.Leases["postgres"].Expires != 25000 {
		t.Fatal("the holder could not renew")
	}
	if f.block(11000, f.req("node2", "lease", lease("postgres", "renew", 1))) != 0 {
		t.Fatal("a box renewed a lease it does not hold")
	}
	// node1 goes quiet: the lease runs out, and the next holder's epoch is higher
	f.beat("node2", 20000)
	f.beat("node2", 26000)
	if _, held := f.s.Leases["postgres"]; held {
		t.Fatal("an expired lease is still held")
	}
	if f.block(27000, f.req("node2", "lease", lease("postgres", "acquire", 0))) != 1 || f.s.Leases["postgres"].Epoch != 2 {
		t.Fatalf("node2 should hold postgres at epoch 2, have %+v", f.s.Leases["postgres"])
	}
	// node1 wakes up with its old epoch: nothing it asks about epoch 1 counts
	if f.block(28000, f.req("node1", "lease", lease("postgres", "renew", 1))) != 0 {
		t.Fatal("a stale holder renewed")
	}
	if f.block(29000, f.req("node2", "lease", lease("postgres", "release", 2))) != 1 {
		t.Fatal("the holder could not let go")
	}
	f.beat("node1", 29500)
	if f.block(30000, f.req("node1", "lease", lease("postgres", "acquire", 0))) != 1 || f.s.Leases["postgres"].Epoch != 3 {
		t.Fatal("after a release the next epoch is not higher")
	}
}

func TestOnlyEligibleAndAliveBoxesHoldLeases(t *testing.T) {
	f := newFleet(t)
	f.beat("node3", 1000)
	if f.block(2000, f.req("node3", "lease", lease("postgres", "acquire", 0))) != 0 {
		t.Fatal("a box the rules do not allow took the lease")
	}
	if f.block(2000, f.req("node3", "lease", lease("anyone", "acquire", 0))) != 1 {
		t.Fatal("a lease open to any box refused a live one")
	}
	if f.block(3000, f.req("node1", "lease", lease("postgres", "acquire", 0))) != 0 {
		t.Fatal("a box never heard from took a lease")
	}
	if f.block(3000, f.req("node1", "lease", lease("nosuch", "acquire", 0))) != 0 {
		t.Fatal("a lease that does not exist was granted")
	}
}

func TestTheAgreedClockOnlyMovesForward(t *testing.T) {
	f := newFleet(t)
	f.beat("node1", 5000)
	f.block(4000)
	if f.s.Time != 5000 {
		t.Fatalf("time went back to %d", f.s.Time)
	}
}

func TestEveryValidatorReachesTheSameState(t *testing.T) {
	run := func() string {
		f := newFleet(t)
		f.beat("node1", 1000)
		f.beat("node2", 1000)
		f.block(2000, f.req("node1", "lease", lease("postgres", "acquire", 0)), f.req("node2", "lease", lease("postgres", "acquire", 0)))
		b, _ := json.Marshal(f.s)
		return string(b)
	}
	if a, b := run(), run(); a != b {
		t.Fatalf("two runs differ:\n%s\n%s", a, b)
	}
}

func TestTheDirectoryLogKeepsOrderAndBoundsEachBox(t *testing.T) {
	f := newFleet(t)
	item := map[string]any{"entry": map[string]any{"name": "ann"}}
	if got := f.block(1_000, f.req("node1", "directory", item), f.req("node2", "directory", item)); got != 2 {
		t.Fatalf("applied %d of 2", got)
	}
	d := f.s.Directory
	if len(d) != 2 || d[0].Box != "node1" || d[1].Box != "node2" || d[0].Height != f.s.Height || d[0].Time != 1_000 {
		t.Fatalf("log: %+v", d)
	}
	// too big, or not json: never in the log
	big := f.req("node1", "directory", strings.Repeat("x", MaxDirItem))
	bad := f.req("node1", "directory", nil)
	bad.Body = []byte("{nope")
	if got := f.block(2_000, big, bad); got != 0 {
		t.Fatalf("applied %d bad items", got)
	}
	// a box sends at most DirPerMinute a minute; the next minute it may again
	var many []Request
	for i := 0; i < DirPerMinute+5; i++ {
		many = append(many, f.req("node1", "directory", item))
	}
	if got := f.block(3_000, many...); got != DirPerMinute-1 {
		t.Fatalf("applied %d in the minute that had one already; want %d", got, DirPerMinute-1)
	}
	if got := f.block(62_000, f.req("node1", "directory", item)); got != 1 {
		t.Fatal("a new minute takes items again")
	}
}
