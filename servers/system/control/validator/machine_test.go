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
	"sync/atomic"
	"testing"
	"time"

	"commonty/control/state"
	"commonty/control/state/attest"
	"commonty/control/state/ident"
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

// Release 360's switch on 2026-10-24: three validators in the config, the
// third (the witness) off. The two that run start on version 1 and switch
// to version 2 together. Version 2 and the new release's lease come once
// the witness has been heard from in none of absentDecisions decisions
// over absentAfter, and not before; before this, never.
func TestSwitchTogetherWithTheWitnessOff(t *testing.T) {
	t.Parallel()
	old := &state.Config{Boxes: map[string]state.Box{}, Rules: state.Rules{LeaseTTL: 45000, GoneAfter: 90000, Leases: map[string]state.LeaseRule{}}}
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
	raw, _ := json.Marshal(old)
	cfg := &state.Config{}
	_ = json.Unmarshal(raw, cfg)
	cfg.Rules.Leases["svc/postgresql"] = state.LeaseRule{Eligible: []string{"node1", "node2"}}
	cfg.Before = old

	r := &relay{in: map[uint64]chan [3]any{}, down: map[uint64]bool{3: true}}
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
	live := []uint64{1, 2}
	for _, id := range live {
		start(id, 1)
	}
	var smu sync.Mutex
	submit := func(box string, kind string, body any) {
		smu.Lock()
		raw := boxes[box].req(kind, body)
		smu.Unlock()
		for _, id := range live {
			r.mu.Lock()
			up := !r.down[id]
			r.mu.Unlock()
			if up {
				val(id).Receive(id, wire.Submit, raw)
			}
		}
	}
	var boxMachine atomic.Int64
	boxMachine.Store(1)
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
			// as its placer asks, every round
			submit("node1", "lease", state.LeaseOp{Role: "svc/postgresql", Op: "acquire"})
		}
	}()
	t0 := time.Now()
	waitFor := func(what string, within time.Duration, ok func(*state.State) bool) time.Duration {
		deadline := time.Now().Add(within)
		for time.Now().Before(deadline) {
			if ok(val(1).State()) && ok(val(2).State()) {
				t.Logf("%5.1fs %s", time.Since(t0).Seconds(), what)
				return time.Since(t0)
			}
			time.Sleep(100 * time.Millisecond)
		}
		t.Fatalf("timed out waiting for %s (heights %d, %d)", what, val(1).State().Height, val(2).State().Height)
		return 0
	}
	waitFor("decisions on version 1", 60*time.Second, func(s *state.State) bool { return s.Height > 10 })
	if s := val(1).State(); s.Machine != 0 || s.Leases["svc/postgresql"] != nil {
		t.Fatalf("version 1 agreed something new: %d %+v", s.Machine, s.Leases)
	}
	stop(1)
	stop(2)
	time.Sleep(time.Second)
	t0 = time.Now()
	start(1, 2)
	start(2, 2)
	boxMachine.Store(2)
	h := val(1).State().Height
	waitFor("decisions after the switch", 30*time.Second, func(s *state.State) bool { return s.Height > h+5 })
	at := waitFor("version 2 agreed", 90*time.Second, func(s *state.State) bool { return s.Machine == 2 })
	if at < absentAfter {
		t.Fatalf("version 2 agreed %v after the switch, before the witness could be found off", at)
	}
	waitFor("svc/postgresql granted", 10*time.Second, func(s *state.State) bool {
		l := s.Leases["svc/postgresql"]
		return l != nil && l.Holder == "node1"
	})
	if a, b := val(1).State(), val(2).State(); a.Height == b.Height && hashOf(a) != hashOf(b) {
		t.Fatalf("the two disagree at height %d", a.Height)
	}
	for _, id := range live {
		val(id).Stop()
	}
}

// A join big enough to carry a whole TPM proof reaches a block through the
// validators' pools from version 3, and only then, and not anything else
// that large; and an answer from a box still proving its TPM, which has no
// key in the state but its attestation's, passes their check of who signed
// it. Neither is seen by state.Block alone.
func TestARequestAsBigAsAProofIsJudged(t *testing.T) {
	t.Parallel()
	cfg := &state.Config{Boxes: map[string]state.Box{}, Rules: state.Rules{LeaseTTL: 15000, GoneAfter: 90000}}
	keys := map[uint64]ed25519.PrivateKey{}
	for id := uint64(1); id <= 3; id++ {
		pub, priv, _ := ed25519.GenerateKey(rand.Reader)
		keys[id] = priv
		cfg.Validators = append(cfg.Validators, state.Validator{ID: id, Name: fmt.Sprint("v", id), Key: base64.StdEncoding.EncodeToString(pub)})
	}
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	der, _ := x509.MarshalPKIXPublicKey(&k.PublicKey)
	spki := base64.StdEncoding.EncodeToString(der)
	name, _, _ := state.JoinName(spki)
	r := &relay{in: map[uint64]chan [3]any{}, down: map[uint64]bool{}}
	vals := map[uint64]*Validator{}
	for id := uint64(1); id <= 3; id++ {
		r.in[id] = make(chan [3]any, 4096)
		v, err := New(Options{Config: cfg, ID: id, Key: keys[id], Transport: tp{r, id}, Logger: quiet{}, WAL: memWAL{}, Persist: func(Saved) {}})
		if err != nil {
			t.Fatal(err)
		}
		vals[id] = v
		go func(v *Validator, ch chan [3]any) {
			for f := range ch {
				v.Receive(f[0].(uint64), f[1].(byte), f[2].([]byte))
			}
		}(v, r.in[id])
	}
	for _, v := range vals {
		if err := v.Start(); err != nil {
			t.Fatal(err)
		}
		defer v.Stop()
	}
	facts := map[string]float64{"tpm_major": 2, "uefi": 1, "secure_boot_capable": 1, "arch_x86_64": 1,
		"threads": ident.MinThreads, "mem_total_bytes": ident.MinMemTotal, "system_disk_bytes": ident.MinSystemDisk, "nics": 1}
	sign := func(q state.Request) []byte {
		d := sha256.Sum256(q.Bytes())
		sig, err := ecdsa.SignASN1(rand.Reader, k, d[:])
		if err != nil {
			t.Fatal(err)
		}
		q.Sig = base64.RawURLEncoding.EncodeToString(sig)
		raw, _ := json.Marshal(q)
		if len(raw) > state.MaxRequest {
			t.Fatalf("%d bytes, over the pool's %d", len(raw), state.MaxRequest)
		}
		return raw
	}
	// as large as a Begin may be, and no proof at all: refused for that,
	// and kept, which only a block does
	log := base64.StdEncoding.EncodeToString(make([]byte, attest.MaxBegin*3/4-4096))
	body, _ := json.Marshal(state.JoinReq{Key: spki, Recipient: "age1x", House: "h", Facts: facts, Attest: &attest.Begin{Log: log}})
	if len(body) <= attest.MaxBegin/2 {
		t.Fatalf("a request of %d bytes proves little", len(body))
	}
	raw := sign(state.Request{Kind: "join", Box: name, Seq: uint64(time.Now().UnixMilli()), Body: body})
	if vals[1].admit(raw) {
		t.Fatal("before version 3, a request above the old limit let in")
	}
	deadline := time.Now().Add(60 * time.Second)
	// a config is agreed in a block, and a block needs a request: any small one
	short, _ := json.Marshal(state.JoinReq{Key: spki, Recipient: "age1x", House: "h"})
	tick := state.Request{Kind: "join", Box: name, Body: short}
	for vals[1].State().Version() < 3 {
		if time.Now().After(deadline) {
			t.Fatal("version 3 was not agreed")
		}
		tick.Seq++
		for id, v := range vals {
			v.Receive(id, wire.Submit, sign(tick))
		}
		time.Sleep(200 * time.Millisecond)
	}
	for seq := uint64(time.Now().UnixMilli()); vals[1].State().Refused[name] == nil; seq++ {
		if time.Now().After(deadline) {
			t.Fatal("the join never reached a block")
		}
		raw = sign(state.Request{Kind: "join", Box: name, Seq: seq, Body: body})
		for id, v := range vals {
			v.Receive(id, wire.Submit, raw)
		}
		time.Sleep(time.Second)
	}
	// refused for that already: not carried again
	time.Sleep(1100 * time.Millisecond)
	if vals[1].admit(sign(state.Request{Kind: "join", Box: name, Seq: uint64(time.Now().UnixMilli()) + 10_000, Body: body})) {
		t.Fatal("the same refusal carried again")
	}
	// nor anything else that large
	time.Sleep(1100 * time.Millisecond)
	big, _ := json.Marshal(map[string]string{"x": log})
	if vals[1].admit(sign(state.Request{Kind: "directory", Box: name, Seq: 1, Body: big})) {
		t.Fatal("a directory item that large let in")
	}

	// a box still proving its TPM: signed by the key it started with
	k2, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	der2, _ := x509.MarshalPKIXPublicKey(&k2.PublicKey)
	spki2 := base64.StdEncoding.EncodeToString(der2)
	name2, _, _ := state.JoinName(spki2)
	v := vals[1]
	v.mu.Lock()
	v.st.Attest = attest.New()
	v.st.Attest.Pending[name2] = &attest.Pending{Key: spki2}
	v.mu.Unlock()
	a := state.Request{Kind: "attest-response", Box: name2, Seq: 1, Body: []byte("{}")}
	d := sha256.Sum256(a.Bytes())
	sig, _ := ecdsa.SignASN1(rand.Reader, k2, d[:])
	a.Sig = base64.RawURLEncoding.EncodeToString(sig)
	rawA, _ := json.Marshal(a)
	if _, err := v.VerifyRequest(rawA); err != nil {
		t.Fatalf("an answer from a box still proving: %v", err)
	}
	a.Kind = "heartbeat"
	d = sha256.Sum256(a.Bytes())
	sig, _ = ecdsa.SignASN1(rand.Reader, k2, d[:])
	a.Sig = base64.RawURLEncoding.EncodeToString(sig)
	rawA, _ = json.Marshal(a)
	if _, err := v.VerifyRequest(rawA); err == nil {
		t.Fatal("a heartbeat from a box still proving passed")
	}
}
