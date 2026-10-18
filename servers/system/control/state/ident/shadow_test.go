package ident

// The shadow check (docs/dev/identity-state.md, section 4): the same log the
// gate's shadow_dump replayed, through the judge, and the two answers
// compared. SHADOW_LOG (the log), SHADOW_RUST (the gate's dump),
// SHADOW_RELEASE, SHADOW_MEMBERS (fleet/members.json), SHADOW_VOTERS.
// Without SHADOW_LOG it is skipped; TestShadowFixture runs the same on a
// fixture, and by hand it runs on a log fetched from a box.

import (
	"encoding/json"
	"os"
	"reflect"
	"sort"
	"strings"
	"testing"
)

func dump(s *State, items []Item, at uint64) map[string]any {
	entries := map[string]any{}
	for _, n := range s.names() {
		h := s.entry(n, at)
		if h == nil {
			continue
		}
		e := &h.Signed.Entry
		devs := []any{}
		for _, d := range e.Devices {
			devs = append(devs, d.PublicKey)
		}
		pks := []any{}
		for _, p := range e.Passkeys {
			pks = append(pks, p.ID)
		}
		libs := []any{}
		for _, l := range e.Libraries {
			libs = append(libs, l.ID)
		}
		rf := []any{}
		for _, r := range e.RecoveredFrom {
			rf = append(rf, r)
		}
		entries[n] = map[string]any{
			"root": e.Root, "version": float64(e.Version), "recovered_from": rf, "devices": devs,
			"passkeys": pks, "libraries": libs, "grant": e.Grant != nil, "handovers": float64(len(s.Handovers[n])),
		}
	}
	pending := []any{}
	for _, p := range s.Pending {
		pending = append(pending, p.Hash)
	}
	said := map[string]any{}
	for n, x := range s.Said {
		said[n] = map[string]any{"at": float64(x.Leave.At), "back": x.Leave.Back, "agreed": float64(x.Agreed)}
	}
	outcomes := map[string]any{}
	// the gate's replay keeps an outcome per log item; the judge one per
	// statement (its item hash). A statement that came in more than one
	// wrapping is one verdict here and several there, by design
	// (identity-state.md, "Item hash"): compared only where a statement
	// came once
	logNames := map[string]map[string]bool{}
	for _, it := range items {
		k := ItemHash(it.Item)
		if logNames[k] == nil {
			logNames[k] = map[string]bool{}
		}
		logNames[k][LogName(it.Item)] = true
	}
	for _, it := range items {
		k := ItemHash(it.Item)
		if len(logNames[k]) > 1 {
			continue
		}
		if o, ok := s.Outcomes[k]; ok {
			outcomes[LogName(it.Item)] = o == "ok"
		}
	}
	return map[string]any{"entries": entries, "pending": pending, "said": said, "outcomes": outcomes}
}

// shadow replays a log page through the judge, and compares the answer to
// the gate's dump of the same log when there is one
func shadow(t *testing.T, raw []byte, cfg *Config, voters []string, rustf, out string) {
	t.Helper()
	var page struct {
		Items []Item `json:"items"`
	}
	if err := json.Unmarshal(raw, &page); err != nil {
		t.Fatal(err)
	}
	sort.Strings(voters)
	r := NewReplayer(cfg, voters)
	s := r.Catch(page.Items)
	// and once more through JSON, as the state machine copies it
	b, _ := json.Marshal(s)
	again := New()
	if err := json.Unmarshal(b, again); err != nil {
		t.Fatalf("the state does not read back: %v", err)
	}
	last := uint64(0)
	if n := len(page.Items); n > 0 {
		last = uint64(page.Items[n-1].Time / 1000)
	}
	mine := dump(again, page.Items, last)
	if out != "" {
		b, _ := json.MarshalIndent(mine, "", "  ")
		_ = os.WriteFile(out, b, 0o600)
	}
	if rustf == "" {
		return
	}
	rb, err := os.ReadFile(rustf)
	if err != nil {
		t.Fatal(err)
	}
	var theirs map[string]any
	if err := json.Unmarshal(rb, &theirs); err != nil {
		t.Fatal(err)
	}
	// round mine through JSON so both are the same kinds of value
	mb, _ := json.Marshal(mine)
	var m2 map[string]any
	_ = json.Unmarshal(mb, &m2)
	// the gate's outcomes for a statement that came in more than one
	// wrapping are left out on its side too (see dump)
	if go_, ok := theirs["outcomes"].(map[string]any); ok {
		several := map[string]map[string]bool{}
		for _, it := range page.Items {
			k := ItemHash(it.Item)
			if several[k] == nil {
				several[k] = map[string]bool{}
			}
			several[k][LogName(it.Item)] = true
		}
		for _, it := range page.Items {
			if len(several[ItemHash(it.Item)]) > 1 {
				delete(go_, LogName(it.Item))
			}
		}
	}
	for _, k := range []string{"entries", "pending", "said", "outcomes"} {
		if !reflect.DeepEqual(m2[k], theirs[k]) {
			a, _ := json.MarshalIndent(m2[k], "", " ")
			c, _ := json.MarshalIndent(theirs[k], "", " ")
			t.Errorf("%s differ\njudge: %s\ngate:  %s", k, a, c)
		}
	}
}

func TestShadow(t *testing.T) {
	logf := os.Getenv("SHADOW_LOG")
	if logf == "" {
		t.Skip("no SHADOW_LOG")
	}
	raw, err := os.ReadFile(logf)
	if err != nil {
		t.Fatal(err)
	}
	cfg := &Config{Release: os.Getenv("SHADOW_RELEASE")}
	if m := os.Getenv("SHADOW_MEMBERS"); m != "" {
		b, err := os.ReadFile(m)
		if err != nil {
			t.Fatal(err)
		}
		if err := json.Unmarshal(b, &cfg.Members); err != nil {
			t.Fatal(err)
		}
	}
	voters := strings.Split(os.Getenv("SHADOW_VOTERS"), ",")
	if os.Getenv("SHADOW_VOTERS") == "" {
		voters = []string{"node1", "node2"}
	}
	shadow(t, raw, cfg, voters, os.Getenv("SHADOW_RUST"), os.Getenv("SHADOW_GO_OUT"))
}

// The synthetic log vectors.rs wrote (claims raced, acks out of order, a
// forgery, recoveries, invites, grants reused, libraries fought over,
// leaves old and new, junk), against testdata/shadow-gate.json: what the
// gate's own replay (servers/gate directory.rs shadow_dump) made of it
func TestShadowFixture(t *testing.T) {
	raw, err := os.ReadFile("testdata/vectors.json")
	if err != nil {
		t.Fatal(err)
	}
	var v struct {
		Release string          `json:"release"`
		Log     json.RawMessage `json:"shadow_log"`
	}
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	shadow(t, v.Log, &Config{Release: v.Release}, []string{"node1", "node2"}, "testdata/shadow-gate.json", "")
}
