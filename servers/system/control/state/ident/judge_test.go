package ident

// Who may say what, and that nothing said once counts twice: each kind only
// from its rightful signer, an old item sent again changes nothing, and what
// hangs off a root or a name follows it when it changes.

import (
	"crypto/ed25519"
	"encoding/json"
	"fmt"
	"testing"
)

type person struct {
	name           string
	root, recovery ed25519.PrivateKey
	device         ed25519.PrivateKey
	old            []string
	version        uint64
}

func key(seed byte) ed25519.PrivateKey {
	s := make([]byte, ed25519.SeedSize)
	s[0] = seed
	return ed25519.NewKeyFromSeed(s)
}

func pub(k ed25519.PrivateKey) string { return b64.EncodeToString(k.Public().(ed25519.PublicKey)) }

func seal(t *testing.T, kind string, body []byte, k ed25519.PrivateKey, rec ed25519.PrivateKey) json.RawMessage {
	t.Helper()
	env := Envelope{V: 2, Kind: kind, Body: b64.EncodeToString(body), Sig: b64.EncodeToString(ed25519.Sign(k, Message(kind, body)))}
	if rec != nil {
		r := b64.EncodeToString(ed25519.Sign(rec, Message(kind, body)))
		env.RecoverySig = &r
	}
	b, err := json.Marshal(env)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

// entry: p's entry at version v, signed by `by` (and the recovery key when
// rec is set)
func (p *person) entry(t *testing.T, v uint64, by ed25519.PrivateKey, rec ed25519.PrivateKey) json.RawMessage {
	t.Helper()
	e := Entry{
		Name: p.name, Root: pub(p.root), Recovery: pub(p.recovery), RecoveredFrom: p.old,
		Devices: []Device{{Fingerprint: "f", PublicKey: pub(p.device), Added: 1}}, Version: v, Updated: v,
	}
	body, err := e.Canonical()
	if err != nil {
		t.Fatal(err)
	}
	return seal(t, "entry", body, by, rec)
}

func jsonBody(t *testing.T, v any) []byte {
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

type log struct {
	t  *testing.T
	s  *State
	at uint64
}

var voters = []string{"node1", "node2"}

// put: one item through the judge; its outcome
func (l *log) put(raw json.RawMessage) string {
	l.at++
	l.s.Judge(&Config{Release: pub(key(99))}, voters, "node1", raw, l.at)
	return l.s.Outcomes[ItemHash(raw)]
}

// claim: a first entry, acknowledged by every voter
func (l *log) claim(raw json.RawMessage) string {
	l.put(raw)
	for _, v := range voters {
		l.at++
		l.s.Judge(&Config{}, voters, v, jsonBody(l.t, map[string]string{"ack": LogName(raw)}), l.at)
	}
	return l.s.Outcomes[ItemHash(raw)]
}

func (l *log) must(what, got string, ok bool) {
	l.t.Helper()
	if (got == "ok") != ok {
		l.t.Fatalf("%s: %q", what, got)
	}
}

func newLog(t *testing.T) *log { return &log{t: t, s: New(), at: 1_791_700_000} }

func enlistBody(member, box string, at, expires uint64) []byte {
	return []byte(fmt.Sprintf(`{"member":%q,"box":%q,"key":"k","ticket":"00","expires":%d,"at":%d}`, member, box, expires, at))
}

func unenlistBody(member, box string, at uint64) []byte {
	return []byte(fmt.Sprintf(`{"member":%q,"box":%q,"at":%d}`, member, box, at))
}

func TestOnlyTheRightfulSignerIsHeard(t *testing.T) {
	l := newLog(t)
	ann := &person{name: "ann", root: key(1), recovery: key(2), device: key(3)}
	l.must("ann's claim", l.claim(ann.entry(t, 1, ann.root, nil)), true)
	// a first claim to a held name, by another root, takes nothing
	eve := &person{name: "ann", root: key(10), recovery: key(11), device: key(12)}
	l.must("another's claim to ann", l.claim(eve.entry(t, 5, eve.root, nil)), false)
	// an update signed by a device of hers, not her root
	l.must("signed by a device", l.put(ann.entry(t, 2, ann.device, nil)), false)
	// a new root without the recovery key; then with it
	was := *ann
	ann.old, ann.root = []string{pub(was.root)}, key(4)
	l.must("a new root alone", l.put(ann.entry(t, 2, ann.root, nil)), false)
	l.must("a new root and the recovery key", l.put(ann.entry(t, 2, ann.root, ann.recovery)), true)
	// the old root is no one now: its leave, its enlist
	leave := jsonBody(t, Leave{Name: "ann", Root: pub(was.root), At: l.at})
	l.must("a leave by the old root", l.put(seal(t, "leave", leave, was.root, nil)), false)
	l.must("an enlist by the old root", l.put(seal(t, "enlist", enlistBody(MemberID(pub(was.root)), "j1", l.at, l.at+3600), was.root, nil)), false)
	l.must("an enlist by the old root, naming the new", l.put(seal(t, "enlist", enlistBody(MemberID(pub(ann.root)), "j1", l.at, l.at+3600), was.root, nil)), false)
	l.must("an enlist by her root", l.put(seal(t, "enlist", enlistBody(MemberID(pub(ann.root)), "j1", l.at, l.at+3600), ann.root, nil)), true)
	// someone else's leave in her name
	leave = jsonBody(t, Leave{Name: "ann", Root: pub(eve.root), At: l.at})
	l.must("a leave by another", l.put(seal(t, "leave", leave, eve.root, nil)), false)
	// an invite only by the release key
	inv := (&Invite{PublicKey: "c", Issued: l.at, Expires: l.at + 600}).Bytes()
	l.must("an invite by ann", l.put(seal(t, "invite", inv, ann.root, nil)), false)
	l.must("an invite by the release key", l.put(seal(t, "invite", inv, key(99), nil)), true)
}

func TestNothingSaidOnceCountsTwice(t *testing.T) {
	l := newLog(t)
	ann := &person{name: "ann", root: key(1), recovery: key(2), device: key(3)}
	id := MemberID(pub(ann.root))
	l.claim(ann.entry(t, 1, ann.root, nil))
	v2 := ann.entry(t, 2, ann.root, nil)
	l.must("version 2", l.put(v2), true)
	l.must("version 3", l.put(ann.entry(t, 3, ann.root, nil)), true)
	l.put(v2)
	if got := l.s.Entries["ann"].Signed.Entry.Version; got != 3 {
		t.Fatalf("version 2 sent again: now %d", got)
	}
	// leaving, coming back, and the old leave again
	gone := seal(t, "leave", jsonBody(t, Leave{Name: "ann", Root: pub(ann.root), At: l.at}), ann.root, nil)
	l.must("a leave", l.put(gone), true)
	back := seal(t, "leave", jsonBody(t, Leave{Name: "ann", Root: pub(ann.root), At: l.at + 1, Back: true}), ann.root, nil)
	l.must("back", l.put(back), true)
	l.put(gone)
	if !l.s.Said["ann"].Leave.Back {
		t.Fatal("the old leave, sent again, took her out")
	}
	// a box: enlisted, unenlisted, and each sent again
	en := seal(t, "enlist", enlistBody(id, "j1", l.at, l.at+3600), ann.root, nil)
	l.must("enlist", l.put(en), true)
	un := seal(t, "unenlist", unenlistBody(id, "j1", l.at), ann.root, nil)
	l.must("unenlist", l.put(un), true)
	l.must("the old enlist again", l.put(en), false)
	if l.s.Enlisted["j1"] != nil {
		t.Fatal("the old enlist, sent again, enlisted the box")
	}
	en2 := seal(t, "enlist", enlistBody(id, "j1", l.at, l.at+3600), ann.root, nil)
	l.must("enlisted anew", l.put(en2), true)
	l.put(un)
	if l.s.Enlisted["j1"] == nil {
		t.Fatal("the old unenlist, sent again, took the box away")
	}
	// the same words under another hash (a fresh signature): still old
	un2 := seal(t, "unenlist", unenlistBody(id, "j1", l.at-5), ann.root, nil)
	l.must("an unenlist dated before the enlist", l.put(un2), false)
	// undated, stale, expired
	l.must("undated", l.put(seal(t, "enlist", []byte(fmt.Sprintf(`{"member":%q,"box":"j2","expires":%d}`, id, l.at+3600)), ann.root, nil)), false)
	l.must("a day old", l.put(seal(t, "enlist", enlistBody(id, "j2", l.at-2*leaveBehind, l.at+3600), ann.root, nil)), false)
	l.must("expired", l.put(seal(t, "enlist", enlistBody(id, "j2", l.at, l.at), ann.root, nil)), false)
}

func TestWhatHangsOffARootFollowsIt(t *testing.T) {
	l := newLog(t)
	cfg := &Config{}
	ann := &person{name: "ann", root: key(1), recovery: key(2), device: key(3)}
	bob := &person{name: "bob", root: key(5), recovery: key(6), device: key(7)}
	l.claim(ann.entry(t, 1, ann.root, nil))
	l.claim(bob.entry(t, 1, bob.root, nil))
	l.must("ann's box", l.put(seal(t, "enlist", enlistBody(MemberID(pub(ann.root)), "j1", l.at, l.at+7200), ann.root, nil)), true)
	// bob cannot take it while it is hers
	l.must("bob takes ann's box", l.put(seal(t, "enlist", enlistBody(MemberID(pub(bob.root)), "j1", l.at, l.at+7200), bob.root, nil)), false)
	// a recovery: the box is still hers, the person is the same
	was := *ann
	ann.old, ann.root = []string{pub(was.root)}, key(4)
	l.must("recovered", l.put(ann.entry(t, 2, ann.root, ann.recovery)), true)
	if got := l.s.boxesOf("ann", l.at); len(got) != 1 {
		t.Fatalf("after a recovery: %v", got)
	}
	// leaving: the box is no one's at once, and bob may enlist it
	l.must("ann leaves", l.put(seal(t, "leave", jsonBody(t, Leave{Name: "ann", Root: pub(ann.root), At: l.at}), ann.root, nil)), true)
	if got := l.s.boxesOf("ann", l.at); len(got) != 0 {
		t.Fatalf("after leaving: %v", got)
	}
	if st, _, _ := l.s.Status(cfg, &l.s.Entries["ann"].Signed.Entry, l.at, nil); st != "left" {
		t.Fatal(st)
	}
	l.must("bob enlists it now", l.put(seal(t, "enlist", enlistBody(MemberID(pub(bob.root)), "j1", l.at, l.at+7200), bob.root, nil)), true)
	if got := l.s.boxesOf("bob", l.at); len(got) != 1 {
		t.Fatalf("bob's: %v", got)
	}
}

// a guest's name that lapsed and went to someone else takes none of the
// first guest's boxes along
func TestALapsedGuestNameComesClean(t *testing.T) {
	l := newLog(t)
	g1 := &person{name: "guest-x", root: key(1), recovery: key(2), device: key(3)}
	g2 := &person{name: "guest-x", root: key(5), recovery: key(6), device: key(7)}
	l.must("first guest", l.claim(g1.entry(t, l.at, g1.root, nil)), true)
	l.must("their box", l.put(seal(t, "enlist", enlistBody(MemberID(pub(g1.root)), "j1", l.at, l.at+7200), g1.root, nil)), true)
	l.at += guestTTL + 60
	l.must("the name again, by another", l.claim(g2.entry(t, l.at, g2.root, nil)), true)
	if got := l.s.boxesOf("guest-x", l.at); len(got) != 0 {
		t.Fatalf("the new guest has the old one's boxes: %v", got)
	}
}
