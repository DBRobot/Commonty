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
	"testing"
	"time"

	"commonty/control/state"

	bft "github.com/hyperledger/SmartBFT/pkg/types"
)

type standFleet struct {
	t    *testing.T
	keys map[uint64]ed25519.PrivateKey
	cfg  *state.Config
	box  *boxKey
	now  int64
}

func newStandFleet(t *testing.T) *standFleet {
	f := &standFleet{t: t, keys: map[uint64]ed25519.PrivateKey{}, now: 10_000_000}
	f.cfg = &state.Config{Boxes: map[string]state.Box{}, Rules: state.Rules{LeaseTTL: 15000, GoneAfter: 90000}}
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	der, _ := x509.MarshalPKIXPublicKey(&k.PublicKey)
	f.cfg.Boxes["node1"] = state.Box{ID: 101, House: "h", Key: base64.StdEncoding.EncodeToString(der)}
	f.box = &boxKey{name: "node1", k: k}
	for id := uint64(1); id <= 3; id++ {
		pub, priv, _ := ed25519.GenerateKey(rand.Reader)
		f.keys[id] = priv
		f.cfg.Validators = append(f.cfg.Validators, state.Validator{ID: id, Name: fmt.Sprint("v", id), Key: base64.StdEncoding.EncodeToString(pub)})
	}
	return f
}

// a validator, id 1, with cfg as its own
func (f *standFleet) validator(cfg *state.Config) *Validator {
	v, err := New(Options{Config: cfg, ID: 1, Key: f.keys[1], Transport: tp{&relay{in: map[uint64]chan [3]any{}, down: map[uint64]bool{}}, 1},
		Logger: quiet{}, WAL: memWAL{}, Now: func() time.Time { return time.UnixMilli(f.now) }})
	if err != nil {
		f.t.Fatal(err)
	}
	f.t.Cleanup(v.Stop)
	return v
}

func (f *standFleet) stand(id uint64, cfg *state.Config, since, at int64) Stand {
	_, d := cfg.Canonical()
	s := Stand{ID: id, Machine: 2, Config: d, Since: since, Time: at}
	s.Sig = ed25519.Sign(f.keys[id], s.bytes())
	return s
}

func (f *standFleet) activation(cfg *state.Config, since, at int64) *activation {
	a := &activation{Machine: 2}
	a.Config, _ = cfg.Canonical()
	for id := uint64(1); id <= 3; id++ {
		a.Stands = append(a.Stands, f.stand(id, cfg, since, at))
	}
	return a
}

// a state where node1 is live and says it runs version m
func (f *standFleet) state(m float64) *state.State {
	st := state.New()
	st.Time = f.now - 1000
	st.Boxes["node1"] = &state.BoxState{LastSeen: f.now - 1000, Facts: map[string]float64{state.MachineFact: m}}
	return st
}

func refused(t *testing.T, err error, why string) {
	t.Helper()
	if err == nil || !strings.Contains(err.Error(), why) {
		t.Fatalf("want %q, got %v", why, err)
	}
}

func TestActivationIsTheVerifiersOwnConfig(t *testing.T) {
	f := newStandFleet(t)
	v := f.validator(f.cfg)
	if err := f.activation(f.cfg, f.now-5000, f.now).check(v, f.state(2), f.now); err != nil {
		t.Fatalf("a good activation refused: %v", err)
	}
	// every validator stood for an older config once; this one has moved on
	older := *f.cfg
	older.Rules.LeaseTTL = 1
	refused(t, f.activation(&older, f.now-5000, f.now).check(v, f.state(2), f.now), "is not this validator's")
}

func TestActivationCarriesTheVerifiersLatestStand(t *testing.T) {
	f := newStandFleet(t)
	v := f.validator(f.cfg)
	v.stands[1] = f.stand(1, f.cfg, f.now-9000, f.now-1000)
	a := f.activation(f.cfg, f.now-9000, f.now-2000)
	refused(t, a.check(v, f.state(2), f.now), "older than its latest")
	a = f.activation(f.cfg, f.now-9000, f.now-1000)
	if err := a.check(v, f.state(2), f.now); err != nil {
		t.Fatalf("its latest refused: %v", err)
	}
}

func TestAStandFromBeforeTheConfigInForceAgreesNothing(t *testing.T) {
	f := newStandFleet(t)
	v := f.validator(f.cfg)
	st := f.state(2)
	other := *f.cfg
	other.Rules.LeaseTTL = 1
	st.Machine, st.Config = 2, func() json.RawMessage { c, _ := other.Canonical(); return c }()
	st.ConfigTime = f.now - 3000
	refused(t, f.activation(f.cfg, f.now-9000, f.now-3000).check(v, st, f.now), "before the config in force")
	if err := f.activation(f.cfg, f.now-9000, f.now-2999).check(v, st, f.now); err != nil {
		t.Fatalf("a stand after it refused: %v", err)
	}
}

// A box saying an old version holds activation back for boxGrace after the
// last validator stood, no longer; one the new config drops or rekeys never
func TestABoxHoldsActivationBackOnlyForAWhile(t *testing.T) {
	f := newStandFleet(t)
	v := f.validator(f.cfg)
	refused(t, f.activation(f.cfg, f.now-boxGrace+1, f.now).check(v, f.state(1), f.now), "run a version below 2")
	if err := f.activation(f.cfg, f.now-boxGrace, f.now).check(v, f.state(1), f.now); err != nil {
		t.Fatalf("a box held it back past the grace: %v", err)
	}
	// the new config knows node1 by another key: its word never counts
	next := *f.cfg
	next.Boxes = map[string]state.Box{"node1": {ID: 101, House: "h", Key: "another"}}
	nv := f.validator(&next)
	st := f.state(1)
	st.Machine, st.Config = 2, func() json.RawMessage { c, _ := f.cfg.Canonical(); return c }()
	st.ConfigTime = f.now - 60_000
	if err := f.activation(&next, f.now-5000, f.now).check(nv, st, f.now); err != nil {
		t.Fatalf("a rekeyed box held it back: %v", err)
	}
	// and none drops out: a box that stops reporting is not live and does
	// not count either
	st = f.state(1)
	st.Boxes["node1"].LastSeen = f.now - f.cfg.Rules.GoneAfter - 1
	if err := f.activation(f.cfg, f.now-5000, f.now).check(v, st, f.now); err != nil {
		t.Fatalf("a box gone held it back: %v", err)
	}
}

// The block that changes the config applies a request only if both
// configs know its box by the same key
func TestTheActivatingBlockTakesOnlyKeysBothKnow(t *testing.T) {
	f := newStandFleet(t)
	before := *f.cfg
	before.Boxes = map[string]state.Box{"node1": {ID: 101, House: "h", Key: "old-key"}}
	f.cfg.Before = &before
	v := f.validator(f.cfg)
	st := f.state(2)
	raw := f.box.req("heartbeat", state.Heartbeat{Facts: map[string]float64{"cores": 1}})
	data, _ := json.Marshal([][]byte{raw})
	a := f.activation(f.cfg, f.now-5000, f.now)
	h := header{Height: st.Height + 1, Time: f.now, Before: hashOf(st), Data: digest(data), Activate: a}
	hb, _ := json.Marshal(h)
	v.mu.Lock()
	v.apply(st, proposalOf(hb, data), nil, h)
	got := v.st
	v.mu.Unlock()
	if got.Machine != 2 || got.ConfigTime != f.now {
		t.Fatalf("not activated: %d %d", got.Machine, got.ConfigTime)
	}
	if got.Boxes["node1"].Facts["cores"] == 1 {
		t.Fatal("a request signed by a key only the new config knows was applied across the change")
	}
	// the block after: the new config alone
	raw = f.box.req("heartbeat", state.Heartbeat{Facts: map[string]float64{"cores": 1}})
	data, _ = json.Marshal([][]byte{raw})
	h = header{Height: got.Height + 1, Time: f.now + 1, Before: hashOf(got), Data: digest(data)}
	hb, _ = json.Marshal(h)
	v.mu.Lock()
	v.apply(got, proposalOf(hb, data), nil, h)
	got = v.st
	v.mu.Unlock()
	if got.Boxes["node1"].Facts["cores"] != 1 {
		t.Fatal("the new config's key refused after the change")
	}
}

func proposalOf(header, payload []byte) bft.Proposal {
	return bft.Proposal{Header: header, Payload: payload}
}
