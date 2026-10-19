package state

// Joining: a box the release does not list joins the agreed state on its
// own word - the key its TPM holds - as a stranger. Its name comes from its
// key, so it cannot take another box's; it gets no lease (the rules name
// the boxes that may hold each), so it runs no service and holds only the
// copies the plan gives it, which are ciphertext. How many may join, and
// how fast, is bounded, and one unheard for thirty days is gone again
// (docs/dev/joining.md).
//
// Below the hardware minimum (ident.Minimum) a box can't join at all.
//
// A box a member set up carries the member's join ticket, a biscuit one of
// their devices signed. It joins with the ticket's claim, never the ticket
// (ident.Claim): the ticket is a bearer token, and all of this is for every
// box to read. Here the claim is held to its id and to the box holding the
// ticket; the join puts the box's key and the claim in the directory log
// ({"joined": ...}, under the box's name), where the identity judge
// (state/ident) holds it to the member's devices when their root enlists
// the box. Until then, and without a ticket, the box is a stranger.

import (
	"bytes"
	"commonty/control/state/ident"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
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
	// its address on the fleet's network, as the box running the
	// network's control server gave it (a netaddr request), never its own
	// word: what its store's name points at
	IPv4 uint32 `json:"ipv4,omitempty"`
	// the id of the member's join ticket it came with
	Ticket string `json:"ticket,omitempty"`
}

// NetAddr: where a joined box is on the fleet's network, said by a box of
// the release that runs the network's control server
type NetAddr struct {
	Name string `json:"name"`
	IPv4 uint32 `json:"ipv4"`
}

// the fleet's network: 100.64.0.0/10
func fleetIPv4(ip uint32) bool { return ip>>22 == 100<<2|1 }

// JoinReq is a join's body; the request is signed by Key itself.
type JoinReq struct {
	Key       string `json:"key"`
	Recipient string `json:"recipient"`
	House     string `json:"house"`
	// what it has, as its heartbeat will say (ident.Minimum reads it)
	Facts map[string]float64 `json:"facts"`
	// its member's join ticket's claim, or nothing for a stranger
	Claim *ident.Claim `json:"claim,omitempty"`
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
	// MaxTicket bounds a join ticket's signed block: a few facts and checks
	MaxTicket = 2048
	// maxJoinFacts bounds what a join says of itself
	maxJoinFacts = 512
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
	return ident.BoxName(spki), JoinedIDBase | binary.BigEndian.Uint64(b[:]), nil
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
	if len(j.Facts) > maxJoinFacts {
		return fmt.Errorf("%d facts; at most %d", len(j.Facts), maxJoinFacts)
	}
	if why := ident.Minimum(j.Facts); why != "" {
		return fmt.Errorf("refused: this box %s", why)
	}
	if c := j.Claim; c != nil {
		if len(c.Block) > MaxTicket || len(c.Sig) > 128 || len(c.ID) > 64 {
			return fmt.Errorf("a ticket's claim of more than %d bytes", MaxTicket)
		}
		if _, err := ident.CheckClaim(c, j.Key); err != nil {
			return fmt.Errorf("refused: its ticket: %w", err)
		}
		// a ticket goes with one box, by its id: another with the same id
		// is a copy, or one made to look like it
		for _, o := range s.Joined {
			if o.Ticket == c.ID {
				return errors.New("that ticket came with another box")
			}
		}
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
	if c := j.Claim; c != nil {
		s.Joined[r.Box].Ticket = c.ID
		rec, _ := json.Marshal(map[string]map[string]string{"joined": {"key": j.Key, "ticket": c.ID, "block": c.Block}})
		s.Directory = append(s.Directory, DirItem{Height: s.Height, Time: now, Box: r.Box, Item: rec})
	}
}

// joinedRecord: whether a directory item a box sends says "joined" at its
// top, which only the state machine writes
func joinedRecord(body []byte) bool {
	d := json.NewDecoder(bytes.NewReader(body))
	if t, err := d.Token(); err != nil || t != json.Delim('{') {
		return false
	}
	for d.More() {
		k, err := d.Token()
		if err != nil {
			return true
		}
		if k == "joined" {
			return true
		}
		var skip json.RawMessage
		if d.Decode(&skip) != nil {
			return true
		}
	}
	return false
}

// fleetRelease: the newest release a box of the release says it runs; a
// joined box's word never counts
func (s *State) fleetRelease() float64 {
	var r float64
	for name, b := range s.Boxes {
		if _, joined := s.Joined[name]; !joined && b != nil {
			r = max(r, b.Facts["release"])
		}
	}
	return r
}

// IdentBoxes: what the identity view reads of each box (ident.Boxes), from
// this state
func (s *State) IdentBoxes() ident.Boxes {
	fleet := s.fleetRelease()
	return func(name string) (ident.BoxInfo, bool) {
		b := s.Boxes[name]
		if b == nil {
			return ident.BoxInfo{}, false
		}
		return ident.BoxInfo{LastSeen: b.LastSeen, Facts: b.Facts, Fleet: fleet}, true
	}
}

func (s *State) checkNetAddr(cfg *Config, r Request) error {
	if _, listed := cfg.Boxes[r.Box]; !listed {
		return errors.New("only a box of the release says where a box is on the network")
	}
	var a NetAddr
	if err := json.Unmarshal(r.Body, &a); err != nil {
		return err
	}
	if s.Joined[a.Name] == nil {
		return fmt.Errorf("%s has not joined", a.Name)
	}
	if !fleetIPv4(a.IPv4) {
		return errors.New("not an address on the fleet's network")
	}
	return nil
}

// dropGone forgets strangers unheard for JoinedGoneAfter, but not one whose
// last word was a release that failed on it: a box down by our fault keeps
// its place (and its member's access, ident.OurFault) until
// ident.OurFaultFor
func (s *State) dropGone(now int64) {
	fleet := s.fleetRelease()
	for name, j := range s.Joined {
		last := j.At
		b := s.Boxes[name]
		if b != nil && b.LastSeen > last {
			last = b.LastSeen
		}
		ours := b != nil && ident.OurFault(b.Facts, fleet)
		if now-last > JoinedGoneAfter && !ours || now-last > ident.OurFaultFor*1000 {
			delete(s.Joined, name)
			delete(s.Boxes, name)
		}
	}
	if len(s.Joined) == 0 {
		s.Joined = nil
	}
}
