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
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"commonty/control/state"
	"commonty/control/wire"
)

// Three validators start on version 1 (as release 359: no stand, the
// config before); they switch to version 2 one at a time, in both orders.
// They decide throughout, hold the same state at every height, keep the
// lease, and agree version 2 with its config only once all three run it
// and every live box says it does too.
func TestVersionsSwitchOneAtATime(t *testing.T) {
	t.Parallel()
	for _, order := range [][]uint64{{1, 2, 3}, {3, 2, 1}} {
		t.Run(fmt.Sprint(order), func(t *testing.T) { t.Parallel(); switchInOrder(t, order) })
	}
}

func switchInOrder(t *testing.T, order []uint64) {
	old := &state.Config{Boxes: map[string]state.Box{}, Rules: state.Rules{
		LeaseTTL: 15000, GoneAfter: 90000,
		Leases: map[string]state.LeaseRule{"postgres": {Eligible: []string{"node1", "node2"}}},
	}}
	boxes := map[string]*boxKey{}
	for i, n := range []string{"node1", "node2"} {
		k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
		der, _ := x509.MarshalPKIXPublicKey(&k.PublicKey)
		old.Boxes[n] = state.Box{ID: uint64(100 + i), House: "h" + n, Key: base64.StdEncoding.EncodeToString(der)}
		boxes[n] = &boxKey{name: n, k: k}
	}
	keys := map[uint64]ed25519.PrivateKey{}
	for id := uint64(1); id <= 3; id++ {
		pub, priv, _ := ed25519.GenerateKey(rand.Reader)
		keys[id] = priv
		old.Validators = append(old.Validators, state.Validator{ID: id, Name: fmt.Sprint("v", id), Key: base64.StdEncoding.EncodeToString(pub)})
	}
	// the new release: a lease the old one has no rule for, and the old
	// config as the one before
	raw, _ := json.Marshal(old)
	cfg := &state.Config{}
	_ = json.Unmarshal(raw, cfg)
	cfg.Rules.Leases["svc/next"] = state.LeaseRule{Eligible: []string{"node1"}}
	cfg.Before = old

	r := &relay{in: map[uint64]chan [3]any{}, down: map[uint64]bool{}}
	var vmu sync.Mutex
	vals := map[uint64]*Validator{}
	saved := map[uint64]*Saved{}
	val := func(id uint64) *Validator { vmu.Lock(); defer vmu.Unlock(); return vals[id] }
	start := func(id uint64, machine int) {
		r.mu.Lock()
		r.in[id] = make(chan [3]any, 4096)
		r.down[id] = false
		ch := r.in[id]
		r.mu.Unlock()
		vmu.Lock()
		var s *Saved
		if saved[id] != nil {
			c := *saved[id]
			s = &c
		}
		vmu.Unlock()
		c := cfg
		if machine < 2 {
			c = old
		}
		v, err := New(Options{Config: c, ID: id, Key: keys[id], Transport: tp{r, id}, Logger: quiet{}, WAL: memWAL{}, Saved: s, Machine: machine,
			Persist: func(sv Saved) { vmu.Lock(); saved[id] = &sv; vmu.Unlock() }})
		if err != nil {
			t.Fatal(err)
		}
		vmu.Lock()
		vals[id] = v
		vmu.Unlock()
		go func() {
			for f := range ch {
				v.Receive(f[0].(uint64), f[1].(byte), f[2].([]byte))
			}
		}()
		if err := v.Start(); err != nil {
			t.Fatal(err)
		}
	}
	stop := func(id uint64) {
		r.mu.Lock()
		r.down[id] = true
		close(r.in[id])
		delete(r.in, id)
		r.mu.Unlock()
		val(id).Stop()
	}
	for id := uint64(1); id <= 3; id++ {
		start(id, 1)
	}
	var smu sync.Mutex
	submit := func(box string, kind string, body any) {
		smu.Lock()
		raw := boxes[box].req(kind, body)
		smu.Unlock()
		for id := uint64(1); id <= 3; id++ {
			r.mu.Lock()
			up := !r.down[id]
			r.mu.Unlock()
			if up {
				val(id).Receive(id, wire.Submit, raw)
			}
		}
	}
	// the boxes heartbeat and node1 renews its lease, as on a box; what
	// version they say comes with their own switch
	var boxMachine atomic.Int64
	boxMachine.Store(1)
	var epoch atomic.Uint64
	done := make(chan struct{})
	defer close(done)
	go func() {
		for {
			select {
			case <-done:
				return
			case <-time.After(300 * time.Millisecond):
			}
			f := map[string]float64{"cores": 4}
			if m := boxMachine.Load(); m > 1 {
				f[state.MachineFact] = float64(m)
			}
			submit("node1", "heartbeat", state.Heartbeat{Facts: f})
			submit("node2", "heartbeat", state.Heartbeat{Facts: f})
			if e := epoch.Load(); e > 0 {
				submit("node1", "lease", state.LeaseOp{Role: "postgres", Op: "renew", Epoch: e})
				// and asks, as its placer does, for what only the new
				// release has a rule for
				submit("node1", "lease", state.LeaseOp{Role: "svc/next", Op: "acquire"})
			}
		}
	}()

	t0 := time.Now()
	waitFor := func(what string, ok func(*state.State) bool, ids ...uint64) {
		deadline := time.Now().Add(40 * time.Second)
		for time.Now().Before(deadline) {
			all := true
			for _, id := range ids {
				if !ok(val(id).State()) {
					all = false
				}
			}
			if all {
				t.Logf("%5.1fs %s", time.Since(t0).Seconds(), what)
				return
			}
			time.Sleep(100 * time.Millisecond)
		}
		t.Fatalf("timed out waiting for %s", what)
	}
	all := []uint64{1, 2, 3}
	waitFor("both heartbeats", func(s *state.State) bool { return len(s.Boxes) == 2 }, all...)
	submit("node1", "lease", state.LeaseOp{Role: "postgres", Op: "acquire"})
	waitFor("node1's lease", func(s *state.State) bool {
		l := s.Leases["postgres"]
		return l != nil && l.Holder == "node1"
	}, all...)
	epoch.Store(val(1).State().Leases["postgres"].Epoch)
	held := epoch.Load()

	// the same state at the same height on every validator, decisions still
	// coming, the lease still node1's at its epoch, and nothing new agreed
	steady := func(what string, agreed bool) {
		h := val(order[0]).State().Height
		waitFor(what, func(s *state.State) bool { return s.Height >= h+5 }, all...)
		byHeight := map[uint64]string{}
		for _, id := range all {
			s := val(id).State()
			b, _ := json.Marshal(s)
			if prev, ok := byHeight[s.Height]; ok && prev != string(b) {
				t.Fatalf("%s: validator %d disagrees at height %d", what, id, s.Height)
			}
			byHeight[s.Height] = string(b)
			if l := s.Leases["postgres"]; l == nil || l.Holder != "node1" || l.Epoch != held {
				t.Fatalf("%s: the lease moved on validator %d: %+v", what, id, l)
			}
			if !agreed && (s.Machine != 0 || len(s.Config) != 0) {
				t.Fatalf("%s: version %d agreed on validator %d before its time", what, s.Machine, id)
			}
			if !agreed && s.Leases["svc/next"] != nil {
				t.Fatalf("%s: a lease of the new release's granted before it was agreed", what)
			}
		}
	}
	steady("all on version 1", false)
	for i, id := range order {
		stop(id)
		time.Sleep(time.Second)
		start(id, 2)
		steady(fmt.Sprintf("%d of 3 on version 2", i+1), false)
	}
	// every validator stands for 2; the boxes do not yet
	steady("validators on 2, boxes on 1", false)
	boxMachine.Store(2)
	waitFor("version 2 agreed", func(s *state.State) bool { return s.Machine == 2 && len(s.Config) > 0 }, all...)
	waitFor("the new release's lease", func(s *state.State) bool {
		l := s.Leases["svc/next"]
		return l != nil && l.Holder == "node1"
	}, all...)
	steady("after", true)
	_, d := cfg.Canonical()
	if got := val(1).State().ConfigDigest(); got != d {
		t.Fatalf("agreed config %s, want %s", got, d)
	}
	for _, id := range all {
		val(id).Stop()
	}
}
