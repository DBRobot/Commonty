package state

// Joining: a box the release does not list joins the agreed state on its
// own word - the key its TPM holds - as a stranger. Its name comes from its
// key, so it cannot take another box's; it gets no lease (the rules name
// the boxes that may hold each), so it runs no service and holds only the
// copies the plan gives it, which are ciphertext. How many may join, and
// how fast, is bounded, and one unheard for thirty days is gone again
// (docs/dev/joining.md).
//
// Below the hardware minimum (ident.Minimum) a box can't join at all, nor
// without proving its TPM (docs/dev/attestation.md): its join carries the
// attestation's Begin, judged at once (the EK's chain to its maker, Secure
// Boot, the release's boot), and the box is joined only once its answer to
// the box voters' challenges holds. Until then it is Joining; refused, the
// reason is kept (Refused), and said to its member where the join carried
// their ticket.
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
	"commonty/control/state/attest"
	"commonty/control/state/ident"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"fmt"
	"regexp"
	"sort"
	"strings"
	"sync"
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
	// joined on its word alone, under the release's test switch
	// (attest.Test.UnattestedJoin): never in a real fleet
	Unattested bool `json:"unattested,omitempty"`
}

// Joining is a join waiting for its TPM's proof (Attest.Pending)
type Joining struct {
	Req JoinReq `json:"req"` // without its Begin, which the attestation keeps
	At  int64   `json:"at"`
}

// Refused is why a box's join was refused, kept a week for the box and its
// member to read
type Refused struct {
	Why string `json:"why"`
	At  int64  `json:"at"`
	// its ticket and that ticket's member, only where the identity state
	// held the ticket good when it was refused (ticketOwner)
	Ticket string `json:"ticket,omitempty"`
	Owner  string `json:"owner,omitempty"`
	// the newest Seq of that box's judged: a replay at or below is stale
	Seq uint64 `json:"seq,omitempty"`
}

// Refusal is a join (or its proof) refused for what the box is - its
// hardware, its TPM, its boot - rather than for how it was sent: the
// agreed state keeps why (Refused). Why is in words for the person who set
// the box up (ident.Minimum, attest.Why).
type Refusal struct {
	Why string
	Err error
}

func (r *Refusal) Error() string {
	if r.Err != nil {
		return "refused: " + r.Why + " (" + r.Err.Error() + ")"
	}
	return "refused: this box " + r.Why
}

// Sentence: a refusal's why as a sentence about the box - the minimum's
// say what it needs ("needs 8 GiB of memory"), the rest are whole
func Sentence(why string) string {
	if strings.HasPrefix(why, "needs ") {
		return "this box " + why
	}
	return why
}

func (r *Refusal) Unwrap() error { return r.Err }

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
	// the start of its TPM's proof (state/attest)
	Attest *attest.Begin `json:"attest,omitempty"`
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
	// MaxRefused: refusals kept at once, a ticketless one given up first;
	// RefusedPerHour: new ones with a ticket the fleet keeps in an hour (each
	// told to the identity judge), one a ticket; StrangersRefusedPerHour:
	// ticketless ones, a bucket of their own, so junk joins cannot spend
	// what a member's refusal needs; RefusedFor: how long one is kept, ms.
	// A box sends its join again every minute: the same refusal is kept
	// again only after a day
	MaxRefused              = 64
	RefusedPerHour          = 16
	StrangersRefusedPerHour = 4
	RefusedFor              = 7 * 24 * 3600 * 1000
	refusedAgain            = 24 * 3600 * 1000
	// JoinFresh: a join's Seq is its sender's clock, ms (dd-control's
	// sequence starts there); one further than this from the agreed time is
	// stale, and so is one not above the last a refusal of that box kept:
	// an old signed join replayed is never judged again
	JoinFresh = 10 * 60 * 1000
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
	if err := s.freshJoin(name, r, now); err != nil {
		return err
	}
	if j.Recipient == "" || len(j.Recipient) > 512 || !houseName.MatchString(j.House) {
		return errors.New("a join names its recipient and its house")
	}
	if len(j.Facts) > maxJoinFacts {
		return fmt.Errorf("%d facts; at most %d", len(j.Facts), maxJoinFacts)
	}
	if jn := s.Joining[name]; jn != nil {
		return fmt.Errorf("%s is proving its TPM already", name)
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
		for _, o := range s.Joining {
			if o.Req.Claim != nil && o.Req.Claim.ID == c.ID {
				return errors.New("that ticket came with another box")
			}
		}
	}
	if len(s.Joined)+len(s.Joining) >= MaxJoined {
		return fmt.Errorf("%d boxes have joined; no more", len(s.Joined)+len(s.Joining))
	}
	if now-s.JoinWindow < 3_600_000 && s.JoinCount >= JoinsPerHour {
		return fmt.Errorf("%d boxes joined this hour", s.JoinCount)
	}
	return JoinRefusal(cfg, s, name, j, now)
}

// freshJoin: a join is its box's now - its Seq within JoinFresh of the
// agreed time and above any of that box's kept (a refusal's, a join's)
func (s *State) freshJoin(box string, r Request, now int64) error {
	if d := int64(r.Seq) - now; d < -JoinFresh || d > JoinFresh {
		return fmt.Errorf("%w: a join's sequence %d is not within ten minutes of %d", ErrStale, r.Seq, now)
	}
	if rf := s.Refused[box]; rf != nil && r.Seq <= rf.Seq {
		return ErrStale
	}
	if b := s.Boxes[box]; b != nil && r.Seq <= b.Seq {
		return ErrStale
	}
	return nil
}

// JoinRefusal: why a join of box with j would be refused for what the box
// is, or nil. The validators judge a join by it, and the box itself before
// it asks (dd-control), so it can say why where it can be read. s may be
// nil: the box's own check, before it knows the agreed state.
func JoinRefusal(cfg *Config, s *State, box string, j JoinReq, now int64) error {
	if why := ident.Minimum(j.Facts); why != "" {
		return &Refusal{Why: why}
	}
	if unattested(cfg) {
		return nil
	}
	if j.Attest == nil {
		return &Refusal{Why: "it sent no proof of its TPM", Err: errors.New("no Begin")}
	}
	if s == nil {
		s = New()
	}
	if err := s.CheckAttestOpen(cfg, box, j.Key, *j.Attest, now); err != nil {
		return &Refusal{Why: attest.Why(err), Err: err}
	}
	if err := attest.JudgeBoot(cfg.Attest, *j.Attest); err != nil {
		return &Refusal{Why: attest.Why(err), Err: err}
	}
	return nil
}

// unattested: the release's test switch lets a box join on its word
func unattested(cfg *Config) bool {
	return cfg.Attest != nil && cfg.Attest.Test != nil && cfg.Attest.Test.UnattestedJoin
}

// applyJoin: the box proves its TPM next (Joining), or, under the test
// switch alone, is joined now
func (s *State) applyJoin(cfg *Config, r Request, now int64) {
	var j JoinReq
	_ = json.Unmarshal(r.Body, &j)
	if now-s.JoinWindow >= 3_600_000 {
		s.JoinWindow, s.JoinCount = now, 0
	}
	s.JoinCount++
	if unattested(cfg) {
		s.joined(r.Box, j, now, true)
		return
	}
	s.AttestOpen(cfg, r.Box, j.Key, *j.Attest, now)
	j.Attest = nil
	if s.Joining == nil {
		s.Joining = map[string]*Joining{}
	}
	s.Joining[r.Box] = &Joining{Req: j, At: now}
}

// joined: the box is one of the fleet's strangers from now, and its
// member's to enlist where it came with their ticket
func (s *State) joined(box string, j JoinReq, now int64, unproven bool) {
	_, id, _ := JoinName(j.Key)
	if s.Joined == nil {
		s.Joined = map[string]*Joined{}
	}
	s.Joined[box] = &Joined{ID: id, Key: j.Key, Recipient: j.Recipient, House: j.House, At: now, Unattested: unproven}
	if c := j.Claim; c != nil {
		s.Joined[box].Ticket = c.ID
		rec, _ := json.Marshal(map[string]map[string]string{"joined": {"key": j.Key, "ticket": c.ID, "block": c.Block}})
		s.Directory = append(s.Directory, DirItem{Height: s.Height, Time: now, Box: box, Item: rec})
	}
	delete(s.Refused, box)
}

// attested: a box Joining that has just proved its TPM joins
func (s *State) attested(box string, now int64) {
	jn := s.Joining[box]
	if jn == nil || s.Attested(box) == nil {
		return
	}
	delete(s.Joining, box)
	if len(s.Joining) == 0 {
		s.Joining = nil
	}
	s.joined(box, jn.Req, now, false)
}

// ticketOwner: the member whose ticket claim is, where the identity state
// the log holds now says a current device of theirs signed it, for them,
// and it is unexpired and unused (ident.TicketOwner); "" otherwise. The
// claim's id is the joining box's word: a ticket counts only once its
// member's signature on it does.
func (s *State) ticketOwner(cfg *Config, claim *ident.Claim, now int64) string {
	if claim == nil || cfg.Identity == nil {
		return ""
	}
	rp := identReplayOf(cfg)
	rp.mu.Lock()
	defer rp.mu.Unlock()
	n := len(s.Directory)
	if rp.r == nil || n < rp.n || (rp.n > 0 && !sameItem(s.Directory[rp.n-1], rp.last)) {
		rp.r = ident.NewReplayer(cfg.Identity, cfg.BoxVoters())
	}
	ids := rp.r.CatchN(n, func(i int) ident.Item {
		d := s.Directory[i]
		return ident.Item{Time: d.Time, Box: d.Box, Item: d.Item, Boxes: d.Boxes}
	})
	rp.n = n
	if n > 0 {
		rp.last = s.Directory[n-1]
	}
	if owner, id := ids.TicketOwner(cfg.Identity, claim.Block, uint64(now/1000)); id == claim.ID {
		return owner
	}
	return ""
}

// identReplay: the identity state judged from a log, kept per release so
// each refusal judges only what is new (its own, never the computers'
// replay, whose state its callers read outside its lock)
type identReplay struct {
	mu   sync.Mutex
	r    *ident.Replayer
	n    int
	last DirItem
}

var (
	identReplaysMu sync.Mutex
	identReplays   = map[*Config]*identReplay{}
)

func identReplayOf(c *Config) *identReplay {
	identReplaysMu.Lock()
	defer identReplaysMu.Unlock()
	r := identReplays[c]
	if r == nil {
		r = &identReplay{}
		identReplays[c] = r
	}
	return r
}

// refuse keeps why box was refused (claim: the ticket it came with, if
// any), within the bounds above. A ticket counts only where its member's
// device signed it (ticketOwner): then it is in the ticketed bound, one a
// ticket and member an hour, the identity judge is told too ({"refused":
// ...}), and says it to the ticket's member alone (ident's View.Refused).
// Any other claim, which anyone can make up, is a stranger's.
func (s *State) refuse(cfg *Config, box string, claim *ident.Claim, key, why string, seq uint64, now int64) {
	old := s.Refused[box]
	if old != nil {
		old.Seq = max(old.Seq, seq) // a replay of what was judged is stale either way
	}
	if old != nil && old.Why == why && now-old.At < refusedAgain {
		return
	}
	owner := s.ticketOwner(cfg, claim, now)
	if owner == "" {
		claim = nil
		if now-s.StrangerWindow >= 3_600_000 {
			s.StrangerWindow, s.StrangerCount = now, 0
		}
		if s.StrangerCount >= StrangersRefusedPerHour {
			return
		}
		s.StrangerCount++
	} else {
		// one a ticket an hour, whatever box comes with it
		for b, x := range s.Refused {
			if b != box && x.Ticket == claim.ID && x.Owner == owner && now-x.At < 3_600_000 {
				return
			}
		}
		if now-s.RefuseWindow >= 3_600_000 {
			s.RefuseWindow, s.RefuseCount = now, 0
		}
		if s.RefuseCount >= RefusedPerHour {
			return
		}
		s.RefuseCount++
	}
	if s.Refused == nil {
		s.Refused = map[string]*Refused{}
	}
	rf := &Refused{Why: why, At: now, Seq: seq}
	if old != nil {
		rf.Seq = max(rf.Seq, old.Seq)
	}
	if claim != nil {
		rf.Ticket, rf.Owner = claim.ID, owner
	}
	s.Refused[box] = rf
	for len(s.Refused) > MaxRefused {
		// the oldest ticketless one first: a member's never goes for junk
		oldest, ticketless := "", false
		for b, x := range s.Refused {
			tl := x.Ticket == ""
			o := s.Refused[oldest]
			if oldest == "" || tl && !ticketless || tl == ticketless && (x.At < o.At || x.At == o.At && b < oldest) {
				oldest, ticketless = b, tl
			}
		}
		delete(s.Refused, oldest)
	}
	if claim != nil {
		rec, _ := json.Marshal(map[string]map[string]string{"refused": {"key": key, "ticket": claim.ID, "block": claim.Block, "why": why}})
		s.Directory = append(s.Directory, DirItem{Height: s.Height, Time: now, Box: box, Item: rec})
	}
}

// refused: a request Check refused for what the box is (a Refusal), kept
func (s *State) refused(cfg *Config, r Request, rf *Refusal, now int64) {
	switch r.Kind {
	case "join":
		var j JoinReq
		if json.Unmarshal(r.Body, &j) != nil {
			return
		}
		s.refuse(cfg, r.Box, j.Claim, j.Key, rf.Why, r.Seq, now)
	case "attest-response":
		// its proof did not hold: it starts again, from its join
		jn := s.Joining[r.Box]
		if jn == nil {
			return
		}
		s.refuse(cfg, r.Box, jn.Req.Claim, jn.Req.Key, rf.Why, r.Seq, now)
		s.dropJoining(r.Box)
	}
}

// dropJoining: a join that did not prove its TPM is gone, and every trace
// of the box with it
func (s *State) dropJoining(box string) {
	delete(s.Joining, box)
	if s.Attest != nil {
		delete(s.Attest.Pending, box)
	}
	if s.Joined[box] == nil {
		delete(s.Boxes, box)
	}
	if len(s.Joining) == 0 {
		s.Joining = nil
	}
}

// expireJoining: joins whose proof did not come within the attestation's
// hour are refused, and refusals a week old are forgotten
func (s *State) expireJoining(cfg *Config, now int64) {
	var gone []string
	for box, jn := range s.Joining {
		if s.Attest == nil || s.Attest.Pending[box] == nil || now-jn.At >= attest.TTL {
			gone = append(gone, box)
		}
	}
	sort.Strings(gone)
	for _, box := range gone {
		jn := s.Joining[box]
		s.refuse(cfg, box, jn.Req.Claim, jn.Req.Key, "it did not finish proving its TPM within an hour", 0, now)
		s.dropJoining(box)
	}
	for box, rf := range s.Refused {
		if now-rf.At >= RefusedFor {
			delete(s.Refused, box)
		}
	}
	if len(s.Refused) == 0 {
		s.Refused = nil
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
		if k == "joined" || k == "refused" {
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
		// a box of the release's proves nothing here; a joined one has
		// (its record stands), unless it joined under the test switch
		j := s.Joined[name]
		unproven := j != nil && !j.Unattested && s.Attested(name) == nil
		return ident.BoxInfo{LastSeen: b.LastSeen, Facts: b.Facts, Fleet: fleet, Unattested: unproven}, true
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
			// its TPM and its Secure Boot key may be another box's now
			if s.Attest != nil {
				s.Attest.Forget(name)
			}
		}
	}
	if len(s.Joined) == 0 {
		s.Joined = nil
	}
}
