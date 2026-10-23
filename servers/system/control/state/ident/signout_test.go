package ident

// Signing out everywhere: only the member's root says it, it only moves
// forward, it follows the person and not the name, and it changes nothing a
// judge that does not know it would decide otherwise.

import (
	"encoding/json"
	"fmt"
	"strings"
	"testing"
)

func signoutBody(member string, at uint64, keep string) []byte {
	if keep == "" {
		return []byte(fmt.Sprintf(`{"member":%q,"at":%d}`, member, at))
	}
	return []byte(fmt.Sprintf(`{"member":%q,"at":%d,"keep":%q}`, member, at, keep))
}

func (l *log) signedOut(name string) (uint64, string) {
	m := l.s.View(&Config{}, 1, int64(l.at)*1000, nil).Members[name]
	if m == nil {
		l.t.Fatalf("%s has no entry", name)
	}
	return m.SignedOut, m.SignedOutKeep
}

func TestOnlyTheMembersRootSignsThemOut(t *testing.T) {
	l := newLog(t)
	ann := &person{name: "ann", root: key(1), recovery: key(2), device: key(3)}
	eve := &person{name: "eve", root: key(10), recovery: key(11), device: key(12)}
	l.claim(ann.entry(t, 1, ann.root, nil))
	l.claim(eve.entry(t, 1, eve.root, nil))
	id := MemberID(pub(ann.root))
	l.must("by her device", l.put(seal(t, "signout", signoutBody(id, l.at, ""), ann.device, nil)), false)
	l.must("by someone else", l.put(seal(t, "signout", signoutBody(id, l.at, ""), eve.root, nil)), false)
	l.must("her recovery key alone", l.put(seal(t, "signout", signoutBody(id, l.at, ""), ann.recovery, nil)), false)
	l.must("eve, naming herself", l.put(seal(t, "signout", signoutBody(MemberID(pub(eve.root)), l.at, ""), eve.root, nil)), true)
	if at, _ := l.signedOut("ann"); at != 0 {
		t.Fatalf("ann signed out by eve's word: %d", at)
	}
	said := l.at
	l.must("by her root", l.put(seal(t, "signout", signoutBody(id, said, "kept-one"), ann.root, nil)), true)
	if at, keep := l.signedOut("ann"); at != said*1000 || keep != "kept-one" {
		t.Fatalf("ann: %d %q", at, keep)
	}
	if at, _ := l.signedOut("eve"); at == 0 {
		t.Fatal("eve's own sign-out went")
	}
}

func TestASignOutOnlyMovesForward(t *testing.T) {
	l := newLog(t)
	ann := &person{name: "ann", root: key(1), recovery: key(2), device: key(3)}
	l.claim(ann.entry(t, 1, ann.root, nil))
	id := MemberID(pub(ann.root))
	first := seal(t, "signout", signoutBody(id, l.at, ""), ann.root, nil)
	l.must("signed out", l.put(first), true)
	was, _ := l.signedOut("ann")
	l.must("the same again", l.put(first), true)
	l.must("the same time, signed again", l.put(seal(t, "signout", signoutBody(id, was/1000, ""), ann.root, nil)), true)
	l.must("older", l.put(seal(t, "signout", signoutBody(id, was/1000-1, ""), ann.root, nil)), false)
	l.must("the same time, keeping a session", l.put(seal(t, "signout", signoutBody(id, was/1000, "x"), ann.root, nil)), false)
	if now, keep := l.signedOut("ann"); now != was || keep != "" {
		t.Fatalf("taken back: %d %q", now, keep)
	}
	l.must("a day and more old", l.put(seal(t, "signout", signoutBody(id, l.at-leaveBehind-1, ""), ann.root, nil)), false)
	l.must("far ahead", l.put(seal(t, "signout", signoutBody(id, l.at+leaveAhead+10, ""), ann.root, nil)), false)
	// a little ahead is taken, and ends nothing begun after it was agreed
	l.must("a little ahead", l.put(seal(t, "signout", signoutBody(id, l.at+60, ""), ann.root, nil)), true)
	if now, _ := l.signedOut("ann"); now != l.at*1000 {
		t.Fatalf("dated ahead: %d, agreed %d", now, l.at*1000)
	}
	// a later version of her entry keeps it: it is not in the entry
	l.must("version 2", l.put(ann.entry(t, 2, ann.root, nil)), true)
	if now, _ := l.signedOut("ann"); now == 0 {
		t.Fatal("an entry update took the sign-out back")
	}
	for _, bad := range [][]byte{
		[]byte(fmt.Sprintf(`{"member":%q,"at":%d,"keep":"a b"}`, id, l.at)),
		[]byte(fmt.Sprintf(`{"member":%q,"at":%d,"keep":%q}`, id, l.at, strings.Repeat("a", keepMax+1))),
		[]byte(fmt.Sprintf(`{"member":%q,"at":%d,"keep":""}`, id, l.at)),
		[]byte(fmt.Sprintf(`{"member":%q,"at":%d,"all":true}`, id, l.at)),
		[]byte(fmt.Sprintf(`{"member":%q}`, id)),
	} {
		l.must(string(bad), l.put(seal(t, "signout", bad, ann.root, nil)), false)
	}
}

func TestASignOutFollowsThePersonNotTheName(t *testing.T) {
	l := newLog(t)
	ann := &person{name: "ann", root: key(1), recovery: key(2), device: key(3)}
	l.claim(ann.entry(t, 1, ann.root, nil))
	l.must("signed out", l.put(seal(t, "signout", signoutBody(MemberID(pub(ann.root)), l.at, ""), ann.root, nil)), true)
	was, _ := l.signedOut("ann")
	// a recovery: the same person, still signed out
	old := ann.root
	ann.old, ann.root = []string{pub(old)}, key(4)
	l.must("recovered", l.put(ann.entry(t, 2, ann.root, ann.recovery)), true)
	if now, _ := l.signedOut("ann"); now != was {
		t.Fatalf("after a recovery: %d, was %d", now, was)
	}
	l.must("the old root's", l.put(seal(t, "signout", signoutBody(MemberID(pub(old)), l.at, ""), old, nil)), false)
	l.must("the new root's", l.put(seal(t, "signout", signoutBody(MemberID(pub(ann.root)), l.at, ""), ann.root, nil)), true)

	// a guest's name gone to someone else: the first guest's sign-out is not theirs
	g1 := &person{name: "guest-y", root: key(20), recovery: key(21), device: key(22)}
	g2 := &person{name: "guest-y", root: key(25), recovery: key(26), device: key(27)}
	l.claim(g1.entry(t, l.at, g1.root, nil))
	l.must("the guest signs out", l.put(seal(t, "signout", signoutBody(MemberID(pub(g1.root)), l.at, ""), g1.root, nil)), true)
	l.at += guestTTL + 60
	l.must("the name again, by another", l.claim(g2.entry(t, l.at, g2.root, nil)), true)
	if now, _ := l.signedOut("guest-y"); now != 0 {
		t.Fatalf("the new guest is signed out by the old one's word: %d", now)
	}
	l.must("the new guest's own", l.put(seal(t, "signout", signoutBody(MemberID(pub(g2.root)), l.at-guestTTL, ""), g2.root, nil)), true)
}

// A judge from before this kind refuses it as "no such item" and moves
// nothing else. What the state machine reads of the judge (an entry, a
// status, access, boxes: a remote computer's lease) must come out the same
// either way, or validators on two releases would decide differently.
func TestASignOutChangesNothingAnOldJudgeDecides(t *testing.T) {
	newer, older := newLog(t), newLog(t)
	cfg := &Config{Release: pub(key(99))}
	both := func(raw json.RawMessage, signout bool) {
		newer.put(raw)
		if signout {
			older.at++
			older.s.outcome(ItemHash(raw), fmt.Errorf("no such item: %q", "signout"))
			return
		}
		older.put(raw)
	}
	claim := func(raw json.RawMessage) {
		newer.claim(raw)
		older.claim(raw)
	}
	ann := &person{name: "ann", root: key(1), recovery: key(2), device: key(3)}
	bob := &person{name: "bob", root: key(5), recovery: key(6), device: key(7)}
	claim(ann.entry(t, 1, ann.root, nil))
	claim(bob.entry(t, 1, bob.root, nil))
	both(seal(t, "signout", signoutBody(MemberID(pub(ann.root)), newer.at, ""), ann.root, nil), true)
	both(ann.entry(t, 2, ann.root, nil), false)
	j1 := newer.join(1, ann, tid(1), newer.at+3600)
	older.join(1, ann, tid(1), older.at+3600)
	both(seal(t, "enlist", enlistBody(MemberID(pub(ann.root)), j1, tid(1), newer.at), ann.root, nil), false)
	both(seal(t, "signout", signoutBody(MemberID(pub(bob.root)), newer.at, "k"), bob.root, nil), true)
	both(seal(t, "leave", jsonBody(t, Leave{Name: "bob", Root: pub(bob.root), At: newer.at}), bob.root, nil), false)
	if newer.at != older.at {
		t.Fatalf("the logs drifted: %d %d", newer.at, older.at)
	}
	boxes := func(string) (BoxInfo, bool) {
		return BoxInfo{LastSeen: int64(newer.at) * 1000, Facts: computeBox}, true
	}
	for _, n := range []string{"ann", "bob"} {
		a, b := newer.s.Entries[n], older.s.Entries[n]
		if string(a.Item) != string(b.Item) || a.Agreed != b.Agreed {
			t.Fatalf("%s's entry differs", n)
		}
		sa, _, _ := newer.s.Status(cfg, &a.Signed.Entry, newer.at, boxes)
		sb, _, _ := older.s.Status(cfg, &b.Signed.Entry, older.at, boxes)
		aa, _, _ := newer.s.Access(cfg, &a.Signed.Entry, newer.at, boxes)
		ab, _, _ := older.s.Access(cfg, &b.Signed.Entry, older.at, boxes)
		if sa != sb || aa != ab {
			t.Fatalf("%s: status %s/%s, access %s/%s", n, sa, sb, aa, ab)
		}
	}
	va := newer.s.View(cfg, 9, int64(newer.at)*1000, boxes)
	vb := older.s.View(cfg, 9, int64(older.at)*1000, boxes)
	if va.Members["ann"].SignedOut == 0 || va.Members["bob"].SignedOutKeep != "k" {
		t.Fatal("the newer judge did not take the sign-outs")
	}
	for _, m := range va.Members {
		m.SignedOut, m.SignedOutKeep = 0, ""
	}
	va.Outcomes, vb.Outcomes = nil, nil
	ja, _ := json.Marshal(va)
	jb, _ := json.Marshal(vb)
	if string(ja) != string(jb) {
		t.Fatalf("the views differ beyond the sign-outs:\n%s\n%s", ja, jb)
	}
	if strings.Contains(string(jb), "signed_out") {
		t.Fatal("a view with no sign-out says signed_out: the old answer's bytes changed")
	}
}
