package state

// Joining: a box the release does not list joins the agreed state on its
// own word - the key its TPM holds - as a stranger. Its name comes from its
// key, so it cannot take another box's; it gets no lease (the rules name
// the boxes that may hold each), so it runs no service and holds only the
// copies the plan gives it, which are ciphertext. How many may join, and
// how fast, is bounded, and one unheard for thirty days is gone again
// (docs/dev/joining.md).

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"regexp"
)

type Joined struct {
	ID        uint64 `json:"id"`        // its address on the relay
	Key       string `json:"key"`       // its TPM-held key, SPKI, base64
	Recipient string `json:"recipient"` // its secrets key's age recipient: what copies' keys are sealed to
	House     string `json:"house"`     // as it says (dd-locate)
	At        int64  `json:"at"`        // when it joined, agreed ms
}

// JoinReq is a join's body; the request is signed by Key itself.
type JoinReq struct {
	Key       string `json:"key"`
	Recipient string `json:"recipient"`
	House     string `json:"house"`
}

const (
	// MaxJoined: strangers at once
	MaxJoined = 32
	// JoinsPerHour: joins the fleet takes in an hour, all boxes together
	JoinsPerHour = 4
	// JoinedGoneAfter: a stranger unheard this long is dropped, ms
	JoinedGoneAfter = 30 * 24 * 3600 * 1000
	// JoinedIDBase: joined boxes' relay addresses, above the release's and
	// below the validators' and the relay's own
	JoinedIDBase = 1 << 40
)

var houseName = regexp.MustCompile(`^[a-z0-9-]{1,64}$`)

// JoinName is the name and relay address a key joins under: from the key
// alone, so the same key always has them and no other key can
func JoinName(spki string) (string, uint64, error) {
	der, err := base64.StdEncoding.DecodeString(spki)
	if err != nil || len(der) == 0 {
		return "", 0, errors.New("not a key")
	}
	h := sha256.Sum256(der)
	var b [8]byte
	copy(b[3:], h[:5])
	return "j" + hex.EncodeToString(h[:5]), JoinedIDBase | binary.BigEndian.Uint64(b[:]), nil
}

// BoxKey is a box's key: from the release's list, else a joined box's
func (s *State) BoxKey(cfg *Config, name string) (string, bool) {
	if b, ok := cfg.Boxes[name]; ok {
		return b.Key, true
	}
	if j, ok := s.Joined[name]; ok {
		return j.Key, true
	}
	return "", false
}

func (s *State) checkJoin(cfg *Config, verify Verify, r Request, now int64) error {
	var j JoinReq
	if err := json.Unmarshal(r.Body, &j); err != nil {
		return err
	}
	name, _, err := JoinName(j.Key)
	if err != nil {
		return err
	}
	if r.Box != name {
		return fmt.Errorf("a key joins as %s, not %s", name, r.Box)
	}
	if _, ok := cfg.Boxes[name]; ok {
		return fmt.Errorf("%s is a box of the release", name)
	}
	if _, ok := s.Joined[name]; ok {
		return fmt.Errorf("%s has joined already", name)
	}
	for _, b := range cfg.Boxes {
		if b.Key == j.Key {
			return errors.New("a box of the release does not join again")
		}
	}
	if !verify(j.Key, r.Bytes(), r.Sig) {
		return ErrBadSig
	}
	if j.Recipient == "" || len(j.Recipient) > 512 || !houseName.MatchString(j.House) {
		return errors.New("a join names its recipient and its house")
	}
	if len(s.Joined) >= MaxJoined {
		return fmt.Errorf("%d boxes have joined; no more", len(s.Joined))
	}
	if now-s.JoinWindow < 3_600_000 && s.JoinCount >= JoinsPerHour {
		return fmt.Errorf("%d boxes joined this hour", s.JoinCount)
	}
	return nil
}

func (s *State) applyJoin(r Request, now int64) {
	var j JoinReq
	_ = json.Unmarshal(r.Body, &j)
	_, id, _ := JoinName(j.Key)
	if now-s.JoinWindow >= 3_600_000 {
		s.JoinWindow, s.JoinCount = now, 0
	}
	s.JoinCount++
	if s.Joined == nil {
		s.Joined = map[string]*Joined{}
	}
	s.Joined[r.Box] = &Joined{ID: id, Key: j.Key, Recipient: j.Recipient, House: j.House, At: now}
}

// dropGone forgets strangers unheard for JoinedGoneAfter
func (s *State) dropGone(now int64) {
	for name, j := range s.Joined {
		last := j.At
		if b := s.Boxes[name]; b != nil && b.LastSeen > last {
			last = b.LastSeen
		}
		if now-last > JoinedGoneAfter {
			delete(s.Joined, name)
			delete(s.Boxes, name)
		}
	}
	if len(s.Joined) == 0 {
		s.Joined = nil
	}
}
