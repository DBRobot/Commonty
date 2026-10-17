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
	s.Enlisted["j01"] = &Enlisted{Member: "ada", Box: "j01", Expires: at + day}
	s.Enlisted["j02"] = &Enlisted{Member: "bea", Box: "j02", Expires: at + day}
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
		{"no box counts", &ada.Entry, func(c *Config) {}, at + 2*day, "none"},
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
		if got, _, _ := s.Status(cfg, c.e, c.at, nil); got != c.want {
			t.Errorf("%s: %s, want %s", c.what, got, c.want)
		}
	}
	// their own leaving beats everything, suspension too
	s.Said["ada"] = &Said{Leave: Leave{Name: "ada"}, Agreed: at}
	cfg := &Config{Release: v.Release}
	cfg.Members.Members = []string{adaID}
	cfg.Members.Revoked = []string{adaID}
	if got, _, _ := s.Status(cfg, &ada.Entry, at, nil); got != "left" {
		t.Errorf("left: %s", got)
	}
}
