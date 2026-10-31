//go:build testswitches

package state

import (
	"commonty/control/state/attest"
	"commonty/control/state/ident"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/base64"
	"encoding/json"
	"encoding/pem"
	"errors"
	"math/big"
	"os"
	"strings"
	"testing"
	"time"
)

// a fleet where attestation is in force: version 3 agreed
func attestFleet(t *testing.T) *fleet {
	f := newFleet(t)
	f.s.Machine = 3
	return f
}

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
	f := attestFleet(t)
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
	f := attestFleet(t)
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
		n := ticketMember(t, f, ticketDevice(), v.At-1000)
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
		if len(f.s.Directory) != n+1 || string(f.s.Directory[n].Item) != want {
			t.Fatalf("%+v", f.s.Directory)
		}
		// the box asks again every minute: kept once a day, not once a minute
		f.block(v.At+60_000, v.join(c, 2))
		if len(f.s.Directory) != n+1 {
			t.Fatal("the same refusal logged twice")
		}
		// and the box can say the same before it asks
		j := JoinReq{Key: v.Key, Facts: enough, Attest: &v.Begin}
		var rf *Refusal
		if err := JoinRefusal(f.c, f.s, v.Box, j, v.At); !errors.As(err, &rf) || rf.Why != why {
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
	f := attestFleet(t)
	f.c.Attest = &attest.Config{}
	n := ticketMember(t, f, ticketDevice(), 500)
	junk := func(i int, now int64) {
		k := strangerKey(i)
		name, _, _ := JoinName(k)
		b, _ := json.Marshal(JoinReq{Key: k, Recipient: "age1x", House: "h", Facts: enough})
		f.block(now, Request{Kind: "join", Box: name, Seq: uint64(now), Body: b, Sig: "ok:" + k})
	}
	for i := range 3 * StrangersRefusedPerHour {
		junk(i, 1000)
	}
	if len(f.s.Refused) != StrangersRefusedPerHour || len(f.s.Directory) != n {
		t.Fatalf("%d ticketless refusals kept in an hour, %d logged", len(f.s.Refused), len(f.s.Directory))
	}
	// a member's box is refused, and told, whatever the junk did
	k := strangerKey(100)
	name, _, _ := JoinName(k)
	c := claimFor(t, k)
	b, _ := json.Marshal(JoinReq{Key: k, Recipient: "age1x", House: "h", Facts: enough, Claim: c})
	f.block(1000, Request{Kind: "join", Box: name, Seq: 1000, Body: b, Sig: "ok:" + k})
	if r := f.s.Refused[name]; r == nil || r.Ticket != c.ID || len(f.s.Directory) != n+1 {
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
	n := ticketMember(t, f, ticketDevice(), v.At-1000)
	c := claimFor(t, v.Key)
	old := v.join(c, 1)
	f.block(v.At, old)
	if len(f.s.Directory) != n+1 {
		t.Fatal("not refused")
	}
	// replayed a day on, when the same refusal would be kept again: stale
	if err := f.s.Check(f.c, fakeVerify, old, v.At+RefusedAgain+1); !errors.Is(err, ErrStale) {
		t.Fatalf("an old join: %v", err)
	}
	f.block(v.At+RefusedAgain+1, old)
	// and within its ten minutes, the same request again: at or below the
	// Seq the refusal kept
	f.block(v.At+1000, old)
	if len(f.s.Directory) != n+1 {
		t.Fatalf("a replay logged again: %d", len(f.s.Directory))
	}
	// a fresh one from the box is judged as ever
	fresh := v.join(c, RefusedAgain+2)
	f.block(v.At+RefusedAgain+2, fresh)
	if len(f.s.Directory) != n+2 {
		t.Fatal("a fresh join was not judged")
	}
}

// A ticket's claim is the joining box's word: its refusal is a member's -
// in the ticketed bound, one a ticket and member, told to the identity
// judge - only once the identity state holds the ticket signed by a
// current device of the member it names. A ticket any other key signed is
// a stranger's refusal, however many there are.
func TestAForgedTicketsRefusalIsAStrangers(t *testing.T) {
	f := attestFleet(t)
	f.c.Attest = &attest.Config{}
	other := base64.StdEncoding.EncodeToString(make([]byte, ed25519.PublicKeySize))
	n := ticketMember(t, f, other, 500)
	join := func(i int, now int64) string {
		k := strangerKey(i)
		name, _, _ := JoinName(k)
		f.block(now, ticketJoin(k, claimFor(t, k), uint64(now)))
		return name
	}
	for i := range 3 * StrangersRefusedPerHour {
		join(i, 1000)
	}
	if len(f.s.Refused) != StrangersRefusedPerHour || f.s.RefuseCount != 0 || len(f.s.Directory) != n {
		t.Fatalf("%d forged refusals kept, %d counted as ticketed, %d logged", len(f.s.Refused), f.s.RefuseCount, len(f.s.Directory)-n)
	}
	for b, r := range f.s.Refused {
		if r.Ticket != "" || r.Owner != "" {
			t.Fatalf("%s: a forged ticket kept as one: %+v", b, r)
		}
	}
	// nor with no identity state at all
	g := attestFleet(t)
	g.c.Attest = &attest.Config{}
	k := strangerKey(1)
	name, _, _ := JoinName(k)
	g.block(1000, ticketJoin(k, claimFor(t, k), 1000))
	if r := g.s.Refused[name]; r == nil || r.Ticket != "" || g.s.StrangerCount != 1 || g.s.RefuseCount != 0 {
		t.Fatalf("%+v", r)
	}
}

func TestARealTicketsRefusalIsCountedForItsOwner(t *testing.T) {
	f := attestFleet(t)
	f.c.Attest = &attest.Config{}
	tk := ticketOf(t)
	n := ticketMember(t, f, ticketDevice(), 500)
	// the stranger bucket full first
	for i := range StrangersRefusedPerHour {
		k := strangerKey(100 + i)
		name, _, _ := JoinName(k)
		b, _ := json.Marshal(JoinReq{Key: k, Recipient: "age1x", House: "h", Facts: enough})
		f.block(1000, Request{Kind: "join", Box: name, Seq: 1000, Body: b, Sig: "ok:" + k})
	}
	k := strangerKey(1)
	name, _, _ := JoinName(k)
	c := claimFor(t, k)
	f.block(1000, ticketJoin(k, c, 1000))
	r := f.s.Refused[name]
	if r == nil || r.Ticket != c.ID || r.Owner != tk.User || f.s.RefuseCount != 1 || len(f.s.Directory) != n+1 {
		t.Fatalf("%+v, %d ticketed, %d logged", r, f.s.RefuseCount, len(f.s.Directory)-n)
	}
	// the same ticket with another box, within the hour: one a ticket
	k2 := strangerKey(2)
	name2, _, _ := JoinName(k2)
	f.block(2000, ticketJoin(k2, claimFor(t, k2), 2000))
	if f.s.Refused[name2] != nil || f.s.RefuseCount != 1 {
		t.Fatalf("a second refusal on one ticket in an hour: %+v", f.s.Refused[name2])
	}
}

// Below version 3, attestation is not there: what a block decides and the
// state it leaves are version 2's, whatever the config says
func TestBelowVersion3NothingOfAttestationHappens(t *testing.T) {
	f := newFleet(t)
	f.s.Machine = 2
	_, v := loadVector(t)
	f.c.Attest = &v.Config
	f.c.Validators = []Validator{{Name: "node1"}, {Name: "node2"}, {Name: "witness"}}
	// a join with no proof, as ever, and with a sequence ten minutes off
	if f.block(v.At, joinReq(strangerKey(1), 1)) != 1 {
		t.Fatal("a join with no proof refused below version 3")
	}
	name, _, _ := JoinName(strangerKey(1))
	if f.s.Joined[name] == nil || f.s.Joining != nil || f.s.Attest != nil {
		t.Fatalf("%+v", f.s)
	}
	// one below the minimum is refused and leaves no trace
	k := strangerKey(2)
	nm, _, _ := JoinName(k)
	b, _ := json.Marshal(JoinReq{Key: k, Recipient: "age1x", House: "h"})
	if f.block(v.At+1, Request{Kind: "join", Box: nm, Seq: 1, Body: b, Sig: "ok:" + k}) != 0 || f.s.Refused != nil {
		t.Fatalf("%+v", f.s.Refused)
	}
	// no such requests, from a voter or a joined box
	for _, kind := range []string{"attest-challenge", "attest-response", "attest-begin"} {
		if err := f.s.Check(f.c, fakeVerify, f.req("node1", kind, map[string]string{}), v.At+2); err == nil || !strings.Contains(err.Error(), "no such request") {
			t.Fatalf("%s: %v", kind, err)
		}
	}
	// a directory item saying "refused" is as any other
	if !json.Valid([]byte(`{"refused":{}}`)) || f.block(v.At+3, f.req("node1", "directory", map[string]any{"refused": map[string]string{}})) != 1 {
		t.Fatal("refused below version 3")
	}
	// a box that joined with no proof is no less a box to its member
	if info, _ := f.s.IdentBoxes()(name); info.Unattested {
		t.Fatal("unproven below version 3")
	}
	raw, _ := json.Marshal(f.s)
	for _, field := range []string{`"attest"`, `"joining"`, `"refused":{"`, `"refuseWindow"`, `"strangerWindow"`, `"unattested"`} {
		if strings.Contains(string(raw), field) && field != `"refused":{"` {
			t.Fatalf("the state holds %s below version 3", field)
		}
	}
	if f.s.Refused != nil {
		t.Fatal("a refusal kept below version 3")
	}
}

// A joined box's proof counts for what it runs only while it is current;
// its member's access rests on its having proved once
func TestAProofGoesStaleForWhatItRunsNeverForAccess(t *testing.T) {
	f, v := loadVector(t)
	f.c.Validators = []Validator{{Name: "node1"}, {Name: "node2"}, {Name: "witness"}}
	f.s.Joined = map[string]*Joined{v.Box: {Key: v.Key}}
	f.s.Boxes[v.Box] = &BoxState{LastSeen: v.At}
	if f.s.Proved(v.Box, v.At) {
		t.Fatal("proved with no record")
	}
	if info, _ := f.s.IdentBoxes()(v.Box); !info.Unattested {
		t.Fatal("no record: unproven for access")
	}
	f.s.Attest = attest.New()
	rec := &attest.Record{At: v.At}
	f.s.Attest.Done[v.Box] = rec
	if !f.s.Proved(v.Box, v.At+attest.ProofFresh) || f.s.Proved(v.Box, v.At+attest.ProofFresh+1) {
		t.Fatal("not current for exactly ProofFresh")
	}
	rec.Failed = "it did not start the fleet's own system"
	if f.s.Proved(v.Box, v.At) {
		t.Fatal("a failed proof counts")
	}
	if info, _ := f.s.IdentBoxes()(v.Box); info.Unattested {
		t.Fatal("a stale proof cost its member's access")
	}
	if f.s.Proved("jother", v.At) {
		t.Fatal("a box that has not joined")
	}
}

// A joined box proves itself again through agreed requests. A Begin that
// does not hold keeps the box, marks its proof, spends its sequence and
// waits out the hour
func TestAJoinedBoxProvingAgainThroughTheAgreedState(t *testing.T) {
	f, v := loadVector(t)
	f.c.Validators = []Validator{{Name: "node1"}, {Name: "node2"}, {Name: "witness"}}
	f.s.Joined = map[string]*Joined{v.Box: {Key: v.Key}}
	f.s.Boxes[v.Box] = &BoxState{LastSeen: v.At}
	begin := func(seq uint64, at int64) Request {
		b, _ := json.Marshal(v.Begin)
		return Request{Kind: "attest-begin", Box: v.Box, Seq: seq, Body: b, Sig: "ok:" + v.Key}
	}
	if f.block(v.At, begin(1, v.At)) != 1 || f.s.Attest.Pending[v.Box] == nil {
		t.Fatal("a joined box's Begin refused")
	}
	if f.block(v.At+1, begin(2, v.At+1)) != 0 {
		t.Fatal("a second Begin while the first is under way")
	}
	if f.block(v.At+2, f.req("node1", "attest-challenge", v.Challenges[0])) != 1 || f.block(v.At+3, f.req("node2", "attest-challenge", v.Challenges[1])) != 1 {
		t.Fatal("challenges refused")
	}
	b, _ := json.Marshal(v.Response)
	if f.block(v.At+4, Request{Kind: "attest-response", Box: v.Box, Seq: 3, Body: b, Sig: "ok:" + v.Key}) != 1 {
		t.Fatal("the answer refused")
	}
	if !f.s.Proved(v.Box, v.At+5) || f.s.Attest.Pending[v.Box] != nil {
		t.Fatalf("%+v", f.s.Attest)
	}
	// its release moves on and it is in another boot: the box stays, its
	// proof no longer counts
	moved := *f.c.Attest
	mb := *moved.Boot
	mb.Kernels = []string{strings.Repeat("00", 32)}
	moved.Boot = &mb
	f.c.Attest = &moved
	f.s.Attest.Done[v.Box].PCRs["4"] = strings.Repeat("00", 32)
	at := v.At + attest.RetryEvery + 10
	if f.block(at, begin(4, at)) != 0 {
		t.Fatal("a boot not the release's")
	}
	if f.s.Joined[v.Box] == nil || f.s.Proved(v.Box, at) || f.s.Attest.Done[v.Box].Failed == "" {
		t.Fatalf("%+v", f.s.Attest.Done[v.Box])
	}
	if f.s.Boxes[v.Box].Seq != 4 {
		t.Fatalf("its sequence not used: %d", f.s.Boxes[v.Box].Seq)
	}
	if f.block(at+1, begin(5, at+1)) != 0 {
		t.Fatal("tried again within the hour")
	}
	if f.s.Refused[v.Box] == nil {
		t.Fatal("no reason kept")
	}
}

// A box that proves it holds a TPM another joined box was on takes its place
func TestSettingAMachineUpAgainFreesItsTPM(t *testing.T) {
	f, v := loadVector(t)
	f.c.Validators = []Validator{{Name: "node1"}, {Name: "node2"}, {Name: "witness"}}
	ek := func() string {
		c, err := attest.VerifyBegin(f.c.Attest, v.Begin, v.Key, time.UnixMilli(v.At).UTC())
		if err != nil {
			t.Fatal(err)
		}
		return c.EKHash
	}()
	f.s.Attest = attest.New()
	f.s.Attest.EKs[ek] = "jold"
	f.s.Attest.Done["jold"] = &attest.Record{EK: ek, At: v.At}
	f.s.Joined = map[string]*Joined{"jold": {Key: "old", At: v.At}}
	f.s.Boxes["jold"] = &BoxState{LastSeen: v.At}
	if f.block(v.At, v.join(nil, 1)) != 1 {
		t.Fatalf("the same machine, set up again, is refused: %+v", f.s.Refused)
	}
	if f.s.Joined["jold"] == nil {
		t.Fatal("the old box went before any proof")
	}
	if f.block(v.At+1, f.req("node1", "attest-challenge", v.Challenges[0])) != 1 || f.block(v.At+2, f.req("node2", "attest-challenge", v.Challenges[1])) != 1 {
		t.Fatal("challenges refused")
	}
	b, _ := json.Marshal(v.Response)
	if f.block(v.At+3, Request{Kind: "attest-response", Box: v.Box, Seq: uint64(v.At) + 2, Body: b, Sig: "ok:" + v.Key}) != 1 {
		t.Fatal("the answer refused")
	}
	if f.s.Joined[v.Box] == nil || f.s.Joined["jold"] != nil || f.s.Boxes["jold"] != nil || f.s.Attest.EKs[ek] != v.Box {
		t.Fatalf("joined %+v, eks %+v", f.s.Joined, f.s.Attest.EKs)
	}
}
