package ident

// Standing, against the gate's rules (servers/gate/src/lib.rs member,
// suspension; identity::is_member; leaving.rs)

import "testing"

func TestStanding(t *testing.T) {
	v := load(t)
	ada, err := FromEnvelope(mustJSON(t, v.V2["ada"].Envelope))
	if err != nil {
		t.Fatal(err)
	}
	bea, err := FromEnvelope(mustJSON(t, v.V2["bea"].Envelope))
	if err != nil {
		t.Fatal(err)
	}
	adaID := MemberID(ada.Entry.Root)
	beaID := MemberID(bea.Entry.Root)
	day := uint64(24 * 3600)
	const at = 1_791_700_500
	s := New()
	s.Entries[ada.Entry.Name] = &Held{Signed: ada, Item: ada.Raw}
	s.Entries[bea.Entry.Name] = &Held{Signed: bea, Item: bea.Raw}
	s.Enlisted["j01"] = &Enlisted{Member: ada.Entry.Name, ID: adaID, Box: "j01"}
	s.Enlisted["j02"] = &Enlisted{Member: bea.Entry.Name, ID: beaID, Box: "j02"}
	// both boxes qualify, and were last heard at `at`
	boxes := func(string) (BoxInfo, bool) { return BoxInfo{LastSeen: at * 1000, Facts: computeBox}, true }
	cases := []struct {
		what string
		e    *Entry
		cfg  func(*Config)
		at   uint64
		want string
	}{
		{"on the list", &ada.Entry, func(c *Config) { c.Members.Members = []string{adaID} }, at, "member"},
		{"let in by an invite", &bea.Entry, func(c *Config) {}, at, "member"},
		{"neither, but a box of theirs", &ada.Entry, func(c *Config) {}, at, "member"},
		{"its box gone a month", &ada.Entry, func(c *Config) {}, at + Grace + day, "none"},
		{"revoked, with an invite and a box", &bea.Entry, func(c *Config) { c.Members.Revoked = []string{beaID} }, at, "revoked"},
		{"removed and revoked, its invite older", &bea.Entry, func(c *Config) {
			c.Members.Revoked = []string{beaID}
			c.Members.Removed = map[string]uint64{beaID: at - day}
		}, at, "revoked"},
		{"listed and revoked: revoked wins", &ada.Entry, func(c *Config) {
			c.Members.Members = []string{adaID}
			c.Members.Revoked = []string{adaID}
		}, at, "revoked"},
		{"suspended, in the window", &ada.Entry, func(c *Config) {
			c.Members.Revoked = []string{adaID}
			c.Members.Removed = map[string]uint64{adaID: at - day}
			c.Members.Suspended = map[string]string{adaID: "spam"}
		}, at, "suspended"},
		{"suspended, past the window", &ada.Entry, func(c *Config) {
			c.Members.Revoked = []string{adaID}
			c.Members.Removed = map[string]uint64{adaID: at - SuspendWindow - 1}
			c.Members.Suspended = map[string]string{adaID: "spam"}
		}, at, "revoked"},
		{"a reason without a removal date", &ada.Entry, func(c *Config) {
			c.Members.Revoked = []string{adaID}
			c.Members.Suspended = map[string]string{adaID: "spam"}
		}, at, "revoked"},
		{"removed but not revoked counts for nothing", &ada.Entry, func(c *Config) {
			c.Members.Members = []string{adaID}
			c.Members.Removed = map[string]uint64{adaID: at - day}
		}, at, "member"},
	}
	for _, c := range cases {
		cfg := &Config{Release: v.Release}
		c.cfg(cfg)
		if got, _, _ := s.Status(cfg, c.e, c.at, boxes); got != c.want {
			t.Errorf("%s: %s, want %s", c.what, got, c.want)
		}
	}
	// their own leaving beats everything, suspension too
	s.Said[ada.Entry.Name] = &Said{Leave: Leave{Name: "ada"}, Agreed: at}
	cfg := &Config{Release: v.Release}
	cfg.Members.Members = []string{adaID}
	cfg.Members.Revoked = []string{adaID}
	if got, _, _ := s.Status(cfg, &ada.Entry, at, boxes); got != "left" {
		t.Errorf("left: %s", got)
	}
}
