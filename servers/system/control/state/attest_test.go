package state

import (
	"commonty/control/state/attest"
	"encoding/json"
	"os"
	"testing"
)

// the swtpm attestation (state/attest/testdata) through agreed requests:
// each box voter's challenge, then the attesting box's answer, signed by
// the join key it started with
func TestAnAttestationThroughTheAgreedState(t *testing.T) {
	raw, err := os.ReadFile("attest/testdata/swtpm-vector.json")
	if err != nil {
		t.Fatal(err)
	}
	var v struct {
		Config     attest.Config      `json:"config"`
		Box        string             `json:"box"`
		Key        string             `json:"key"`
		At         int64              `json:"at"`
		Begin      attest.Begin       `json:"begin"`
		Challenges []attest.Challenge `json:"challenges"`
		Response   attest.Response    `json:"response"`
	}
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	f := newFleet(t)
	f.c.Validators = []Validator{{Name: "node1"}, {Name: "node2"}, {Name: "witness"}}
	f.c.Attest = &v.Config
	if err := f.s.CheckAttestOpen(f.c, v.Box, v.Key, v.Begin, v.At); err != nil {
		t.Fatal(err)
	}
	f.s.AttestOpen(f.c, v.Box, v.Key, v.Begin, v.At)

	answer := func() Request {
		b, _ := json.Marshal(v.Response)
		return Request{Kind: "attest-response", Box: v.Box, Seq: 1, Body: b, Sig: "ok:" + v.Key}
	}
	// node3 votes for nothing; node1 alone is not a majority of two
	if f.block(v.At, f.req("node3", "attest-challenge", v.Challenges[0])) != 0 {
		t.Fatal("a challenge from a box that does not vote")
	}
	if f.block(v.At, f.req("node1", "attest-challenge", v.Challenges[0]), answer()) != 1 {
		t.Fatal("want node1's challenge in and the answer out")
	}
	if f.block(v.At+1000, f.req("node2", "attest-challenge", v.Challenges[1])) != 1 {
		t.Fatal("node2's challenge refused")
	}
	// signed by another key: refused
	wrong := answer()
	wrong.Sig = "ok:k1"
	if f.block(v.At+2000, wrong) != 0 {
		t.Fatal("an answer not signed by the join key")
	}
	if f.block(v.At+2000, answer()) != 1 {
		t.Fatal("the answer refused")
	}
	rec := f.s.Attested(v.Box)
	if rec == nil || !rec.SecureBoot || f.s.Attest.EKs[rec.EK] != v.Box {
		t.Fatalf("not attested: %+v", rec)
	}
	// and the agreed state says the same after a round trip through json
	out, _ := json.Marshal(f.s)
	var back State
	if err := json.Unmarshal(out, &back); err != nil || back.Attested(v.Box) == nil {
		t.Fatalf("lost in json: %v", err)
	}
}
