package ident

// Signing out everywhere (docs/dev/identity-state.md, "signout"): a
// member's root says every sign-in made before a time is over, on every box.
// Its own item, beside the entry, so the entry and everything read from it
// (status, access, a remote computer's lease) are the same whether a judge
// knows this kind or not.

import (
	"encoding/json"
	"errors"
	"fmt"
)

// SignOut is the newest sign-out a member's root said
type SignOut struct {
	ID string `json:"id"` // the root that said it
	At uint64 `json:"at"` // its own date, seconds
	// the one sign-in it keeps: the session id of the browser or app that
	// asked (a fleet sign-in's id is the same on every box)
	Keep   string          `json:"keep,omitempty"`
	Agreed uint64          `json:"agreed"` // seconds
	Item   json.RawMessage `json:"item"`
}

// the longest session id a sign-out may keep (the gate's are 24 base64url)
const keepMax = 64

func validKeep(k string) bool {
	if len(k) > keepMax {
		return false
	}
	for _, c := range []byte(k) {
		ok := c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '-' || c == '_'
		if !ok {
			return false
		}
	}
	return true
}

// signout: signed by the root on file whose id is `member`, dated like a
// leave, and newer than the sign-out on record, so one sent again, or an
// older one, changes nothing. A member who left or was removed may still
// say it: it only ever ends sessions.
func (s *State) signout(env Envelope, body []byte, raw json.RawMessage, at uint64) error {
	var o struct {
		Member string `json:"member"`
		At     uint64 `json:"at"`
		Keep   string `json:"keep,omitempty"`
	}
	if err := strict(body, &o); err != nil {
		return fmt.Errorf("not an item: %w", err)
	}
	again, _ := json.Marshal(&o)
	if err := sameAs(body, again); err != nil {
		return err
	}
	if !validKeep(o.Keep) {
		return errors.New("not an item: keep is not a session id")
	}
	who := s.rootOf(o.Member, at)
	if who == nil {
		return errors.New("no member with that root")
	}
	if err := VerifyRoot(&env, &who.Signed.Entry); err != nil {
		return fmt.Errorf("refused: %w", err)
	}
	if err := timely(o.At, at); err != nil {
		return err
	}
	name := who.Signed.Entry.Name
	if old := s.SignedOut[name]; old != nil && contains(who.Signed.Entry.MemberIDs(), old.ID) {
		if old.At == o.At && old.Keep == o.Keep {
			return nil
		}
		if o.At <= old.At {
			return errors.New("older than the sign-out on record")
		}
	}
	s.SignedOut[name] = &SignOut{ID: o.Member, At: o.At, Keep: o.Keep, Agreed: at, Item: raw}
	return nil
}

// signedOut: when every sign-in of the person holding the name now ended
// (ms), and the one it keeps. A sign-out by a root that is not theirs (a
// guest's name gone to someone else) is not theirs. Never later than it was
// agreed, so one dated ahead ends nothing begun after the fleet took it.
func (s *State) signedOut(name string, e *Entry) (uint64, string) {
	o := s.SignedOut[name]
	if o == nil || !contains(e.MemberIDs(), o.ID) {
		return 0, ""
	}
	return min(o.At, o.Agreed) * 1000, o.Keep
}
