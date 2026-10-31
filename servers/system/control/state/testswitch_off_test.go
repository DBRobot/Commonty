//go:build !testswitches

package state

import (
	"testing"

	"commonty/control/state/attest"
)

// a release whose config sets every switch, in a box's build: no box joins
// on its word, and one marked so in the state counts as nothing proved
func TestABoxsBuildLetsNoBoxJoinOnItsWord(t *testing.T) {
	c := &Config{Attest: &attest.Config{Test: &attest.Test{SoftwareTPM: true, NoSecureBoot: true, UnattestedJoin: true}}}
	if Unattested(c) {
		t.Fatal("unattestedJoin read")
	}
	s := New()
	s.Machine = 3
	s.Joined = map[string]*Joined{"jabc": {Unattested: true}}
	if s.Proved("jabc", 1) {
		t.Fatal("proved by a test switch")
	}
}
