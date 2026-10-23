package validator

import (
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"fmt"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"commonty/control/state"
	"commonty/control/wire"
)

// A validator's votes wait in a queue of their own: another validator
// that floods fills only its own, and the votes are taken in turn.
func TestAFloodingValidatorNeverPushesOutOtherVotes(t *testing.T) {
	cfg := &state.Config{Boxes: map[string]state.Box{}}
	keys := map[uint64]ed25519.PrivateKey{}
	for id := uint64(1); id <= 4; id++ {
		pub, priv, _ := ed25519.GenerateKey(rand.Reader)
		keys[id] = priv
		cfg.Validators = append(cfg.Validators, state.Validator{ID: id, Name: fmt.Sprint("v", id), Key: base64.StdEncoding.EncodeToString(pub)})
	}
	// not started: nothing takes the votes, so the queues are as Receive left them
	v, err := New(Options{Config: cfg, ID: 1, Key: keys[1], Transport: tp{&relay{in: map[uint64]chan [3]any{}, down: map[uint64]bool{}}, 1}, Logger: quiet{}, WAL: memWAL{}})
	if err != nil {
		t.Fatal(err)
	}
	defer v.Stop()
	v.Receive(3, wire.Consensus, []byte("3's vote"))
	for i := 0; i < 10*peerInbox; i++ {
		v.Receive(2, wire.Consensus, []byte(fmt.Sprint("2's ", i)))
	}
	v.Receive(4, wire.Consensus, []byte("4's vote"))
	// and nobody else's, nor this validator's own, get a queue
	v.Receive(1, wire.Consensus, []byte("its own"))
	v.Receive(9, wire.Consensus, []byte("nobody's"))
	if n := len(v.peers[2]); n != peerInbox {
		t.Fatalf("2's queue holds %d, want %d", n, peerInbox)
	}
	if h := v.Health(); h.PeerDropped != uint64(9*peerInbox) {
		t.Fatalf("%d votes dropped, want only 2's %d", h.PeerDropped, 9*peerInbox)
	}
	var got []string
	for {
		f, ok := v.nextVote()
		if !ok {
			break
		}
		if len(got) < 5 {
			got = append(got, string(f.payload))
		}
	}
	// 2's oldest left is its 9*peerInbox-th; 3 and 4 are taken in their turn
	want := []string{fmt.Sprint("2's ", 9*peerInbox), "3's vote", "4's vote", fmt.Sprint("2's ", 9*peerInbox+1), fmt.Sprint("2's ", 9*peerInbox+2)}
	if fmt.Sprint(got) != fmt.Sprint(want) {
		t.Fatalf("taken in the order %q, want %q", got, want)
	}
}

// A validator that sends junk votes without end slows nobody: the other
// three keep deciding, every one of them, and only its own frames are lost.
func TestAValidatorsVoteFloodDoesNotStarveConsensus(t *testing.T) {
	c := newCluster(t)
	c.until("a first decision everywhere", func() bool {
		return c.height(1) >= 1 && c.height(2) >= 1 && c.height(3) >= 1 && c.height(4) >= 1
	})
	stop := make(chan struct{})
	var wg sync.WaitGroup
	var sent atomic.Int64
	for _, id := range []uint64{1, 2, 3} {
		v := c.vals[id]
		wg.Add(1)
		go func() {
			defer wg.Done()
			for {
				select {
				case <-stop:
					return
				default:
				}
				v.Receive(4, wire.Consensus, []byte("not a vote"))
				sent.Add(1)
			}
		}()
	}
	defer func() { close(stop); wg.Wait() }()
	start := max(c.height(1), c.height(2), c.height(3))
	c.until("ten more decisions among the three", func() bool {
		top := max(c.height(1), c.height(2), c.height(3))
		low := min(c.height(1), c.height(2), c.height(3))
		return top >= start+10 && low >= top-1
	})
	if n := sent.Load(); n < 1000 {
		t.Fatalf("the flood was only %d frames", n)
	}
}

func TestLimiterNeverResetsABucketForAnother(t *testing.T) {
	var l limiter
	now := time.Unix(1000, 0)
	// a full table of senders, then one that has spent its burst
	for id := uint64(1); id < senders; id++ {
		l.allow(id, aValidator, now)
	}
	for i := 0; i < burst; i++ {
		if !l.allow(senders, aValidator, now) {
			t.Fatalf("refused within the burst, at %d", i)
		}
	}
	if l.allow(senders, aValidator, now) {
		t.Fatal("allowed past the burst")
	}
	// new senders past the cap make room one at a time and leave it spent
	for id := uint64(senders + 1); id < 3*senders; id++ {
		l.allow(id, aValidator, now)
		if len(l.buckets) > senders {
			t.Fatalf("%d buckets, over the cap of %d", len(l.buckets), senders)
		}
	}
	if l.allow(senders, aValidator, now) {
		t.Fatal("a full table handed a spent sender a fresh bucket")
	}
	// buckets that have refilled are the first to go, all of them
	later := now.Add(burst / perSecond * time.Second)
	l.allow(senders, aValidator, later)
	l.allow(9*senders, aValidator, later)
	if len(l.buckets) != 2 {
		t.Fatalf("%d buckets after the refilled ones went, want 2", len(l.buckets))
	}
}

func TestLimiterTurnsAwayUnknownSendersAndCapsTheRest(t *testing.T) {
	var l limiter
	now := time.Unix(1000, 0)
	for id := uint64(0); id < 10000; id++ {
		if l.allow(id, unknown, now) {
			t.Fatal("an unknown sender was let in")
		}
	}
	if len(l.buckets) != 0 {
		t.Fatalf("unknown senders were given %d buckets", len(l.buckets))
	}
	// boxes still joining share one small bucket, whatever their addresses
	in := 0
	for id := uint64(0); id < 1000; id++ {
		if l.allow(state.JoinedIDBase+id, aStranger, now) {
			in++
		}
	}
	if in != strangerBurst {
		t.Fatalf("%d requests from boxes joining went in, want %d", in, strangerBurst)
	}
	// every box together is capped; the validators are not
	in = 0
	for id := uint64(1); id <= 100; id++ {
		for i := 0; i < burst; i++ {
			if l.allow(1000+id, aBox, now) {
				in++
			}
		}
	}
	if in != allBurst-strangerBurst {
		t.Fatalf("%d requests from boxes went in, want the cap's %d", in, allBurst-strangerBurst)
	}
	if !l.allow(1, aValidator, now) {
		t.Fatal("a validator was held to the boxes' cap")
	}
}

func TestSenderIsKnownByTheConfigAndTheAgreedState(t *testing.T) {
	c := newCluster(t)
	v := c.vals[1]
	v.mu.Lock()
	v.st.Joined = map[string]*state.Joined{"box-x": {ID: state.JoinedIDBase + 7}}
	v.mu.Unlock()
	for _, tc := range []struct {
		from uint64
		want sender
	}{
		{2, aValidator},
		{100, aBox},
		{state.JoinedIDBase + 7, aBox},
		{state.JoinedIDBase + 8, aStranger},
		{101, unknown},
		{2 * state.JoinedIDBase, unknown},
		{wire.ToValidators, unknown},
	} {
		if got := v.sender(tc.from); got != tc.want {
			t.Errorf("sender(%d) = %d, want %d", tc.from, got, tc.want)
		}
	}
}
