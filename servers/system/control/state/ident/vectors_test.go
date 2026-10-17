package ident

// Held to bytes and verdicts the Rust code made (client/identity/tests/
// vectors.rs wrote testdata/vectors.json).

import (
	"crypto/sha256"
	"encoding/json"
	"os"
	"strings"
	"testing"
)

type vectors struct {
	Release string `json:"release"`
	V1      struct {
		Entries []struct {
			What      string          `json:"what"`
			Canonical string          `json:"canonical"`
			Challenge string          `json:"challenge"`
			Signed    json.RawMessage `json:"signed"`
			Logged    json.RawMessage `json:"logged"`
			MemberID  string          `json:"member_id"`
			Previous  *int            `json:"previous"`
			Root      string          `json:"root"`
		} `json:"entries"`
		Invite struct {
			Signed SignedInvite    `json:"signed"`
			Bytes  string          `json:"bytes"`
			Logged json.RawMessage `json:"logged"`
		} `json:"invite"`
		ProofBytes string `json:"proof_bytes"`
		Leave      struct {
			Signed    SignedLeave     `json:"signed"`
			Challenge string          `json:"challenge"`
			Logged    json.RawMessage `json:"logged"`
			OnFile    int             `json:"on_file"`
		} `json:"leave"`
		Ack struct {
			Logged json.RawMessage `json:"logged"`
			Hash   string          `json:"hash"`
		} `json:"ack"`
		ItemHash string `json:"item_hash"`
	} `json:"v1"`
	V2 map[string]struct {
		Envelope Envelope `json:"envelope"`
		Key      string   `json:"key"`
		Recovery string   `json:"recovery"`
		Root     string   `json:"root"`
		Passkey  *Passkey `json:"passkey"`
	} `json:"v2"`
}

func load(t *testing.T) *vectors {
	t.Helper()
	raw, err := os.ReadFile("testdata/vectors.json")
	if err != nil {
		t.Fatal(err)
	}
	var v vectors
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	return &v
}

func unb64(t *testing.T, s string) []byte {
	t.Helper()
	b, err := b64.DecodeString(s)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func TestV1BytesAreRustsBytes(t *testing.T) {
	v := load(t)
	var signed []*Signed
	for _, e := range v.V1.Entries {
		// read from the log's form (keys sorted), rebuilt to serde's
		var item struct {
			Entry json.RawMessage `json:"entry"`
		}
		if err := json.Unmarshal(e.Logged, &item); err != nil {
			t.Fatal(err)
		}
		s, err := FromV1(item.Entry)
		if err != nil {
			t.Fatalf("%s: %v", e.What, err)
		}
		got, err := s.Entry.Canonical()
		if err != nil {
			t.Fatalf("%s: %v", e.What, err)
		}
		if string(got) != string(unb64(t, e.Canonical)) {
			t.Fatalf("%s: canonical bytes differ\n got %s\nwant %s", e.What, got, unb64(t, e.Canonical))
		}
		if e.Challenge != "" {
			c, _ := s.challenge()
			if b64.EncodeToString(c) != e.Challenge {
				t.Fatalf("%s: challenge differs", e.What)
			}
		}
		if e.Root != "" && PasskeyRoot(s.Entry.Passkeys[0].ID, s.Entry.Passkeys[0].Cred) != e.Root {
			t.Fatalf("%s: the passkey root's digest differs", e.What)
		}
		if e.MemberID != "" && MemberID(s.Entry.Root) != e.MemberID {
			t.Fatalf("%s: member id differs", e.What)
		}
		var prev *Signed
		if e.Previous != nil {
			prev = signed[*e.Previous]
		}
		if err := Accept(prev, s); err != nil {
			t.Fatalf("%s: refused: %v", e.What, err)
		}
		if s.Entry.Grant != nil {
			if err := VerifyGrant(&s.Entry, v.Release); err != nil {
				t.Fatalf("%s: grant: %v", e.What, err)
			}
			// the code's proof is over the bytes Rust writes
			g := s.Entry.Grant
			if b64.EncodeToString(ProofBytes(g.Invite.Invite.PublicKey, s.Entry.Root, g.Redeemed)) != v.V1.ProofBytes {
				t.Fatalf("%s: proof bytes differ", e.What)
			}
		}
		signed = append(signed, s)
	}
	// a tampered entry is refused
	bad := *signed[0]
	bad.Entry.Version++
	if bad.Verify() == nil {
		t.Fatal("a changed entry still verified")
	}
	// the recovery without its paper key's signature is refused
	nopaper := *signed[1]
	nopaper.RecoverySig = nil
	if Accept(signed[0], &nopaper) == nil {
		t.Fatal("a new root without the paper key was taken")
	}
	// the invite's bytes and signature
	if string(v.V1.Invite.Signed.Invite.Bytes()) != string(unb64(t, v.V1.Invite.Bytes)) {
		t.Fatal("invite bytes differ")
	}
	if err := VerifyInvite(&v.V1.Invite.Signed, v.Release); err != nil {
		t.Fatal(err)
	}
	// the leave, against the entry on file
	if b64.EncodeToString(LeaveChallenge(&v.V1.Leave.Signed.Leave)) != v.V1.Leave.Challenge {
		t.Fatal("leave challenge differs")
	}
	if err := VerifyLeave(&v.V1.Leave.Signed, &signed[v.V1.Leave.OnFile].Entry); err != nil {
		t.Fatal(err)
	}
	// the gate's item hash
	if h := ItemHash(v.V1.Ack.Logged); h != v.V1.Ack.Hash {
		t.Fatalf("ack hash %s, want %s", h, v.V1.Ack.Hash)
	}
	if h := ItemHash(v.V1.Entries[0].Logged); h != v.V1.ItemHash {
		t.Fatalf("entry hash %s, want %s", h, v.V1.ItemHash)
	}
}

func TestV2Envelopes(t *testing.T) {
	v := load(t)
	// a device root, and its recovery co-signed by the paper key
	ada, err := FromEnvelope(mustJSON(t, v.V2["ada"].Envelope))
	if err != nil {
		t.Fatal(err)
	}
	if err := Accept(nil, ada); err != nil {
		t.Fatal(err)
	}
	rec, err := FromEnvelope(mustJSON(t, v.V2["ada_recovered"].Envelope))
	if err != nil {
		t.Fatal(err)
	}
	if err := Accept(ada, rec); err != nil {
		t.Fatalf("recovery: %v", err)
	}
	// a passkey root
	bea, err := FromEnvelope(mustJSON(t, v.V2["bea"].Envelope))
	if err != nil {
		t.Fatal(err)
	}
	if err := Accept(nil, bea); err != nil {
		t.Fatalf("passkey entry: %v", err)
	}
	// statements by the root on file
	for _, k := range []string{"leave", "enlist"} {
		env := v.V2[k].Envelope
		if err := VerifyRoot(&env, &rec.Entry); err != nil {
			t.Fatalf("%s: %v", k, err)
		}
		env.Kind = "unenlist"
		if VerifyRoot(&env, &rec.Entry) == nil {
			t.Fatalf("%s passed as another kind", k)
		}
	}
	// an invite in an envelope, by the release key
	inv := v.V2["invite"].Envelope
	body, _ := inv.BodyBytes()
	if err := VerifyEd25519(v.Release, Message("invite", body), inv.Sig); err != nil {
		t.Fatal(err)
	}
	// a v2 entry's body changed is refused
	tampered := v.V2["ada"].Envelope
	b, _ := tampered.BodyBytes()
	b[len(b)-2]++
	tampered.Body = b64.EncodeToString(b)
	if s, err := FromEnvelope(mustJSON(t, tampered)); err == nil && s.Verify() == nil {
		t.Fatal("a changed body verified")
	}
	_ = sha256.Sum256
}

func mustJSON(t *testing.T, v any) json.RawMessage {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

// Every random entry Rust wrote, read from the log's form and rebuilt to
// exactly Rust's bytes
func TestRandomEntriesRebuildByteForByte(t *testing.T) {
	raw, _ := os.ReadFile("testdata/vectors.json")
	var v struct {
		Random []struct {
			Logged    json.RawMessage `json:"logged"`
			Canonical string          `json:"canonical"`
		} `json:"random"`
	}
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	if len(v.Random) < 100 {
		t.Fatalf("only %d random entries", len(v.Random))
	}
	for i, r := range v.Random {
		var item struct {
			Entry json.RawMessage `json:"entry"`
		}
		if err := json.Unmarshal(r.Logged, &item); err != nil {
			t.Fatal(err)
		}
		s, err := FromV1(item.Entry)
		if err != nil {
			t.Fatalf("%d: %v", i, err)
		}
		got, err := s.Entry.Canonical()
		if err != nil {
			t.Fatalf("%d: %v", i, err)
		}
		if string(got) != string(unb64(t, r.Canonical)) {
			t.Fatalf("%d: differ\n got %q\nwant %q", i, got, unb64(t, r.Canonical))
		}
	}
}

// An item that says more, or other, than what is signed in it is refused
func TestAnItemSayingMoreThanItsSignatureIsRefused(t *testing.T) {
	v := load(t)
	var item struct {
		Entry json.RawMessage `json:"entry"`
	}
	if err := json.Unmarshal(v.V1.Entries[0].Logged, &item); err != nil {
		t.Fatal(err)
	}
	if _, err := FromV1(item.Entry); err != nil {
		t.Fatal(err)
	}
	var m map[string]any
	_ = json.Unmarshal(item.Entry, &m)
	cases := map[string]func() []byte{
		"an extra field": func() []byte {
			e := m["entry"].(map[string]any)
			e["admin"] = true
			defer delete(e, "admin")
			b, _ := json.Marshal(m)
			return b
		},
		"a key in another case": func() []byte {
			e := m["entry"].(map[string]any)
			e["NAME"] = e["name"]
			delete(e, "name")
			defer func() { e["name"] = e["NAME"]; delete(e, "NAME") }()
			b, _ := json.Marshal(m)
			return b
		},
		"null for a field left out": func() []byte {
			e := m["entry"].(map[string]any)
			e["grant"] = nil
			defer delete(e, "grant")
			b, _ := json.Marshal(m)
			return b
		},
		"a key twice": func() []byte {
			b, _ := json.Marshal(m)
			return []byte(strings.Replace(string(b), `"signature":`, `"signature":"x","signature":`, 1))
		},
		"a float": func() []byte {
			b, _ := json.Marshal(m)
			return []byte(strings.Replace(string(b), `"version":1`, `"version":1.0`, 1))
		},
	}
	for what, make := range cases {
		if _, err := FromV1(make()); err == nil {
			t.Errorf("%s was taken", what)
		}
	}
	// and at the top: an item kind in another case is no item
	s := New()
	upper := []byte(strings.Replace(string(v.V1.Entries[0].Logged), `"entry":`, `"ENTRY":`, 1))
	s.Judge(&Config{}, []string{"node1"}, "node1", upper, 1)
	if o := s.Outcomes[ItemHash(upper)]; o == "ok" || o == "" {
		t.Fatalf("an item kind in another case: %q", o)
	}
}
