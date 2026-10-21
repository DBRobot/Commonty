package state

import (
	"commonty/control/state/attest"
	"commonty/control/state/ident"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/json"
	"encoding/pem"
	"errors"
	"math/big"
	"os"
	"strings"
	"testing"
	"time"
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

// the swtpm vector's box, joining: its join carries Begin; it is Joining
// until the box voters' challenges are answered, then joined
type vector struct {
	Config     attest.Config      `json:"config"`
	Box        string             `json:"box"`
	Key        string             `json:"key"`
	At         int64              `json:"at"`
	Begin      attest.Begin       `json:"begin"`
	Challenges []attest.Challenge `json:"challenges"`
	Response   attest.Response    `json:"response"`
}

func loadVector(t *testing.T) (*fleet, vector) {
	t.Helper()
	raw, err := os.ReadFile("attest/testdata/swtpm-vector.json")
	if err != nil {
		t.Fatal(err)
	}
	var v vector
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	if name, _, _ := JoinName(v.Key); name != v.Box {
		t.Fatalf("the vector's box %s is not its key's name %s", v.Box, name)
	}
	f := newFleet(t)
	f.c.Validators = []Validator{{Name: "node1"}, {Name: "node2"}, {Name: "witness"}}
	f.c.Attest = &v.Config
	return f, v
}

func (v vector) join(c *ident.Claim, seq uint64) Request {
	b, _ := json.Marshal(JoinReq{Key: v.Key, Recipient: "age1x", House: "h", Facts: enough, Claim: c, Attest: &v.Begin})
	return Request{Kind: "join", Box: v.Box, Seq: uint64(v.At) + seq, Body: b, Sig: "ok:" + v.Key}
}

func (v vector) answer(seq uint64) Request {
	b, _ := json.Marshal(v.Response)
	return Request{Kind: "attest-response", Box: v.Box, Seq: uint64(v.At) + seq, Body: b, Sig: "ok:" + v.Key}
}

func TestABoxJoinsOnlyOnceItsTPMHasProvedIt(t *testing.T) {
	f, v := loadVector(t)
	c := claimFor(t, v.Key)
	if f.block(v.At, v.join(c, 1)) != 1 {
		t.Fatalf("the join refused: %v", f.s.checkJoin(f.c, fakeVerify, v.join(c, 1), v.At))
	}
	if f.s.Joined[v.Box] != nil || f.s.Joining[v.Box] == nil || f.s.Attest.Pending[v.Box] == nil {
		t.Fatal("joined before its proof")
	}
	// nothing of its join is in the directory log yet: no member can
	// enlist it, and its Begin is kept once, by the attestation
	if len(f.s.Directory) != 0 || f.s.Joining[v.Box].Req.Attest != nil {
		t.Fatalf("%+v", f.s.Directory)
	}
	// it says nothing else meanwhile, nor joins again
	hb := Request{Kind: "heartbeat", Box: v.Box, Seq: uint64(v.At) + 2, Body: []byte(`{"facts":{}}`), Sig: "ok:" + v.Key}
	if f.block(v.At, hb, v.join(c, 3)) != 0 {
		t.Fatal("a box still proving its TPM was heard")
	}
	// an answer before a majority of box voters challenged is early, not a refusal
	if f.block(v.At+100, f.req("node1", "attest-challenge", v.Challenges[0]), v.answer(4)) != 1 || f.s.Refused[v.Box] != nil {
		t.Fatal("an early answer was judged")
	}
	if f.block(v.At+200, f.req("node2", "attest-challenge", v.Challenges[1]), v.answer(5)) != 2 {
		t.Fatal("the answer refused")
	}
	j := f.s.Joined[v.Box]
	if j == nil || j.Ticket != c.ID || j.Unattested || f.s.Joining != nil || f.s.Attested(v.Box) == nil {
		t.Fatalf("not joined on its proof: %+v", j)
	}
	if len(f.s.Directory) != 1 || !strings.Contains(string(f.s.Directory[0].Item), `"joined"`) {
		t.Fatalf("%+v", f.s.Directory)
	}
	if info, _ := f.s.IdentBoxes()(v.Box); info.Unattested {
		t.Fatal("a box that proved its TPM counts as unproven")
	}
}

func TestAJoinIsRefusedForWhatTheBoxIsAndItsMemberToldWhy(t *testing.T) {
	refusedFor := func(t *testing.T, change func(*attest.Config), why string) *fleet {
		t.Helper()
		f, v := loadVector(t)
		cfg := v.Config
		change(&cfg)
		f.c.Attest = &cfg
		c := claimFor(t, v.Key)
		if f.block(v.At, v.join(c, 1)) != 0 || f.s.Joining[v.Box] != nil {
			t.Fatal("joined")
		}
		r := f.s.Refused[v.Box]
		if r == nil || r.Why != why || r.Ticket != c.ID {
			t.Fatalf("want %q, got %+v", why, r)
		}
		// the identity judge hears it, for the ticket's member
		want := `{"refused":{"block":"` + c.Block + `","key":"` + v.Key + `","ticket":"` + c.ID + `","why":"` + why + `"}}`
		if len(f.s.Directory) != 1 || string(f.s.Directory[0].Item) != want {
			t.Fatalf("%+v", f.s.Directory)
		}
		// the box asks again every minute: kept once a day, not once a minute
		f.block(v.At+60_000, v.join(c, 2))
		if len(f.s.Directory) != 1 {
			t.Fatal("the same refusal logged twice")
		}
		// and the box can say the same before it asks
		j := JoinReq{Key: v.Key, Facts: enough, Attest: &v.Begin}
		var rf *Refusal
		if err := JoinRefusal(f.c, nil, v.Box, j, v.At); !errors.As(err, &rf) || rf.Why != why {
			t.Fatalf("the box's own check: %v", err)
		}
		return f
	}
	t.Run("an EK not from its maker's CA", func(t *testing.T) {
		refusedFor(t, func(c *attest.Config) {
			k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
			tmpl := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "not the maker"}, NotBefore: time.Unix(0, 0), NotAfter: time.Unix(1<<33, 0), IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign}
			der, _ := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &k.PublicKey, k)
			roots := map[string][]string{}
			for vendor := range c.Roots {
				roots[vendor] = []string{string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: der}))}
			}
			c.Roots = roots
		}, "its TPM is not one a TPM maker certified")
	})
	t.Run("a software TPM outside tests", func(t *testing.T) {
		refusedFor(t, func(c *attest.Config) { c.Test = nil }, "its TPM is not one a TPM maker certified")
	})
	t.Run("a PCR 4 not the release's", func(t *testing.T) {
		refusedFor(t, func(c *attest.Config) {
			b := *c.Boot
			b.Kernels = []string{strings.Repeat("00", 32)}
			c.Boot = &b
		}, "it did not start the fleet's own system")
	})
	t.Run("below the minimum", func(t *testing.T) {
		f, v := loadVector(t)
		b, _ := json.Marshal(JoinReq{Key: v.Key, Recipient: "age1x", House: "h", Facts: map[string]float64{"tpm_major": 2}, Attest: &v.Begin})
		f.block(v.At, Request{Kind: "join", Box: v.Box, Seq: uint64(v.At) + 1, Body: b, Sig: "ok:" + v.Key})
		if r := f.s.Refused[v.Box]; r == nil || !strings.HasPrefix(r.Why, "needs ") || len(f.s.Directory) != 0 {
			t.Fatalf("%+v", r)
		}
	})
	t.Run("no proof at all", func(t *testing.T) {
		f, v := loadVector(t)
		b, _ := json.Marshal(JoinReq{Key: v.Key, Recipient: "age1x", House: "h", Facts: enough})
		f.block(v.At, Request{Kind: "join", Box: v.Box, Seq: uint64(v.At) + 1, Body: b, Sig: "ok:" + v.Key})
		if r := f.s.Refused[v.Box]; r == nil || r.Why != "it sent no proof of its TPM" {
			t.Fatalf("%+v", r)
		}
	})
	t.Run("signed by another key: not even kept", func(t *testing.T) {
		f, v := loadVector(t)
		cfg := v.Config
		cfg.Test = nil
		f.c.Attest = &cfg
		r := v.join(nil, 1)
		r.Sig = "ok:k1"
		f.block(v.At, r)
		if f.s.Refused != nil {
			t.Fatal("a join not signed by its key was kept")
		}
	})
}

func TestAProofThatDoesNotHoldOrNeverComesIsRefused(t *testing.T) {
	f, v := loadVector(t)
	if f.block(v.At, v.join(nil, 1)) != 1 {
		t.Fatal("the join refused")
	}
	f.block(v.At+100, f.req("node1", "attest-challenge", v.Challenges[0]), f.req("node2", "attest-challenge", v.Challenges[1]))
	bad := v.Response
	bad.QuoteSig = bad.CertifySig
	b, _ := json.Marshal(bad)
	f.block(v.At+200, Request{Kind: "attest-response", Box: v.Box, Seq: uint64(v.At) + 2, Body: b, Sig: "ok:" + v.Key})
	if f.s.Joined[v.Box] != nil || f.s.Joining != nil || f.s.Boxes[v.Box] != nil || f.s.Attest.Pending[v.Box] != nil {
		t.Fatal("a box whose proof did not hold is still there")
	}
	if r := f.s.Refused[v.Box]; r == nil || r.Why != "it could not prove its TPM" {
		t.Fatalf("%+v", r)
	}
	// it starts again from its join; this time it never answers
	if f.block(v.At+300, v.join(nil, 3)) != 1 {
		t.Fatal("a box may try again")
	}
	f.block(v.At + 300 + attest.TTL)
	if f.s.Joining != nil || f.s.Boxes[v.Box] != nil {
		t.Fatal("a join never proved is still there")
	}
	if r := f.s.Refused[v.Box]; r == nil || r.Why != "it did not finish proving its TPM within an hour" {
		t.Fatalf("%+v", r)
	}
	// a week on, forgotten
	f.block(v.At + 300 + attest.TTL + RefusedFor)
	if f.s.Refused != nil {
		t.Fatal("a refusal kept past its week")
	}
}

func TestRefusalsAreBoundedAndJunkCannotSpendAMembersShare(t *testing.T) {
	f := newFleet(t)
	f.c.Attest = &attest.Config{}
	junk := func(i int, now int64) {
		k := strangerKey(i)
		name, _, _ := JoinName(k)
		b, _ := json.Marshal(JoinReq{Key: k, Recipient: "age1x", House: "h", Facts: enough})
		f.block(now, Request{Kind: "join", Box: name, Seq: uint64(now), Body: b, Sig: "ok:" + k})
	}
	for i := range 3 * StrangersRefusedPerHour {
		junk(i, 1000)
	}
	if len(f.s.Refused) != StrangersRefusedPerHour || len(f.s.Directory) != 0 {
		t.Fatalf("%d ticketless refusals kept in an hour, %d logged", len(f.s.Refused), len(f.s.Directory))
	}
	// a member's box is refused, and told, whatever the junk did
	k := strangerKey(100)
	name, _, _ := JoinName(k)
	c := claimFor(t, k)
	b, _ := json.Marshal(JoinReq{Key: k, Recipient: "age1x", House: "h", Facts: enough, Claim: c})
	f.block(1000, Request{Kind: "join", Box: name, Seq: 1000, Body: b, Sig: "ok:" + k})
	if r := f.s.Refused[name]; r == nil || r.Ticket != c.ID || len(f.s.Directory) != 1 {
		t.Fatalf("%+v", r)
	}
	// junk, however long, never pushes a member's refusal out
	for i := range MaxRefused * 2 {
		junk(1000+i, int64(2+i)*3_600_000)
	}
	if f.s.Refused[name] == nil {
		t.Fatal("a member's refusal pushed out by junk")
	}
}

func TestAnOldSignedJoinReplayedIsNotJudgedAgain(t *testing.T) {
	f, v := loadVector(t)
	cfg := v.Config
	cfg.Test = nil // a software TPM: refused
	f.c.Attest = &cfg
	c := claimFor(t, v.Key)
	old := v.join(c, 1)
	f.block(v.At, old)
	if len(f.s.Directory) != 1 {
		t.Fatal("not refused")
	}
	// replayed a day on, when the same refusal would be kept again: stale
	if err := f.s.Check(f.c, fakeVerify, old, v.At+refusedAgain+1); !errors.Is(err, ErrStale) {
		t.Fatalf("an old join: %v", err)
	}
	f.block(v.At+refusedAgain+1, old)
	// and within its ten minutes, the same request again: at or below the
	// Seq the refusal kept
	f.block(v.At+1000, old)
	if len(f.s.Directory) != 1 {
		t.Fatalf("a replay logged again: %d", len(f.s.Directory))
	}
	// a fresh one from the box is judged as ever
	fresh := v.join(c, refusedAgain+2)
	f.block(v.At+refusedAgain+2, fresh)
	if len(f.s.Directory) != 2 {
		t.Fatal("a fresh join was not judged")
	}
}
