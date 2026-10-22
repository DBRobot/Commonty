package state

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"strings"
	"testing"

	"commonty/control/state/ident"
)

var relKey = ed25519.NewKeyFromSeed(make([]byte, 32))

func relPub() string { return base64.StdEncoding.EncodeToString(relKey.Public().(ed25519.PublicKey)) }

// a release file as dd release publish writes it: the payload, and the
// release key's signature over it with keys sorted at every level
func release(t *testing.T, counter uint64, key ed25519.PrivateKey, boxes ...string) string {
	t.Helper()
	bs := map[string]any{}
	for _, b := range boxes {
		bs[b] = map[string]any{"path": "/nix/store/x-" + b, "nar_hash": "sha256-x"}
	}
	payload, _ := json.Marshal(map[string]any{"counter": counter, "rev": "r", "issued": 1, "boxes": bs})
	msg, err := ident.CanonicalValue(payload)
	if err != nil {
		t.Fatal(err)
	}
	pub := base64.StdEncoding.EncodeToString(key.Public().(ed25519.PublicKey))
	out, _ := json.Marshal(map[string]any{
		"payload":   json.RawMessage(payload),
		"signer":    pub,
		"signature": base64.StdEncoding.EncodeToString(ed25519.Sign(key, msg)),
	})
	return string(out)
}

func rolloutFleet(t *testing.T) *fleet {
	f := newFleet(t)
	f.c.Identity = &ident.Config{Release: relPub()}
	f.c.Rules.Rollout = &RolloutRules{SoakMs: []int64{30_000, 0}, TookMs: 60_000, GraceMs: 60_000}
	return f
}

func (f *fleet) beatFacts(box string, now int64, facts map[string]float64) {
	if f.block(now, f.req(box, "heartbeat", Heartbeat{Facts: facts})) != 1 {
		f.t.Fatalf("%s heartbeat refused", box)
	}
}

func (f *fleet) roll(box string, now int64, op RolloutOp) bool {
	return f.block(now, f.req(box, "rollout", op)) == 1
}

func verdict(n uint64, v string) RolloutOp { return RolloutOp{Op: "verdict", Counter: n, Verdict: v} }

func TestBeginTakesOnlyAReleaseSignedByTheReleaseKey(t *testing.T) {
	f := rolloutFleet(t)
	f.beat("node1", 1000)
	other := ed25519.NewKeyFromSeed([]byte(strings.Repeat("o", 32)))
	if f.roll("node1", 2000, RolloutOp{Op: "begin", Release: release(t, 5, other, "node1")}) {
		t.Fatal("a release signed by another key began a rollout")
	}
	good := release(t, 5, relKey, "node1", "node2")
	tampered := strings.Replace(good, `"counter":5`, `"counter":6`, 1)
	if f.roll("node1", 2000, RolloutOp{Op: "begin", Release: tampered}) {
		t.Fatal("a changed release began a rollout")
	}
	if !f.roll("node1", 2000, RolloutOp{Op: "begin", Release: good}) {
		t.Fatal("the release did not begin a rollout")
	}
	if r := f.s.Rollout; r.Counter != 5 || r.Digest != agentDigest(t, good) || r.Status != "soaking" {
		t.Fatalf("rollout %+v", r)
	}
	began := *f.s.Rollout
	if !f.roll("node2", 2100, RolloutOp{Op: "begin", Release: good}) || f.s.Rollout.Began != began.Began {
		t.Fatal("the same release, begun again, was not a no-op")
	}
	if f.roll("node2", 2100, RolloutOp{Op: "begin", Release: release(t, 4, relKey, "node1", "node2")}) {
		t.Fatal("an older release replaced the rollout")
	}
}

// node1 holds the writers, node2 nothing: with two boxes node2 is wave 1
func TestTwoBoxesNode2GoesFirst(t *testing.T) {
	f := rolloutFleet(t)
	delete(f.c.Boxes, "node3")
	f.beat("node1", 1000)
	f.beat("node2", 1000)
	f.block(1100, f.req("node1", "lease", lease("postgres", "acquire", 0)))
	if !f.roll("node1", 1200, RolloutOp{Op: "begin", Release: release(t, 7, relKey, "node1", "node2")}) {
		t.Fatal("no begin")
	}
	got := fmt.Sprint(f.s.Rollout.Waves)
	if got != "[[node2] [node1]]" {
		t.Fatalf("waves %s", got)
	}
}

func TestAGoodReleaseFlowsWaveByWave(t *testing.T) {
	f := rolloutFleet(t)
	delete(f.c.Boxes, "node3")
	f.beat("node1", 1000)
	f.beat("node2", 1000)
	f.block(1100, f.req("node1", "lease", lease("postgres", "acquire", 0)))
	f.roll("node1", 1200, RolloutOp{Op: "begin", Release: release(t, 7, relKey, "node1", "node2")})
	r := f.s.Rollout
	if !r.May("node2") || r.May("node1") {
		t.Fatal("wave 1 is node2 alone")
	}
	if f.roll("node1", 1300, verdict(7, "took")) {
		t.Fatal("a held box said it took the release")
	}
	if f.roll("node2", 1300, verdict(7, "good")) {
		t.Fatal("good before took")
	}
	if !f.roll("node2", 5000, verdict(7, "took")) {
		t.Fatal("took refused")
	}
	// good before the soak is over counts for nothing yet
	f.roll("node2", 20_000, verdict(7, "good"))
	if f.s.Rollout.Status != "soaking" || f.s.Rollout.May("node1") {
		t.Fatalf("passed before the soak: %+v", f.s.Rollout)
	}
	f.beat("node1", 30_000)
	f.roll("node2", 35_000, verdict(7, "good"))
	r = f.s.Rollout
	if r.Status != "good" || !r.May("node1") || !r.Proven("node2") || r.Proven("node1") {
		t.Fatalf("after the soak: %+v", r)
	}
	f.roll("node1", 40_000, verdict(7, "took"))
	f.roll("node1", 41_000, verdict(7, "good"))
	if f.s.Rollout.Status != "done" {
		t.Fatalf("last wave (no soak) not done: %+v", f.s.Rollout)
	}
}

func TestABadVerdictStopsAndHoldsTheRest(t *testing.T) {
	f := rolloutFleet(t)
	for _, b := range []string{"node1", "node2", "node3"} {
		f.beat(b, 1000)
	}
	f.roll("node1", 1200, RolloutOp{Op: "begin", Release: release(t, 8, relKey, "node1", "node2", "node3")})
	r := f.s.Rollout
	first := r.Waves[0][0]
	f.roll(first, 2000, verdict(8, "took"))
	f.roll(first, 3000, RolloutOp{Op: "verdict", Counter: 8, Verdict: "bad", Reason: "probe: unit x failed"})
	r = f.s.Rollout
	if r.Status != "bad" || !strings.Contains(r.Why, "unit x failed") {
		t.Fatalf("%+v", r)
	}
	for _, b := range []string{"node1", "node2", "node3"} {
		if r.May(b) || r.Proven(b) {
			t.Fatalf("%s may take a stopped release", b)
		}
	}
	if f.roll(first, 4000, verdict(8, "good")) {
		t.Fatal("a verdict after the stop")
	}
}

func TestWave1SilentIsNotProven(t *testing.T) {
	f := rolloutFleet(t)
	delete(f.c.Boxes, "node3")
	f.c.Rules.GoneAfter = 90_000
	f.beat("node1", 1000)
	f.beat("node2", 1000)
	f.roll("node1", 1200, RolloutOp{Op: "begin", Release: release(t, 9, relKey, "node1", "node2")})
	w1 := f.s.Rollout.Waves[0][0]
	other := f.s.Rollout.Waves[1][0]
	// the other keeps heartbeating every 30s; wave 1 says nothing
	for now := int64(30_000); now < 1200+60_000+30_000+60_000; now += 30_000 {
		f.beat(other, now)
	}
	if f.s.Rollout.Status != "soaking" {
		t.Fatalf("stopped before the deadline: %+v", f.s.Rollout)
	}
	f.beat(other, 1200+60_000+30_000+60_000+1000)
	r := f.s.Rollout
	if r.Status != "bad" || !strings.Contains(r.Why, w1+": not proven") {
		t.Fatalf("%+v", r)
	}
}

// the control plane down for ten minutes in the middle of a soak: nobody
// is judged for it, and the soak is watched time after it
func TestAnOutageMovesTheDeadlinesAndRestartsTheSoak(t *testing.T) {
	f := rolloutFleet(t)
	delete(f.c.Boxes, "node3")
	f.beat("node1", 1000)
	f.beat("node2", 1000)
	f.block(1100, f.req("node1", "lease", lease("postgres", "acquire", 0)))
	f.roll("node1", 1200, RolloutOp{Op: "begin", Release: release(t, 10, relKey, "node1", "node2")})
	f.roll("node2", 2000, verdict(10, "took"))
	f.roll("node2", 20_000, verdict(10, "good"))
	// nothing agreed from 20s to 620s: past every deadline
	f.roll("node2", 620_000, verdict(10, "good"))
	r := f.s.Rollout
	if r.Status != "soaking" || r.Paused != 600_000 {
		t.Fatalf("after the outage: %+v", r)
	}
	f.roll("node2", 640_000, verdict(10, "good"))
	if f.s.Rollout.Status != "soaking" {
		t.Fatal("the soak counted time before the outage")
	}
	f.roll("node2", 651_000, verdict(10, "good"))
	if f.s.Rollout.Status != "good" {
		t.Fatalf("not good after 30s watched: %+v", f.s.Rollout)
	}
}

func TestANewerReleaseReplacesTheRollout(t *testing.T) {
	f := rolloutFleet(t)
	delete(f.c.Boxes, "node3")
	f.beat("node1", 1000)
	f.beat("node2", 1000)
	f.roll("node1", 1200, RolloutOp{Op: "begin", Release: release(t, 11, relKey, "node1", "node2")})
	f.roll("node2", 1300, RolloutOp{Op: "begin", Release: release(t, 12, relKey, "node1", "node2")})
	if f.s.Rollout.Counter != 12 {
		t.Fatal("not replaced")
	}
	w1 := f.s.Rollout.Waves[0][0]
	if f.roll(w1, 1400, verdict(11, "took")) {
		t.Fatal("a verdict on the replaced release")
	}
}

// a later wave's box away all through its wave does not stop the rollout:
// it takes the release when it is back
func TestALaterWaveBoxAwayIsExcused(t *testing.T) {
	f := rolloutFleet(t)
	for _, b := range []string{"node1", "node2", "node3"} {
		f.beat(b, 1000)
	}
	f.roll("node1", 1200, RolloutOp{Op: "begin", Release: release(t, 13, relKey, "node1", "node2", "node3")})
	r := f.s.Rollout
	if len(r.Waves) != 2 || len(r.Waves[0]) != 1 {
		t.Fatalf("waves %v", r.Waves)
	}
	w1, rest := r.Waves[0][0], r.Waves[1]
	f.roll(w1, 2000, verdict(13, "took"))
	f.roll(w1, 40_000, verdict(13, "good"))
	if f.s.Rollout.Status != "good" {
		t.Fatalf("%+v", f.s.Rollout)
	}
	here, gone := rest[0], rest[1]
	f.roll(here, 41_000, verdict(13, "took"))
	f.roll(here, 42_000, verdict(13, "good"))
	for now := int64(60_000); now <= 300_000; now += 30_000 {
		f.beat(here, now)
		f.beat(w1, now)
	}
	r = f.s.Rollout
	if r.Status != "done" || len(r.Excused) != 1 || r.Excused[0] != gone {
		t.Fatalf("%+v", r)
	}
	if !r.May(gone) {
		t.Fatal("the excused box may not take it when back")
	}
}

func TestWavesTakeFivePercentThenAQuarter(t *testing.T) {
	f := rolloutFleet(t)
	f.c.Boxes = map[string]Box{}
	var names []string
	for i := 0; i < 40; i++ {
		n := fmt.Sprintf("b%02d", i)
		f.c.Boxes[n] = Box{House: fmt.Sprintf("h%d", i%5), Key: "k" + n}
		names = append(names, n)
	}
	for _, n := range names {
		f.beat(n, 1000)
	}
	f.roll("b00", 1200, RolloutOp{Op: "begin", Release: release(t, 14, relKey, names...)})
	w := f.s.Rollout.Waves
	if len(w) != 3 || len(w[0]) != 2 || len(w[0])+len(w[1]) != 10 || len(w[2]) != 30 {
		t.Fatalf("wave sizes %d %d %d", len(w[0]), len(w[1]), len(w[2]))
	}
	// two houses in a wave of two
	if f.c.Boxes[w[0][0]].House == f.c.Boxes[w[0][1]].House {
		t.Fatalf("wave 1 in one house: %v", w[0])
	}
}

// a release the laptop really signed (the releases branch's current.json)
// checks here as it does in the agent: the same bytes, in Go
func TestARealReleaseChecks(t *testing.T) {
	raw, err := os.ReadFile("testdata/release.json")
	if err != nil {
		t.Fatal(err)
	}
	c := &Config{Identity: &ident.Config{Release: "fdpYPlUGQCA+lolOtDKO1cgc4bM1syI3JX/0hFPthvo="}}
	p, d, err := readRelease(c, string(raw))
	if err != nil || p.Counter == 0 || len(p.Boxes) == 0 || d != agentDigest(t, string(raw)) {
		t.Fatalf("%v %+v", err, p)
	}
	if _, _, err := readRelease(c, strings.Replace(string(raw), `"nar_hash": "sha256-`, `"nar_hash": "sha256-X`, 1)); err == nil {
		t.Fatal("a changed path checked")
	}
}

// what dd-agent computes (release::rollout::digest): sha256 of the payload
// with keys sorted and no whitespace, then the signature's bytes
func agentDigest(t *testing.T, raw string) string {
	t.Helper()
	var top struct {
		Payload   json.RawMessage
		Signature string
	}
	if err := json.Unmarshal([]byte(raw), &top); err != nil {
		t.Fatal(err)
	}
	msg, err := ident.CanonicalValue(top.Payload)
	if err != nil {
		t.Fatal(err)
	}
	sig, _ := base64.StdEncoding.DecodeString(top.Signature)
	h := sha256.Sum256(append(msg, sig...))
	return hex.EncodeToString(h[:])
}

// the release written another way: indented, its keys in another order
func rewritten(t *testing.T, raw string) string {
	t.Helper()
	var top map[string]json.RawMessage
	if err := json.Unmarshal([]byte(raw), &top); err != nil {
		t.Fatal(err)
	}
	var p map[string]any
	if err := json.Unmarshal(top["payload"], &p); err != nil {
		t.Fatal(err)
	}
	pb, _ := json.MarshalIndent(p, "  ", "    ")
	return "\n {\"signature\" : " + string(top["signature"]) + ",\n\t\"payload\":" + string(pb) +
		", \"signer\":" + string(top["signer"]) + "}\n\n"
}

// A box hands in the release re-written: it begins the rollout the agents
// recognise, so it cannot block the release by being first
func TestARewrittenReleaseHasTheDigestAgentsCompute(t *testing.T) {
	f := rolloutFleet(t)
	f.beat("node1", 1000)
	f.beat("node2", 1000)
	good := release(t, 5, relKey, "node1", "node2")
	bad := rewritten(t, good)
	if bad == good || !strings.Contains(bad, "\n\t") {
		t.Fatal("not re-written")
	}
	if !f.roll("node2", 2000, RolloutOp{Op: "begin", Release: bad}) {
		t.Fatal("the re-written release did not begin")
	}
	if f.s.Rollout.Digest != agentDigest(t, good) {
		t.Fatalf("digest %s, the agents compute %s", f.s.Rollout.Digest, agentDigest(t, good))
	}
	// the honest box's begin is the same release: nothing changes
	if !f.roll("node1", 2100, RolloutOp{Op: "begin", Release: good}) || f.s.Rollout.Began != 2000 {
		t.Fatalf("rollout %+v", f.s.Rollout)
	}
}

// A field the release key did not sign is refused at the top, and inside
// the payload it breaks the signature
func TestAReleaseWithAnExtraFieldIsRefused(t *testing.T) {
	f := rolloutFleet(t)
	f.beat("node1", 1000)
	good := release(t, 5, relKey, "node1")
	for _, bad := range []string{
		strings.Replace(good, `{"payload"`, `{"extra":1,"payload"`, 1),
		strings.Replace(good, `{"payload"`, `{"Payload":{},"payload"`, 1),
		strings.Replace(good, `{"payload"`, `{"signer":"x","payload"`, 1),
		strings.Replace(good, `"counter":5`, `"counter":5,"extra":1`, 1),
		strings.Replace(good, `"counter":5`, `"counter":9,"counter":5`, 1),
		strings.Replace(good, `"counter":5`, `"counter":5,"counter":9`, 1),
		strings.Replace(good, `"path":`, `"path":"/nix/store/y","path":`, 1),
		good + " {}",
	} {
		if bad == good {
			t.Fatal("not changed")
		}
		if f.roll("node1", 2000, RolloutOp{Op: "begin", Release: bad}) {
			t.Fatalf("began with %s", bad)
		}
	}
	if f.s.Rollout != nil {
		t.Fatal("a rollout began")
	}
}

// Only a box the release is for begins it
func TestBeginOnlyFromABoxOfTheRelease(t *testing.T) {
	f := rolloutFleet(t)
	f.beat("node1", 1000)
	f.beat("node3", 1000)
	if f.roll("node3", 2000, RolloutOp{Op: "begin", Release: release(t, 5, relKey, "node1")}) {
		t.Fatal("a box outside the release began it")
	}
	if !f.roll("node1", 2000, RolloutOp{Op: "begin", Release: release(t, 5, relKey, "node1")}) {
		t.Fatal("the release's own box could not begin it")
	}
}

// the cross-language vector: release::rollout::digest gives the same hex
// for the same file (servers/system/release/src/rollout.rs)
const (
	vectorRelease = `{"payload":{"boxes":{"node1":{"nar_hash":"sha256-x","path":"/nix/store/x-node1"}},"counter":3,"issued":1,"rev":"r"},"signature":"XPJ3sq51slhHO4CBqoPxLlGwAp6BKkm8tlanu+NBO+EEwOPkP9Li2DG9gIDgtsDnZE7x0PKs33zqlV4DRvoGCA==","signer":"O2onvM62pC1io6jQKm8Nc2UyFXcd4kOmOsBIoYtZ2ik="}`
	vectorDigest  = "161eba379dbae48c31d29f4e371761f4182bfe8a57d208ffbdc64816cc6fabef"
)

func TestDigestVector(t *testing.T) {
	if release(t, 3, relKey, "node1") != vectorRelease {
		t.Fatal("the vector is not this release")
	}
	p, d, err := readRelease(&Config{Identity: &ident.Config{Release: relPub()}}, vectorRelease)
	if err != nil || p.Counter != 3 || d != vectorDigest || agentDigest(t, vectorRelease) != vectorDigest {
		t.Fatalf("%v %s", err, d)
	}
}

// A key twice in the payload: the canonical form keeps the last, so the
// signature still verifies; the file is refused for the key, not for that
func TestADuplicatedCounterIsRefused(t *testing.T) {
	c := &Config{Identity: &ident.Config{Release: relPub()}}
	good := release(t, 5, relKey, "node1")
	for _, bad := range []string{
		strings.Replace(good, `"counter":5`, `"counter":9,"counter":5`, 1),
		strings.Replace(good, `"counter":5`, `"counter":5,"counter":9`, 1),
	} {
		if _, _, err := readRelease(c, bad); err == nil || !strings.Contains(err.Error(), "twice") {
			t.Fatalf("%s: %v", bad, err)
		}
	}
}
