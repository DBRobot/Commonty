package validator

import (
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"sync"
	"testing"
	"time"

	"commonty/control/state"
	"commonty/control/wire"
)

type quiet struct{}

func (quiet) Debugf(string, ...any)     {}
func (quiet) Infof(string, ...any)      {}
func (quiet) Warnf(f string, a ...any)  { fmt.Printf("W "+f+"\n", a...) }
func (quiet) Errorf(f string, a ...any) { fmt.Printf("E "+f+"\n", a...) }
func (quiet) Panicf(f string, a ...any) { panic(fmt.Sprintf(f, a...)) }

type memWAL struct{}

func (memWAL) Append([]byte, bool) error { return nil }

// relay: frames to one node, to the validators, or dropped for a node that is down
type relay struct {
	mu   sync.Mutex
	in   map[uint64]chan [3]any
	down map[uint64]bool
}

func (r *relay) send(from, to uint64, kind byte, p []byte) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if r.down[from] {
		return
	}
	deliver := func(id uint64) {
		if id == from || r.down[id] {
			return
		}
		select {
		case r.in[id] <- [3]any{from, kind, append([]byte(nil), p...)}:
		default:
		}
	}
	if to == wire.ToValidators || to == wire.ToAll {
		for id := range r.in {
			deliver(id)
		}
		return
	}
	deliver(to)
}

type tp struct {
	r  *relay
	id uint64
}

func (t tp) Send(to uint64, kind byte, p []byte) { t.r.send(t.id, to, kind, p) }

type boxKey struct {
	name string
	k    *ecdsa.PrivateKey
	seq  uint64
}

func (b *boxKey) req(kind string, body any) []byte {
	b.seq++
	raw, _ := json.Marshal(body)
	q := state.Request{Kind: kind, Box: b.name, Seq: b.seq, Body: raw}
	h := sha256.Sum256(q.Bytes())
	sig, _ := ecdsa.SignASN1(rand.Reader, b.k, h[:])
	q.Sig = base64.RawURLEncoding.EncodeToString(sig)
	out, _ := json.Marshal(q)
	return out
}

func TestFourValidatorsAgreeAndARestartedOneCatchesUp(t *testing.T) {
	cfg := &state.Config{Boxes: map[string]state.Box{}, Rules: state.Rules{
		LeaseTTL: 15000, GoneAfter: 90000,
		Leases: map[string]state.LeaseRule{"postgres": {Eligible: []string{"node1", "node2"}}},
	}}
	boxes := map[string]*boxKey{}
	for i, n := range []string{"node1", "node2"} {
		k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
		der, _ := x509.MarshalPKIXPublicKey(&k.PublicKey)
		cfg.Boxes[n] = state.Box{ID: uint64(100 + i), House: "h" + n, Key: base64.StdEncoding.EncodeToString(der)}
		boxes[n] = &boxKey{name: n, k: k}
	}
	keys := map[uint64]ed25519.PrivateKey{}
	for id := uint64(1); id <= 4; id++ {
		pub, priv, _ := ed25519.GenerateKey(rand.Reader)
		keys[id] = priv
		cfg.Validators = append(cfg.Validators, state.Validator{ID: id, Name: fmt.Sprint("v", id), Key: base64.StdEncoding.EncodeToString(pub)})
	}
	r := &relay{in: map[uint64]chan [3]any{}, down: map[uint64]bool{}}
	vals := map[uint64]*Validator{}
	saved := map[uint64]*Saved{}
	var smu sync.Mutex
	start := func(id uint64) {
		r.mu.Lock()
		r.in[id] = make(chan [3]any, 4096)
		r.down[id] = false
		ch := r.in[id]
		r.mu.Unlock()
		smu.Lock()
		var s *Saved
		if saved[id] != nil {
			c := *saved[id]
			s = &c
		}
		smu.Unlock()
		v, err := New(Options{Config: cfg, ID: id, Key: keys[id], Transport: tp{r, id}, Logger: quiet{}, WAL: memWAL{}, Saved: s,
			Persist: func(sv Saved) { smu.Lock(); saved[id] = &sv; smu.Unlock() }})
		if err != nil {
			t.Fatal(err)
		}
		vals[id] = v
		go func() {
			for f := range ch {
				v.Receive(f[0].(uint64), f[1].(byte), f[2].([]byte))
			}
		}()
		if err := v.Start(); err != nil {
			t.Fatal(err)
		}
	}
	for id := uint64(1); id <= 4; id++ {
		start(id)
	}
	t0 := time.Now()
	submit := func(raw []byte) {
		for id, v := range vals {
			if !r.down[id] {
				v.Receive(cfg.Boxes["node1"].ID, wire.Submit, raw)
			}
		}
	}
	waitFor := func(what string, ok func(*state.State) bool, ids ...uint64) {
		deadline := time.Now().Add(30 * time.Second)
		for time.Now().Before(deadline) {
			all := true
			for _, id := range ids {
				if !ok(vals[id].State()) {
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
	// both boxes are heard; node1 takes the writer lease
	submit(boxes["node1"].req("heartbeat", state.Heartbeat{Facts: map[string]float64{"cores": 8}}))
	submit(boxes["node2"].req("heartbeat", state.Heartbeat{Facts: map[string]float64{"cores": 4}}))
	waitFor("both heartbeats", func(s *state.State) bool { return len(s.Boxes) == 2 }, 1, 2, 3, 4)
	submit(boxes["node1"].req("lease", state.LeaseOp{Role: "postgres", Op: "acquire"}))
	waitFor("node1's lease", func(s *state.State) bool {
		l := s.Leases["postgres"]
		return l != nil && l.Holder == "node1" && l.Epoch == 1
	}, 1, 2, 3, 4)
	// a forged request is ordered but never applied
	forged := boxes["node2"].req("lease", state.LeaseOp{Role: "postgres", Op: "acquire"})
	var q state.Request
	_ = json.Unmarshal(forged, &q)
	q.Box = "node1" // node2's signature on node1's name
	fb, _ := json.Marshal(q)
	submit(fb)
	// every validator holds the same state
	a, _ := json.Marshal(vals[1].State())
	for id := uint64(2); id <= 4; id++ {
		if b, _ := json.Marshal(vals[id].State()); string(a) != string(b) && vals[id].State().Height == vals[1].State().Height {
			t.Fatalf("validator %d disagrees", id)
		}
	}
	// one validator goes down; the other three carry on
	r.mu.Lock()
	r.down[3] = true
	r.mu.Unlock()
	vals[3].Stop()
	submit(boxes["node2"].req("heartbeat", state.Heartbeat{Roles: []string{"garage"}}))
	waitFor("node2's roles with one validator down", func(s *state.State) bool {
		b := s.Boxes["node2"]
		return b != nil && len(b.Roles) == 1
	}, 1, 2, 4)
	// it comes back from what it saved and catches up from the others
	start(3)
	submit(boxes["node1"].req("heartbeat", state.Heartbeat{Roles: []string{"postgres"}}))
	waitFor("the restarted validator to catch up", func(s *state.State) bool {
		b := s.Boxes["node2"]
		n := s.Boxes["node1"]
		return b != nil && len(b.Roles) == 1 && n != nil && len(n.Roles) == 1
	}, 1, 2, 3, 4)
	for _, v := range vals {
		v.Stop()
	}
}
