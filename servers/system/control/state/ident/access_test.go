package ident

// Access by a box: the rules of docs/dev/joining.md ("Access"), each on its
// own, through the judge and the view as a reader gets them.

import (
	"testing"
)

const day = 24 * 3600

var computeBox = map[string]float64{"tpm_major": 2, "uefi": 1, "cores": 4, "ram_bytes": 8e9, "agent_ok": 1}

// fleet: a log, and what the agreed state says of each box
type fleet struct {
	*log
	cfg   *Config
	boxes map[string]BoxInfo
}

func newFleet(t *testing.T) *fleet {
	return &fleet{log: newLog(t), cfg: &Config{Release: pub(key(99)), Admin: "david"}, boxes: map[string]BoxInfo{}}
}

func (f *fleet) put(raw []byte) string {
	f.at++
	f.s.Judge(f.cfg, voters, "node1", raw, f.at)
	return f.s.Outcomes[ItemHash(raw)]
}

// box: p's box n joined with a ticket of theirs, enlisted by their root,
// heard now with these facts
func (f *fleet) box(p *person, n byte, facts map[string]float64) string {
	f.t.Helper()
	name := f.join(n, p, tid(n), f.at+7*day)
	f.must("enlist", f.put(seal(f.t, "enlist", enlistBody(MemberID(pub(p.root)), name, tid(n), f.at), p.root, nil)), true)
	f.boxes[name] = BoxInfo{LastSeen: int64(f.at) * 1000, Facts: facts}
	return name
}

func (f *fleet) unenlist(p *person, name string) {
	f.t.Helper()
	f.must("unenlist", f.put(seal(f.t, "unenlist", unenlistBody(MemberID(pub(p.root)), name, f.at), p.root, nil)), true)
}

func (f *fleet) access(name string) string {
	f.t.Helper()
	v := f.s.View(f.cfg, 1, int64(f.at)*1000, func(b string) (BoxInfo, bool) { i, ok := f.boxes[b]; return i, ok })
	m := v.Members[name]
	if m == nil {
		f.t.Fatalf("no %s in the view", name)
	}
	// the status a reader of status alone sees says the same
	if (m.Access != "none") != (m.Status == "member") {
		f.t.Fatalf("%s: access %s but status %s", name, m.Access, m.Status)
	}
	return m.Access
}

func (f *fleet) person(name string, seed byte) *person {
	p := &person{name: name, root: key(seed), recovery: key(seed + 1), device: key(seed + 2)}
	f.must(name, f.claim(p.entry(f.t, 1, p.root, nil)), true)
	return p
}

func TestSigningUpGivesNothing(t *testing.T) {
	f := newFleet(t)
	f.person("ann", 1)
	if got := f.access("ann"); got != "none" {
		t.Fatal(got)
	}
}

func TestAQualifyingBoxGivesAccess(t *testing.T) {
	f := newFleet(t)
	ann := f.person("ann", 1)
	b := f.box(ann, 1, computeBox)
	if got := f.access("ann"); got != "box" {
		t.Fatal(got)
	}
	v := f.s.View(f.cfg, 1, int64(f.at)*1000, func(n string) (BoxInfo, bool) { i, ok := f.boxes[n]; return i, ok })
	if o := v.Enlisted[b]; o == nil || o.Owner != "ann" || o.Ticket != tid(1) {
		t.Fatalf("%+v", o)
	}
	if e := v.Members["ann"].Enlisted; len(e) != 1 || e[0].Qualifies != "compute" || !e[0].Counts {
		t.Fatalf("%+v", e)
	}
}

func TestWhatQualifies(t *testing.T) {
	with := func(kv ...any) map[string]float64 {
		f := map[string]float64{"tpm_major": 2, "uefi": 1}
		for i := 0; i < len(kv); i += 2 {
			f[kv[i].(string)] = kv[i+1].(float64)
		}
		return f
	}
	cases := []struct {
		what string
		f    map[string]float64
		want string
	}{
		{"a mini PC", with("cores", 4.0, "ram_bytes", 7.6*(1<<30)), "compute"},
		{"a Raspberry Pi 5, 8 GB", with("cores", 4.0, "ram_bytes", 7.9*(1<<30), "raspberry_pi", 1.0), ""},
		{"a Pi with a big disk", with("cores", 4.0, "raspberry_pi", 1.0, "store_size_bytes", 2e12), "storage"},
		{"two cores", with("cores", 2.0, "ram_bytes", 16e9), ""},
		{"4 GB", with("cores", 8.0, "ram_bytes", 4e9), ""},
		{"a NAS", with("cores", 2.0, "ram_bytes", 2e9, "store_size_bytes", 4e12), "storage"},
		{"a 1 TB disk short of 1 TB usable", with("store_size_bytes", 0.93e12), ""},
		{"a GPU box", with("cores", 2.0, "gpus", 1.0), "gpu"},
		{"no TPM", map[string]float64{"uefi": 1, "cores": 16, "ram_bytes": 64e9}, ""},
		{"TPM 1.2", map[string]float64{"tpm_major": 1, "uefi": 1, "cores": 16, "ram_bytes": 64e9}, ""},
		{"BIOS, not UEFI", map[string]float64{"tpm_major": 2, "cores": 16, "ram_bytes": 64e9}, ""},
		{"nothing said", nil, ""},
	}
	for _, c := range cases {
		if got := Qualify(c.f); got != c.want {
			t.Errorf("%s: %q, want %q", c.what, got, c.want)
		}
	}
}

func TestANonQualifyingPiGivesNothing(t *testing.T) {
	f := newFleet(t)
	ann := f.person("ann", 1)
	f.box(ann, 1, map[string]float64{"tpm_major": 2, "uefi": 1, "cores": 4, "ram_bytes": 8e9, "raspberry_pi": 1})
	if got := f.access("ann"); got != "none" {
		t.Fatal(got)
	}
	// and a box that has said nothing of itself yet
	bob := f.person("bob", 10)
	b := f.box(bob, 2, nil)
	if got := f.access("bob"); got != "none" {
		t.Fatal(got)
	}
	f.boxes[b] = BoxInfo{LastSeen: int64(f.at) * 1000, Facts: computeBox}
	if got := f.access("bob"); got != "box" {
		t.Fatal(got)
	}
}

func TestRemovingOneOfTwoKeepsAccess(t *testing.T) {
	f := newFleet(t)
	ann := f.person("ann", 1)
	a := f.box(ann, 1, computeBox)
	b := f.box(ann, 2, computeBox)
	f.unenlist(ann, a)
	if got := f.access("ann"); got != "box" {
		t.Fatal(got)
	}
	f.unenlist(ann, b)
	if got := f.access("ann"); got != "none" {
		t.Fatal(got)
	}
}

func TestGoneAMonthLosesAccess(t *testing.T) {
	f := newFleet(t)
	ann := f.person("ann", 1)
	a := f.box(ann, 1, computeBox)
	b := f.box(ann, 2, computeBox)
	// one box quiet a month, the other still heard: kept
	f.at += Grace + day
	f.boxes[b] = BoxInfo{LastSeen: int64(f.at) * 1000, Facts: computeBox}
	if got := f.access("ann"); got != "box" {
		t.Fatal(got)
	}
	// both quiet: 29 days in, kept; past 30, gone
	f.boxes[b] = BoxInfo{LastSeen: int64(f.at) * 1000, Facts: computeBox}
	f.boxes[a] = f.boxes[b]
	f.at += Grace - day
	if got := f.access("ann"); got != "box" {
		t.Fatal(got)
	}
	f.at += 2 * day
	if got := f.access("ann"); got != "none" {
		t.Fatal(got)
	}
	// and dropped from the agreed state altogether: still gone
	delete(f.boxes, a)
	delete(f.boxes, b)
	if got := f.access("ann"); got != "none" {
		t.Fatal(got)
	}
	// it comes back: access again
	f.boxes[a] = BoxInfo{LastSeen: int64(f.at) * 1000, Facts: computeBox}
	if got := f.access("ann"); got != "box" {
		t.Fatal(got)
	}
}

func TestDownByOurFaultKeepsAccess(t *testing.T) {
	f := newFleet(t)
	ann := f.person("ann", 1)
	a := f.box(ann, 1, computeBox)
	failed := map[string]float64{}
	for k, v := range computeBox {
		failed[k] = v
	}
	failed["agent_ok"] = 0
	f.boxes[a] = BoxInfo{LastSeen: int64(f.at) * 1000, Facts: failed}
	f.at += 3 * Grace
	if got := f.access("ann"); got != "box" {
		t.Fatal(got)
	}
	// a box that never said how its release went is not our fault
	delete(failed, "agent_ok")
	if got := f.access("ann"); got != "none" {
		t.Fatal(got)
	}
}

func TestOnlyTheAdminExtendsGrace(t *testing.T) {
	f := newFleet(t)
	david := f.person("david", 20)
	ann := f.person("ann", 1)
	a := f.box(ann, 1, computeBox)
	until := f.at + 90*day
	ext := func(by *person, until, at uint64) []byte {
		return seal(t, "extend", jsonBody(t, map[string]any{"name": "ann", "until": until, "at": at}), by.root, nil)
	}
	f.must("ann extends her own", f.put(ext(ann, until, f.at)), false)
	f.must("david's device signs it", f.put(seal(t, "extend", jsonBody(t, map[string]any{"name": "ann", "until": until, "at": f.at}), david.device, nil)), false)
	f.must("david extends ann's", f.put(ext(david, until, f.at)), true)
	// gone 60 days: kept by the extension; and dropped from the state
	f.at += 60 * day
	if got := f.access("ann"); got != "box" {
		t.Fatal(got)
	}
	delete(f.boxes, a)
	if got := f.access("ann"); got != "box" {
		t.Fatal(got)
	}
	// past its until: gone
	f.at += 31 * day
	if got := f.access("ann"); got != "none" {
		t.Fatal(got)
	}
	// an extension keeps nobody with no box enlisted
	f.must("again", f.put(ext(david, f.at+90*day, f.at)), true)
	f.boxes[a] = BoxInfo{LastSeen: int64(f.at) * 1000, Facts: computeBox}
	f.unenlist(ann, a)
	if got := f.access("ann"); got != "none" {
		t.Fatal(got)
	}
	// with no admin in the release, nobody extends
	f.cfg.Admin = ""
	f.must("no admin", f.put(ext(david, f.at+90*day, f.at)), false)
}

func TestABanIsABan(t *testing.T) {
	f := newFleet(t)
	ann := f.person("ann", 1)
	f.box(ann, 1, computeBox)
	id := MemberID(pub(ann.root))
	f.cfg.Members.Members = []string{id}
	if got := f.access("ann"); got != "invite" {
		t.Fatal(got)
	}
	// on the list and with a box, then removed: nothing lets her in
	f.cfg.Members.Revoked = []string{id}
	if got := f.access("ann"); got != "none" {
		t.Fatal(got)
	}
	// nor a new box
	name := f.join(2, ann, tid(2), f.at+7*day)
	f.must("a banned member's enlist", f.put(seal(t, "enlist", enlistBody(id, name, tid(2), f.at), ann.root, nil)), false)
	// David lifting it does
	f.cfg.Members.Revoked = nil
	if got := f.access("ann"); got != "invite" {
		t.Fatal(got)
	}
}

func TestATicketIsTheMembersOnce(t *testing.T) {
	f := newFleet(t)
	ann := f.person("ann", 1)
	bob := f.person("bob", 10)
	annID, bobID := MemberID(pub(ann.root)), MemberID(pub(bob.root))
	// ann's box, joined with her ticket: bob's root can't have it
	a := f.join(1, ann, tid(1), f.at+7*day)
	f.must("bob with ann's ticket", f.put(seal(t, "enlist", enlistBody(bobID, a, tid(1), f.at), bob.root, nil)), false)
	// a box that joined with a ticket bob's device made for ann: not bob's
	b := f.join(2, &person{name: "ann", device: bob.device}, tid(2), f.at+7*day)
	f.must("a ticket in another's name", f.put(seal(t, "enlist", enlistBody(bobID, b, tid(2), f.at), bob.root, nil)), false)
	f.must("ann's, by a device not hers", f.put(seal(t, "enlist", enlistBody(annID, b, tid(2), f.at), ann.root, nil)), false)
	// naming a ticket the box did not join with
	f.must("another ticket id", f.put(seal(t, "enlist", enlistBody(annID, a, tid(9), f.at), ann.root, nil)), false)
	// a box that joined with no ticket, or never joined
	f.must("never joined", f.put(seal(t, "enlist", enlistBody(annID, "j0000000000", tid(1), f.at), ann.root, nil)), false)
	f.must("ann's own", f.put(seal(t, "enlist", enlistBody(annID, a, tid(1), f.at), ann.root, nil)), true)
	// the same ticket, on another box that came with a copy of it
	name, spki := joinedBox(3)
	f.at++
	f.s.Judge(f.cfg, voters, name, jsonBody(t, map[string]map[string]string{"joined": {"key": spki, "ticket": f.s.Joins[a].Ticket}}), f.at)
	f.must("a used ticket", f.put(seal(t, "enlist", enlistBody(annID, name, tid(1), f.at), ann.root, nil)), false)
}

func TestOnlyTheStateMachineSaysABoxJoined(t *testing.T) {
	f := newFleet(t)
	ann := f.person("ann", 1)
	name, spki := joinedBox(1)
	rec := jsonBody(t, map[string]map[string]string{"joined": {"key": spki, "ticket": joinTicket("ann", tid(1), f.at+day).token(ann.device)}})
	// from a box whose name is not the key's: nothing
	f.at++
	f.s.Judge(f.cfg, voters, "node1", rec, f.at)
	if f.s.Joins[name] != nil || f.s.Joins["node1"] != nil {
		t.Fatal("a box's gate said another box joined")
	}
	f.at++
	f.s.Judge(f.cfg, voters, name, rec, f.at)
	if f.s.Joins[name] == nil {
		t.Fatal("the record was not kept")
	}
	// past its ticket's day, another join clears it
	f.at += 3 * day
	f.join(2, ann, tid(2), f.at+day)
	if f.s.Joins[name] != nil {
		t.Fatal("an expired join record stayed")
	}
}
