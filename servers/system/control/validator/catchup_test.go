package validator

import (
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"encoding/base64"
	"fmt"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"commonty/control/state"
	"commonty/control/wire"
)

// counts the complaints about a leader, the start of every view change
type complaints struct {
	quiet
	n *atomic.Int64
}

func (c complaints) Warnf(f string, a ...any) {
	if strings.Contains(f, "complaining about leader") {
		c.n.Add(1)
	}
}

// A validator that was behind and caught up by taking the others' latest
// state never received the decisions in between, so the requests they
// decided stayed in its pool. A leader drops such a request as already
// processed, so its timers ran out and it complained, and with three
// validators one complaint changes the view for all. Every view change
// restarted those timers: on 2026-10-24 the validators changed views for
// over a minute while a box switched under load. Now it prunes them, and
// a validator of 359, which does not, has them replayed in a block.
func TestCaughtUpValidatorHoldsNoDecidedRequests(t *testing.T) {
	for _, old := range []bool{false, true} {
		t.Run(fmt.Sprint("as 359: ", old), func(t *testing.T) { t.Parallel(); caughtUp(t, old) })
	}
}

func caughtUp(t *testing.T, old bool) {
	cfg := &state.Config{Boxes: map[string]state.Box{}, Rules: state.Rules{LeaseTTL: 15000, GoneAfter: 90000}}
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	der, _ := x509.MarshalPKIXPublicKey(&k.PublicKey)
	cfg.Boxes["node1"] = state.Box{ID: 100, House: "h1", Key: base64.StdEncoding.EncodeToString(der)}
	box := &boxKey{name: "node1", k: k}
	keys := map[uint64]ed25519.PrivateKey{}
	for id := uint64(1); id <= 3; id++ {
		pub, priv, _ := ed25519.GenerateKey(rand.Reader)
		keys[id] = priv
		cfg.Validators = append(cfg.Validators, state.Validator{ID: id, Name: fmt.Sprint("v", id), Key: base64.StdEncoding.EncodeToString(pub)})
	}
	r := &relay{in: map[uint64]chan [3]any{}, down: map[uint64]bool{}}
	var mu sync.Mutex
	deaf := map[uint64]bool{}
	var complained atomic.Int64
	vals := map[uint64]*Validator{}
	for id := uint64(1); id <= 3; id++ {
		ch := make(chan [3]any, 4096)
		r.in[id] = ch
		v, err := New(Options{Config: cfg, ID: id, Key: keys[id], Transport: tp{r, id}, Logger: complaints{n: &complained}, WAL: memWAL{}})
		if err != nil {
			t.Fatal(err)
		}
		v.keepDecided = old && id == 3
		vals[id] = v
		go func() {
			for f := range ch {
				mu.Lock()
				d := deaf[id] && f[1].(byte) == wire.Consensus
				mu.Unlock()
				if !d {
					v.Receive(f[0].(uint64), f[1].(byte), f[2].([]byte))
				}
			}
		}()
	}
	for _, v := range vals {
		if err := v.Start(); err != nil {
			t.Fatal(err)
		}
	}
	t.Cleanup(func() {
		for _, v := range vals {
			v.Stop()
		}
	})
	height := func(id uint64) uint64 { return vals[id].State().Height }
	n := 0
	heartbeat := func() {
		n++
		raw := box.req("heartbeat", state.Heartbeat{Facts: map[string]float64{"n": float64(n)}})
		for _, v := range vals {
			v.Receive(100, wire.Submit, raw)
		}
	}
	waitFor := func(what string, d time.Duration, ok func() bool) {
		end := time.Now().Add(d)
		for time.Now().Before(end) {
			if ok() {
				return
			}
			time.Sleep(20 * time.Millisecond)
		}
		t.Fatalf("timed out waiting for %s (heights %d %d %d)", what, height(1), height(2), height(3))
	}
	heartbeat()
	waitFor("the first decision everywhere", 20*time.Second, func() bool {
		return height(1) >= 1 && height(2) >= 1 && height(3) >= 1
	})
	// 3 hears the box but no votes while the other two decide; it rejoins
	// before its own requests could time out
	h := height(3)
	mu.Lock()
	deaf[3] = true
	mu.Unlock()
	for i := 0; i < 6; i++ {
		heartbeat()
		time.Sleep(150 * time.Millisecond)
	}
	waitFor("the others ahead", 10*time.Second, func() bool { return height(1) >= h+2 && height(2) >= h+2 })
	mu.Lock()
	deaf[3] = false
	mu.Unlock()
	for i := 0; height(3) < height(1) || height(3) <= h+2; i++ {
		if i > 100 {
			t.Fatalf("validator 3 never caught up (heights %d %d %d)", height(1), height(2), height(3))
		}
		heartbeat()
		time.Sleep(300 * time.Millisecond)
	}
	empty := func() bool {
		return vals[1].c.Pool.Size() == 0 && vals[2].c.Pool.Size() == 0 && vals[3].c.Pool.Size() == 0
	}
	// with nothing more to decide, every pool is empty: it took them out
	// as it caught up
	if !old {
		waitFor("every pool empty", 3*time.Second, empty)
	}
	// the box goes on as before: nobody complains, and every pool is
	// left with only what is still to be decided
	before := complained.Load()
	for range 5 {
		heartbeat()
		time.Sleep(2 * time.Second)
	}
	waitFor("every pool empty", 5*time.Second, empty)
	if c := complained.Load() - before; c > 0 {
		t.Fatalf("%d complaints about a leader after validator 3 caught up", c)
	}
}
