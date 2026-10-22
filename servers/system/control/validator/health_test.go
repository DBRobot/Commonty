package validator

import (
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"commonty/control/state"
	"commonty/control/wire"
)

func TestDiagnose(t *testing.T) {
	t0 := time.Unix(1_000_000, 0)
	l := DefaultLimits
	at := func(d time.Duration) time.Time { return t0.Add(d) }
	// a peer heard, as far as this one: restarting loses nothing
	base := Health{Started: t0, Height: 10, Progress: at(10 * time.Second), PeerHeight: 10, PeersHeard: 1}
	alone := Who{Quorum: 3, Of: 1}
	cases := []struct {
		name   string
		h      func(Health) Health
		now    time.Duration
		healed time.Time
		who    Who
		want   string // a part of the reason; "" for none
	}{
		{"healthy and idle", func(h Health) Health { return h }, 10 * time.Minute, time.Time{}, alone, ""},
		{"in its grace after a start", func(h Health) Health { h.Calling = t0; return h }, 50 * time.Second, time.Time{}, alone, ""},
		{"a call into consensus hangs", func(h Health) Health { h.Calling = at(70 * time.Second); return h }, 100 * time.Second, time.Time{}, alone, "a call into consensus has not returned for 30s"},
		{"a call into consensus is slow, not hung", func(h Health) Health { h.Calling = at(80 * time.Second); return h }, 100 * time.Second, time.Time{}, alone, ""},
		{"a delivery hangs", func(h Health) Health { h.Delivering = at(60 * time.Second); return h }, 100 * time.Second, time.Time{}, alone, "delivering decision 11"},
		{"hung, but no peer heard: maybe the only one up to date", func(h Health) Health { h.Delivering, h.PeersHeard, h.PeerHeight = at(60*time.Second), 0, 0; return h }, 100 * time.Second, time.Time{}, alone, ""},
		{"hung, but every peer heard is behind it", func(h Health) Health { h.Delivering, h.PeerHeight = at(60*time.Second), 9; return h }, 100 * time.Second, time.Time{}, alone, ""},
		{"peers ahead, and still here", func(h Health) Health { h.PeerHeight = 11; return h }, 71 * time.Second, time.Time{}, alone, "peers are at height 11"},
		{"peers ahead, but it just moved", func(h Health) Health { h.PeerHeight, h.Progress = 11, at(30*time.Second); return h }, 71 * time.Second, time.Time{}, alone, ""},
		{"peers level", func(h Health) Health { h.PeersHeard = 2; return h }, 71 * time.Second, time.Time{}, alone, ""},
		{"stalled with requests waiting and peers answering", func(h Health) Health { h.Waiting, h.PeersHeard = at(20*time.Second), 2; return h }, 140 * time.Second, time.Time{}, alone, "requests are waiting"},
		{"stalled, but too few answer: the fleet is down, not this", func(h Health) Health { h.Waiting = at(20 * time.Second); return h }, 10 * time.Minute, time.Time{}, alone, ""},
		{"stalled, no requests waiting", func(h Health) Health { h.PeersHeard = 3; return h }, 10 * time.Minute, time.Time{}, alone, ""},
		// every sign is held back after a self-heal, not only the stall
		{"hung, healed a moment ago", func(h Health) Health { h.Calling = at(70 * time.Second); return h }, 100 * time.Second, at(-5 * time.Minute), alone, ""},
		{"peers ahead, healed a moment ago", func(h Health) Health { h.PeerHeight = 11; return h }, 71 * time.Second, at(-5 * time.Minute), alone, ""},
		{"stalled, healed a moment ago", func(h Health) Health { h.Waiting, h.PeersHeard = at(20*time.Second), 2; return h }, 140 * time.Second, at(-5 * time.Minute), alone, ""},
		{"hung, healed long ago", func(h Health) Health { h.Calling = at(70 * time.Second); return h }, 100 * time.Second, at(-11 * time.Minute), alone, "has not returned"},
		// turns: at 100 s, the slot of 30 s is number 3 since the epoch's
		// 1_000_000 s (33336, and 33336 % 4 == 0); turn 0's, not turn 1's
		{"hung, in its own turn", func(h Health) Health { h.Calling = at(60 * time.Second); return h }, 100 * time.Second, time.Time{}, Who{Quorum: 3, Turn: int((1_000_100 / 30) % 4), Of: 4}, "has not returned"},
		{"hung, in another's turn", func(h Health) Health { h.Calling = at(60 * time.Second); return h }, 100 * time.Second, time.Time{}, Who{Quorum: 3, Turn: int((1_000_100/30)+1) % 4, Of: 4}, ""},
	}
	for _, c := range cases {
		got := Diagnose(c.h(base), at(c.now), c.healed, c.who, l)
		if (c.want == "") != (got == "") || !strings.Contains(got, c.want) {
			t.Errorf("%s: got %q, want %q", c.name, got, c.want)
		}
	}
	// over a whole round of turns each validator gets exactly one, and no
	// two the same
	h := base
	h.Calling = t0
	for s := 0; s < 4; s++ {
		now := time.Unix(((1_000_200/30)+int64(s))*30, 0)
		n := 0
		for turn := range 4 {
			if Diagnose(h, now, time.Time{}, Who{Quorum: 3, Turn: turn, Of: 4}, l) != "" {
				n++
			}
		}
		if n != 1 {
			t.Errorf("slot %d: %d validators may restart", s, n)
		}
	}
}

// Requests nobody of the fleet signed, joins (signed by whatever key they
// bring) and replays of what was agreed never count as waiting: whoever
// reaches the relay can't make a validator look stalled.
func TestOnlyFreshSignedRequestsWait(t *testing.T) {
	c := newCluster(t)
	old := c.box.req("heartbeat", state.Heartbeat{})
	for _, v := range c.vals {
		v.Receive(100, wire.Submit, old)
	}
	c.until("the box's heartbeat agreed", func() bool {
		b := c.vals[1].State().Boxes["node1"]
		return b != nil && b.Seq >= c.box.seq-1
	})
	c.vals[1].mu.Lock()
	saved := c.vals[1].saved
	c.vals[1].mu.Unlock()
	v, err := New(Options{Config: c.cfg, ID: 4, Key: ed25519.NewKeyFromSeed(make([]byte, 32)), Transport: sendFunc(func(uint64, byte, []byte) {}),
		Logger: quiet{}, WAL: memWAL{}, Saved: &saved})
	if err != nil {
		t.Fatal(err)
	}
	defer v.Stop()
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	stranger := &boxKey{name: "node1", k: k, seq: 1 << 40}
	var forged state.Request
	_ = json.Unmarshal(c.box.req("heartbeat", state.Heartbeat{}), &forged)
	forged.Seq++ // the signature no longer covers it
	fb, _ := json.Marshal(forged)
	join := stranger.req("join", state.JoinReq{Key: "x"})
	for i, junk := range [][]byte{[]byte("junk"), []byte(`{"kind":"heartbeat","box":"node1","seq":99999999}`), stranger.req("heartbeat", state.Heartbeat{}), fb, join, old} {
		for _, kind := range []byte{wire.Submit, wire.Forward} {
			v.Receive(7, kind, junk)
			if w := v.Health().Waiting; !w.IsZero() {
				t.Fatalf("frame %d (kind %d) counted as a waiting request", i, kind)
			}
		}
	}
	v.Receive(100, wire.Submit, c.box.req("heartbeat", state.Heartbeat{}))
	if v.Health().Waiting.IsZero() {
		t.Fatal("a fresh request signed by a box of the fleet did not count")
	}
}

// cluster: four validators over a relay in memory; frames of the kinds in
// deaf[id] are not delivered to id, and a validator's Persist waits while
// wedge[id] is set
type cluster struct {
	t     *testing.T
	cfg   *state.Config
	r     *relay
	vals  map[uint64]*Validator
	box   *boxKey
	mu    sync.Mutex
	deaf  map[uint64]bool
	wedge map[uint64]chan struct{}
}

func newCluster(t *testing.T) *cluster {
	cfg := &state.Config{Boxes: map[string]state.Box{}, Rules: state.Rules{LeaseTTL: 15000, GoneAfter: 90000}}
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	der, _ := x509.MarshalPKIXPublicKey(&k.PublicKey)
	cfg.Boxes["node1"] = state.Box{ID: 100, House: "h1", Key: base64.StdEncoding.EncodeToString(der)}
	c := &cluster{t: t, cfg: cfg, box: &boxKey{name: "node1", k: k}, vals: map[uint64]*Validator{},
		r: &relay{in: map[uint64]chan [3]any{}, down: map[uint64]bool{}}, deaf: map[uint64]bool{}, wedge: map[uint64]chan struct{}{}}
	keys := map[uint64]ed25519.PrivateKey{}
	for id := uint64(1); id <= 4; id++ {
		pub, priv, _ := ed25519.GenerateKey(rand.Reader)
		keys[id] = priv
		cfg.Validators = append(cfg.Validators, state.Validator{ID: id, Name: fmt.Sprint("v", id), Key: base64.StdEncoding.EncodeToString(pub)})
	}
	for id := uint64(1); id <= 4; id++ {
		ch := make(chan [3]any, 4096)
		c.r.in[id] = ch
		v, err := New(Options{Config: cfg, ID: id, Key: keys[id], Transport: tp{c.r, id}, Logger: quiet{}, WAL: memWAL{},
			Persist: func(Saved) {
				c.mu.Lock()
				w := c.wedge[id]
				c.mu.Unlock()
				if w != nil {
					<-w
				}
			}})
		if err != nil {
			t.Fatal(err)
		}
		c.vals[id] = v
		go func() {
			for f := range ch {
				c.mu.Lock()
				deaf := c.deaf[id] && f[1].(byte) == wire.Consensus
				c.mu.Unlock()
				if !deaf {
					v.Receive(f[0].(uint64), f[1].(byte), f[2].([]byte))
				}
			}
		}()
	}
	for _, v := range c.vals {
		if err := v.Start(); err != nil {
			t.Fatal(err)
		}
	}
	t.Cleanup(func() {
		c.mu.Lock()
		for _, w := range c.wedge {
			close(w)
		}
		c.wedge = map[uint64]chan struct{}{}
		c.mu.Unlock()
		for _, v := range c.vals {
			v.Stop()
		}
	})
	return c
}

// heartbeat: one request from the box, handed to every validator
func (c *cluster) heartbeat(n float64) {
	raw := c.box.req("heartbeat", state.Heartbeat{Facts: map[string]float64{"n": n}})
	for _, v := range c.vals {
		v.Receive(100, wire.Submit, raw)
	}
}

func (c *cluster) height(id uint64) uint64 { return c.vals[id].State().Height }

// until: heartbeats, one at a time, until ok
func (c *cluster) until(what string, ok func() bool) {
	end := time.Now().Add(30 * time.Second)
	for n := 0; time.Now().Before(end); n++ {
		if ok() {
			return
		}
		c.heartbeat(float64(1000 + n))
		time.Sleep(300 * time.Millisecond)
	}
	c.t.Fatalf("timed out deciding until %s (heights %d %d %d %d)", what, c.height(1), c.height(2), c.height(3), c.height(4))
}

func (c *cluster) waitFor(what string, d time.Duration, ok func() bool) {
	end := time.Now().Add(d)
	for time.Now().Before(end) {
		if ok() {
			return
		}
		time.Sleep(20 * time.Millisecond)
	}
	c.t.Fatalf("timed out waiting for %s (heights %d %d %d %d)", what, c.height(1), c.height(2), c.height(3), c.height(4))
}

// A validator that missed exactly the decision in flight rejoins and
// decides with the others again (SmartBFT #700).
func TestOneSequenceBehindRejoins(t *testing.T) {
	c := newCluster(t)
	c.heartbeat(0)
	c.waitFor("the first decision everywhere", 20*time.Second, func() bool {
		return c.height(1) >= 1 && c.height(2) >= 1 && c.height(3) >= 1 && c.height(4) >= 1
	})
	h := c.height(4)
	c.mu.Lock()
	c.deaf[4] = true
	c.mu.Unlock()
	c.heartbeat(1)
	c.waitFor("the others one ahead", 20*time.Second, func() bool {
		return c.height(1) == h+1 && c.height(2) == h+1 && c.height(3) == h+1
	})
	if c.height(4) != h {
		t.Fatalf("the deaf validator moved to %d", c.height(4))
	}
	c.mu.Lock()
	c.deaf[4] = false
	c.mu.Unlock()
	c.until("the one behind level with the others", func() bool {
		top := max(c.height(1), c.height(2), c.height(3))
		return top >= h+2 && c.height(4) == top
	})
	// and it decides with them, not only catches up
	before := c.height(4)
	c.until("a decision after it rejoined", func() bool { return c.height(4) > before })
}

// A validator whose consensus hangs keeps answering peers that catch up,
// its relay reader is never held up, and the watchdog names the hang.
func TestAWedgedValidatorStillAnswersAndIsFound(t *testing.T) {
	c := newCluster(t)
	c.heartbeat(0)
	c.waitFor("the first decision everywhere", 20*time.Second, func() bool {
		return c.height(1) >= 1 && c.height(2) >= 1 && c.height(3) >= 1 && c.height(4) >= 1
	})
	c.mu.Lock()
	c.wedge[4] = make(chan struct{})
	c.mu.Unlock()
	for i := 1; i < 4; i++ {
		c.heartbeat(float64(i))
		time.Sleep(300 * time.Millisecond)
	}
	c.waitFor("validator 4 stuck delivering", 20*time.Second, func() bool { return !c.vals[4].Health().Delivering.IsZero() })
	// the other three carry on
	top := c.height(1)
	c.heartbeat(10)
	c.waitFor("the others deciding without it", 20*time.Second, func() bool { return c.height(1) > top && c.height(2) > top })
	// a flood for its consensus never holds up whoever hands it frames
	start := time.Now()
	junk, _ := json.Marshal("x")
	for range 3 * inbox {
		c.vals[4].Receive(1, wire.Consensus, junk)
	}
	if d := time.Since(start); d > 2*time.Second {
		t.Fatalf("handing a wedged validator %d frames took %s", 3*inbox, d)
	}
	if c.vals[4].Health().Dropped == 0 {
		t.Fatal("a full queue dropped nothing")
	}
	// and it still answers "send me your latest", with a quorum-signed state
	c.r.mu.Lock()
	c.r.in[99] = make(chan [3]any, 16)
	asker := c.r.in[99]
	c.r.mu.Unlock()
	c.vals[4].Receive(99, wire.SyncAsk, nil)
	select {
	case f := <-asker:
		var s Saved
		if f[1].(byte) != wire.SyncState || json.Unmarshal(f[2].([]byte), &s) != nil || !c.vals[4].signedByQuorum(s.Proposal, s.Sigs) {
			t.Fatalf("a wedged validator's answer: %v", f[1])
		}
	case <-time.After(5 * time.Second):
		t.Fatal("a wedged validator did not answer a peer catching up")
	}
	// a hung delivery may hold up the call into consensus behind it too:
	// either is named
	h := c.vals[4].Health()
	h.PeersHeard, h.PeerHeight = 3, c.height(1) // as its watchdog's asking would find
	later := h.Delivering.Add(DefaultLimits.Hung + DefaultLimits.Grace)
	if why := Diagnose(h, later, time.Time{}, Who{Quorum: c.vals[4].Quorum(), Of: 1}, DefaultLimits); !strings.Contains(why, "has not returned") && !strings.Contains(why, "delivering decision") {
		t.Fatalf("the watchdog's reason: %q", why)
	}
}

// Watch asks the peers while requests wait, believes only what a quorum
// signed, learns they are ahead, and calls the wedge once its limits pass.
func TestWatchFindsPeersAhead(t *testing.T) {
	c := newCluster(t)
	c.until("a few decisions", func() bool { return c.height(1) >= 2 })
	c.vals[1].mu.Lock()
	ahead, _ := json.Marshal(c.vals[1].saved)
	c.vals[1].mu.Unlock()
	var forged Saved
	_ = json.Unmarshal(ahead, &forged)
	forged.Sigs = forged.Sigs[:1]
	lie, _ := json.Marshal(forged)
	// a validator 4 that never decided: consensus not started, and the
	// answers to what it asks come from the test
	var v *Validator
	asked := 0
	answers := [][]byte{lie, ahead}
	var amu sync.Mutex
	tr := sendFunc(func(to uint64, kind byte, _ []byte) {
		if kind != wire.SyncAsk {
			return
		}
		amu.Lock()
		a := answers[min(asked, len(answers)-1)]
		asked++
		amu.Unlock()
		go v.Receive(1, wire.SyncState, a)
	})
	v, err := New(Options{Config: c.cfg, ID: 4, Key: ed25519.NewKeyFromSeed(make([]byte, 32)), Transport: tr, Logger: quiet{}, WAL: memWAL{}})
	if err != nil {
		t.Fatal(err)
	}
	defer v.Stop()
	v.pulse.started(time.Now())
	v.Receive(100, wire.Submit, c.box.req("heartbeat", state.Heartbeat{}))
	l := Limits{Hung: time.Hour, Behind: time.Second, Stalled: time.Hour, Grace: 0, Quiet: time.Hour}
	got := make(chan string, 1)
	go v.Watch(l, 100*time.Millisecond, time.Time{}, func(why string) { got <- why })
	select {
	case why := <-got:
		if !strings.Contains(why, "peers are at height") {
			t.Fatalf("the watchdog's reason: %q", why)
		}
		amu.Lock()
		n := asked
		amu.Unlock()
		if n < 2 {
			t.Fatalf("found peers ahead after %d asks: the answer one validator signed was believed", n)
		}
	case <-time.After(10 * time.Second):
		t.Fatalf("the watchdog never found the peers ahead (health %+v)", v.Health())
	}
}

type sendFunc func(to uint64, kind byte, p []byte)

func (f sendFunc) Send(to uint64, kind byte, p []byte) { f(to, kind, p) }

// A flood of requests, from anyone who reaches the relay, and of votes
// from a sender that is no validator, never pushes out the validators'
// votes: the fleet keeps deciding, every validator with it.
func TestARequestFloodDoesNotStarveConsensus(t *testing.T) {
	c := newCluster(t)
	c.until("a first decision everywhere", func() bool {
		return c.height(1) >= 1 && c.height(2) >= 1 && c.height(3) >= 1 && c.height(4) >= 1
	})
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	outsider := &boxKey{name: "node1", k: k}
	junk := [][]byte{[]byte("junk"), outsider.req("heartbeat", state.Heartbeat{}), c.box.req("heartbeat", state.Heartbeat{})}
	stop := make(chan struct{})
	var wg sync.WaitGroup
	var sent atomic.Int64
	for _, v := range c.vals {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for i := 0; ; i++ {
				select {
				case <-stop:
					return
				default:
				}
				from := uint64(7 + i%50)
				v.Receive(from, wire.Submit, junk[i%len(junk)])
				v.Receive(from, wire.Forward, junk[i%len(junk)])
				v.Receive(from, wire.Consensus, []byte("not a vote"))
				sent.Add(1)
			}
		}()
	}
	defer func() { close(stop); wg.Wait() }()
	start := c.height(1)
	c.until("ten more decisions, every validator with them", func() bool {
		top := max(c.height(1), c.height(2), c.height(3), c.height(4))
		low := min(c.height(1), c.height(2), c.height(3), c.height(4))
		return top >= start+10 && low >= top-1
	})
	for id, v := range c.vals {
		if d := v.Health().PeerDropped; d != 0 {
			t.Errorf("validator %d dropped %d votes under the flood", id, d)
		}
	}
	if n := sent.Load(); n < 1000 {
		t.Fatalf("the flood was only %d frames", n)
	}
}
