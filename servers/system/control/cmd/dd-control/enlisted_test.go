package main

import (
	"os"
	"path/filepath"
	"testing"

	"commonty/control/place"
	"commonty/control/state"
	"commonty/control/state/ident"
)

func TestEnlistedFollowsTheAgreedOwner(t *testing.T) {
	path := filepath.Join(t.TempDir(), "enlisted.json")
	writeEnlisted(path, "")
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatal("a box never enlisted changed what it builds from")
	}
	writeEnlisted(path, "ann")
	raw, _ := os.ReadFile(path)
	if string(raw) != `{"owner":"ann"}` {
		t.Fatalf("%s", raw)
	}
	before, _ := os.Stat(path)
	writeEnlisted(path, "ann")
	if after, _ := os.Stat(path); !os.SameFile(before, after) {
		t.Fatal("rewritten with nothing changed")
	}
	writeEnlisted(path, "")
	raw, _ = os.ReadFile(path)
	if string(raw) != `{"owner":null}` {
		t.Fatalf("%s", raw)
	}
}

func forgetPlacer(t *testing.T) (*placer, *[]string, *state.State) {
	t.Helper()
	p, calls := newPlacer(t)
	p.noLeases = true
	p.seen = filepath.Join(t.TempDir(), "seen")
	p.sawOwner("ann")
	s := state.New()
	s.Time = 100 * place.ForgetHold
	return p, calls, s
}

func forgetPlan(p *placer, s *state.State, member string) place.Plan {
	return place.Plan{Forget: map[string]ident.Ended{p.box: {Member: member, At: s.Time - place.ForgetHold}}}
}

func TestAFormerOwnerIsForgottenOnce(t *testing.T) {
	p, calls, s := forgetPlacer(t)
	plan := forgetPlan(p, s, "ann")
	p.forget(plan, s, "")
	if len(*calls) != 1 || (*calls)[0] != "start --no-block dd-forget-owner@ann.service" {
		t.Fatalf("calls %v", *calls)
	}
	p.forget(plan, s, "")
	if len(*calls) != 1 {
		t.Fatalf("started again at once: %v", *calls)
	}
}

// a plan that is wrong, or a state read wrong, removes nothing
func TestABadPlanRemovesNothing(t *testing.T) {
	cases := map[string]func(p *placer, s *state.State) (place.Plan, string){
		"a plan that says nothing": func(*placer, *state.State) (place.Plan, string) {
			return place.Plan{}, ""
		},
		"another box's word": func(p *placer, s *state.State) (place.Plan, string) {
			return place.Plan{Forget: map[string]ident.Ended{"other": {Member: "ann", At: s.Time - place.ForgetHold}}}, ""
		},
		"a member it never knew": func(p *placer, s *state.State) (place.Plan, string) {
			return forgetPlan(p, s, "bea"), ""
		},
		"its owner by the same state": func(p *placer, s *state.State) (place.Plan, string) {
			return forgetPlan(p, s, "ann"), "ann"
		},
		"a word that has not stood": func(p *placer, s *state.State) (place.Plan, string) {
			return place.Plan{Forget: map[string]ident.Ended{p.box: {Member: "ann", At: s.Time}}}, ""
		},
		"a word from the future": func(p *placer, s *state.State) (place.Plan, string) {
			return place.Plan{Forget: map[string]ident.Ended{p.box: {Member: "ann", At: s.Time + place.ForgetHold}}}, ""
		},
		"a name that is a path": func(p *placer, s *state.State) (place.Plan, string) {
			return forgetPlan(p, s, "../ann"), ""
		},
		"a box of the release": func(p *placer, s *state.State) (place.Plan, string) {
			p.noLeases = false
			return forgetPlan(p, s, "ann"), ""
		},
	}
	for name, c := range cases {
		p, calls, s := forgetPlacer(t)
		plan, owner := c(p, s)
		p.forget(plan, s, owner)
		if len(*calls) != 0 {
			t.Errorf("%s: %v", name, *calls)
		}
	}
}
