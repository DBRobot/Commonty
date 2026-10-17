package ident

// The judge: each directory item, judged once, as it is agreed, into the
// agreed identity state (docs/dev/identity-state.md). Deterministic like the
// rest of the state machine: no clock but the agreed time, no I/O, and every
// map walked in sorted order. These are the gate's replay rules
// (servers/gate/src/directory.rs replay_one, settle, commit, check_grant,
// claims_free; leaving.rs record), moved.

import (
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"strings"
)

// Config is the release's part: the key that signs invites, and the member
// list (fleet/members.json at the commit released)
type Config struct {
	Release string `json:"release"`
	Members struct {
		Members   []string          `json:"members"`
		Revoked   []string          `json:"revoked"`
		Removed   map[string]uint64 `json:"removed,omitempty"`
		Suspended map[string]string `json:"suspended,omitempty"`
	} `json:"members"`
}

// Held is an entry the log has given a name
type Held struct {
	Signed *Signed         `json:"-"`
	Item   json.RawMessage `json:"item"`
	Agreed uint64          `json:"agreed"` // seconds
}

// Pending is a claim to a name nobody holds, waiting for every voter's ack
type Pending struct {
	Hash string          `json:"hash"`
	Item json.RawMessage `json:"item"`
	At   uint64          `json:"at"`
	s    *Signed
}

// Said is a leaving statement agreed at `Agreed`
type Said struct {
	Leave  Leave           `json:"leave"`
	Item   json.RawMessage `json:"item"`
	Agreed uint64          `json:"agreed"`
}

// Enlisted is a box a member's root named as theirs
type Enlisted struct {
	Member  string          `json:"member"` // the name
	Box     string          `json:"box"`
	Key     string          `json:"key"`
	Ticket  string          `json:"ticket"`
	Expires uint64          `json:"expires"`
	Agreed  uint64          `json:"agreed"`
	Item    json.RawMessage `json:"item"`
}

type Handover struct {
	Before json.RawMessage `json:"before"`
	After  json.RawMessage `json:"after"`
}

// State is the agreed identity state
type State struct {
	Entries   map[string]*Held           `json:"entries"`
	Invites   map[string]json.RawMessage `json:"invites,omitempty"` // public key -> the signed invite
	Pending   []*Pending                 `json:"pending,omitempty"`
	Acks      map[string][]string        `json:"acks,omitempty"` // claim hash -> voters, sorted
	Said      map[string]*Said           `json:"said,omitempty"`
	Handovers map[string][]Handover      `json:"handovers,omitempty"`
	Enlisted  map[string]*Enlisted       `json:"enlisted,omitempty"`     // box -> its enlistment
	Outcomes  map[string]string          `json:"outcomes,omitempty"`     // item hash -> "ok" | the refusal
	Order     []string                   `json:"outcomeOrder,omitempty"` // outcome hashes, oldest first
}

// UnmarshalJSON: the state machine copies its state through JSON every
// block, so what an entry is read back from its item as agreed
func (s *State) UnmarshalJSON(b []byte) error {
	type plain State
	var p plain
	if err := json.Unmarshal(b, &p); err != nil {
		return err
	}
	*s = State(p)
	if s.Entries == nil {
		s.Entries = map[string]*Held{}
	}
	if s.Invites == nil {
		s.Invites = map[string]json.RawMessage{}
	}
	if s.Acks == nil {
		s.Acks = map[string][]string{}
	}
	if s.Said == nil {
		s.Said = map[string]*Said{}
	}
	if s.Handovers == nil {
		s.Handovers = map[string][]Handover{}
	}
	if s.Enlisted == nil {
		s.Enlisted = map[string]*Enlisted{}
	}
	if s.Outcomes == nil {
		s.Outcomes = map[string]string{}
	}
	for n, h := range s.Entries {
		sg, err := readSigned(h.Item)
		if err != nil {
			return fmt.Errorf("%s: %w", n, err)
		}
		h.Signed = sg
	}
	for _, p := range s.Pending {
		sg, err := readSigned(p.Item)
		if err != nil {
			return err
		}
		p.s = sg
	}
	return nil
}

// readSigned: an entry as agreed, either format
func readSigned(raw json.RawMessage) (*Signed, error) {
	var probe struct {
		V int `json:"v"`
	}
	_ = json.Unmarshal(raw, &probe)
	if probe.V == 2 {
		return FromEnvelope(raw)
	}
	return FromV1(raw)
}

func New() *State {
	return &State{
		Entries: map[string]*Held{}, Invites: map[string]json.RawMessage{}, Acks: map[string][]string{},
		Said: map[string]*Said{}, Handovers: map[string][]Handover{}, Enlisted: map[string]*Enlisted{},
		Outcomes: map[string]string{},
	}
}

const (
	// how many outcomes are kept, for writers waiting on theirs
	keepOutcomes = 1024
	guestPrefix  = "guest"
	guestTTL     = 15 * 60
	leaveAhead   = 300
	leaveBehind  = 24 * 3600
	maxHandovers = 32
)

func (s *State) outcome(hash string, err error) {
	if _, had := s.Outcomes[hash]; !had {
		s.Order = append(s.Order, hash)
		if len(s.Order) > keepOutcomes {
			delete(s.Outcomes, s.Order[0])
			s.Order = s.Order[1:]
		}
	}
	if err == nil {
		s.Outcomes[hash] = "ok"
	} else {
		s.Outcomes[hash] = err.Error()
	}
}

// entry is the name's agreed entry at `at`: a guest past its time is gone
func (s *State) entry(name string, at uint64) *Held {
	if !ValidName(name) {
		return nil
	}
	h := s.Entries[name]
	if h == nil {
		return nil
	}
	if strings.HasPrefix(name, guestPrefix) && h.Signed.Entry.Updated+guestTTL < at {
		return nil
	}
	return h
}

func (s *State) names() []string {
	ns := make([]string, 0, len(s.Entries))
	for n := range s.Entries {
		ns = append(ns, n)
	}
	sort.Strings(ns)
	return ns
}

// grantTaken is the gate's grant_taken: another root's entry already
// redeemed this invite
func (s *State) grantTaken(key, root string, at uint64) bool {
	for _, n := range s.names() {
		h := s.entry(n, at)
		if h == nil || h.Signed.Entry.Grant == nil {
			continue
		}
		if h.Signed.Entry.Grant.Invite.Invite.PublicKey == key && h.Signed.Entry.Root != root {
			return true
		}
	}
	return false
}

// checkGrant is the gate's check_grant at `at`
func (s *State) checkGrant(cfg *Config, cur *Held, n *Entry, at uint64) error {
	g := n.Grant
	if g == nil {
		return nil
	}
	if cfg.Release == "" {
		return errors.New("this box cannot check invites")
	}
	if err := VerifyGrant(n, cfg.Release); err != nil {
		return fmt.Errorf("grant: %w", err)
	}
	already := false
	if cur != nil && cur.Signed.Entry.Grant != nil {
		a, _ := json.Marshal(cur.Signed.Entry.Grant)
		b, _ := json.Marshal(g)
		already = string(a) == string(b)
	}
	if !already {
		if g.Invite.Invite.Expires <= at {
			return errors.New("that invite has expired")
		}
		if s.grantTaken(g.Invite.Invite.PublicKey, n.Root, at) {
			return errors.New("that code has already been used")
		}
	}
	return nil
}

// claimsFree is the gate's claims_free: a library id belongs to the entry
// that claimed it first
func (s *State) claimsFree(n *Entry, at uint64) error {
	if len(n.Libraries) == 0 {
		return nil
	}
	for _, name := range s.names() {
		if name == n.Name {
			continue
		}
		h := s.entry(name, at)
		if h == nil {
			continue
		}
		for _, theirs := range h.Signed.Entry.Libraries {
			for _, ours := range n.Libraries {
				if ours.ID == theirs.ID {
					return fmt.Errorf("library %s is already %s's", theirs.ID, name)
				}
			}
		}
	}
	return nil
}

// commit is the gate's commit: the accept rule against what is on file at
// `at`, the grant, the libraries, then the entry is the name's
func (s *State) commit(cfg *Config, next *Signed, at uint64) error {
	cur := s.entry(next.Entry.Name, at)
	var curS *Signed
	if cur != nil {
		curS = cur.Signed
	}
	if err := Accept(curS, next); err != nil {
		return fmt.Errorf("refused: %w", err)
	}
	if err := s.checkGrant(cfg, cur, &next.Entry, at); err != nil {
		return err
	}
	if err := s.claimsFree(&next.Entry, at); err != nil {
		return err
	}
	s.Entries[next.Entry.Name] = &Held{Signed: next, Item: next.Raw, Agreed: at}
	if cur != nil && cur.Signed.Entry.Root != next.Entry.Root {
		hs := append(s.Handovers[next.Entry.Name], Handover{Before: cur.Item, After: next.Raw})
		if len(hs) > maxHandovers {
			hs = hs[len(hs)-maxHandovers:]
		}
		s.Handovers[next.Entry.Name] = hs
	}
	return nil
}

// Judge takes one directory item agreed at `at` (seconds), sent by box
// `from`. voters: the validators that are boxes, whose gates acknowledge
// claims while acks are still the rule.
func (s *State) Judge(cfg *Config, voters []string, from string, raw json.RawMessage, at uint64) {
	hash := ItemHash(raw)
	// which item it is, by its exact keys: v1 is one key (entry, invite,
	// ack, leave), as Rust's tagged enum reads it; v2 is an envelope
	if err := noDuplicates(raw); err != nil {
		s.outcome(hash, fmt.Errorf("not an item: %w", err))
		return
	}
	var top map[string]json.RawMessage
	if err := json.Unmarshal(raw, &top); err != nil {
		s.outcome(hash, fmt.Errorf("not an item: %w", err))
		return
	}
	var probe struct {
		V     int
		Entry json.RawMessage
		Inv   json.RawMessage
		Ack   *string
		Leave json.RawMessage
	}
	if v, ok := top["v"]; ok {
		_ = json.Unmarshal(v, &probe.V)
	} else if len(top) == 1 {
		for k, v := range top {
			switch k {
			case "entry":
				probe.Entry = v
			case "invite":
				probe.Inv = v
			case "leave":
				probe.Leave = v
			case "ack":
				var a string
				if json.Unmarshal(v, &a) == nil {
					probe.Ack = &a
				}
			}
		}
	}
	switch {
	case probe.V == 2:
		s.judgeV2(cfg, voters, from, raw, hash, at)
	case probe.Ack != nil:
		if contains(voters, from) {
			got := s.Acks[*probe.Ack]
			if !contains(got, from) {
				got = append(got, from)
				sort.Strings(got)
				s.Acks[*probe.Ack] = got
			}
		}
	case probe.Leave != nil:
		var sl SignedLeave
		if err := strict(probe.Leave, &sl); err != nil {
			s.outcome(hash, fmt.Errorf("not an item: %w", err))
			break
		}
		if err := sameAs(probe.Leave, sl.bytes()); err != nil {
			s.outcome(hash, err)
			break
		}
		s.outcome(hash, s.record(sl.Leave, func(e *Entry) error { return VerifyLeave(&sl, e) }, raw, at))
	case probe.Inv != nil:
		var si SignedInvite
		if err := strict(probe.Inv, &si); err != nil {
			s.outcome(hash, fmt.Errorf("not an item: %w", err))
			break
		}
		if err := sameAs(probe.Inv, si.bytes()); err != nil {
			s.outcome(hash, err)
			break
		}
		s.outcome(hash, s.invite(cfg, si.Invite, func() error { return VerifyInvite(&si, cfg.Release) }, raw, at))
	case probe.Entry != nil:
		next, err := FromV1(probe.Entry)
		if err != nil {
			s.outcome(hash, fmt.Errorf("not an item: %w", err))
			break
		}
		s.entryItem(cfg, voters, next, hash, at)
	default:
		s.outcome(hash, errors.New("not an item"))
	}
	s.settle(cfg, voters)
}

func (s *State) judgeV2(cfg *Config, voters []string, from string, raw json.RawMessage, hash string, at uint64) {
	envp, err := ReadEnvelope(raw)
	if err != nil {
		s.outcome(hash, err)
		return
	}
	env := *envp
	body, err := env.BodyBytes()
	if err != nil {
		s.outcome(hash, err)
		return
	}
	switch env.Kind {
	case "entry":
		next, err := FromEnvelope(raw)
		if err != nil {
			s.outcome(hash, fmt.Errorf("not an item: %w", err))
			return
		}
		s.entryItem(cfg, voters, next, hash, at)
	case "leave":
		var l Leave
		if err := strict(body, &l); err != nil {
			s.outcome(hash, fmt.Errorf("not an item: %w", err))
			return
		}
		if err := sameAs(body, l.Bytes()); err != nil {
			s.outcome(hash, err)
			return
		}
		s.outcome(hash, s.record(l, func(e *Entry) error {
			if l.Name != e.Name {
				return Rejected("that statement is someone else's")
			}
			if l.Root != e.Root {
				return Rejected("not signed by the root on file")
			}
			return VerifyRoot(&env, e)
		}, raw, at))
	case "invite":
		var inv Invite
		if err := strict(body, &inv); err != nil {
			s.outcome(hash, fmt.Errorf("not an item: %w", err))
			return
		}
		if err := sameAs(body, inv.Bytes()); err != nil {
			s.outcome(hash, err)
			return
		}
		s.outcome(hash, s.invite(cfg, inv, func() error {
			if cfg.Release == "" {
				return errors.New("no release key here")
			}
			return VerifyEd25519(cfg.Release, Message("invite", body), env.Sig)
		}, raw, at))
	case "enlist", "unenlist":
		s.outcome(hash, s.enlist(env, body, raw, at))
	default:
		s.outcome(hash, fmt.Errorf("no such item: %q", env.Kind))
	}
}

// entryItem: an update lands at once; a claim waits for the voters
func (s *State) entryItem(cfg *Config, voters []string, next *Signed, hash string, at uint64) {
	if s.entry(next.Entry.Name, at) != nil {
		s.outcome(hash, s.commit(cfg, next, at))
		return
	}
	err := Accept(nil, next)
	if err != nil {
		err = fmt.Errorf("refused: %w", err)
	} else if err = s.checkGrant(cfg, nil, &next.Entry, at); err == nil {
		err = s.claimsFree(&next.Entry, at)
	}
	if err != nil {
		s.outcome(hash, err)
		return
	}
	for _, p := range s.Pending {
		if p.Hash == hash {
			return
		}
	}
	s.Pending = append(s.Pending, &Pending{Hash: hash, Item: next.Raw, At: at, s: next})
}

// settle is the gate's settle: claims every voter acknowledged become
// entries, the first in log order winning its name
func (s *State) settle(cfg *Config, voters []string) {
	for {
		ready := -1
		for i, p := range s.Pending {
			got := s.Acks[p.Hash]
			all := len(voters) > 0
			for _, v := range voters {
				if !contains(got, v) {
					all = false
					break
				}
			}
			if all {
				ready = i
				break
			}
		}
		if ready < 0 {
			return
		}
		p := s.Pending[ready]
		s.Pending = append(s.Pending[:ready:ready], s.Pending[ready+1:]...)
		name := p.s.Entry.Name
		var res error
		if s.entry(name, p.At) != nil {
			res = fmt.Errorf("%s is already taken", name)
		} else {
			res = s.commit(cfg, p.s, p.At)
		}
		if res == nil {
			var others []*Pending
			kept := s.Pending[:0:0]
			for _, q := range s.Pending {
				if q.s.Entry.Name == name {
					others = append(others, q)
				} else {
					kept = append(kept, q)
				}
			}
			s.Pending = kept
			sort.SliceStable(others, func(i, j int) bool { return others[i].s.Entry.Version < others[j].s.Entry.Version })
			for _, q := range others {
				if err := s.commit(cfg, q.s, q.At); err != nil {
					s.outcome(q.Hash, fmt.Errorf("%s went to an earlier claim", name))
				} else {
					s.outcome(q.Hash, nil)
				}
			}
		}
		s.outcome(p.Hash, res)
	}
}

// invite: signed by the release key, not expired when agreed, kept until
// it expires
func (s *State) invite(cfg *Config, inv Invite, verify func() error, raw json.RawMessage, at uint64) error {
	if cfg.Release == "" {
		return errors.New("no release key here")
	}
	if err := verify(); err != nil {
		return fmt.Errorf("refused: %w", err)
	}
	if inv.Expires <= at {
		return errors.New("that invite had expired")
	}
	if inv.Expires <= inv.Issued {
		return errors.New("that invite has no window")
	}
	s.Invites[inv.PublicKey] = raw
	return nil
}

// record is leaving.rs record: the newest statement stands
func (s *State) record(l Leave, verify func(*Entry) error, raw json.RawMessage, at uint64) error {
	h := s.entry(l.Name, at)
	if h == nil {
		return errors.New("no entry by that name")
	}
	if err := verify(&h.Signed.Entry); err != nil {
		return fmt.Errorf("refused: %w", err)
	}
	if l.At > at+leaveAhead {
		return errors.New("dated ahead of now")
	}
	if l.At+leaveBehind < at {
		return errors.New("too old to be taken")
	}
	if old := s.Said[l.Name]; old != nil {
		if old.Leave == l {
			return nil
		}
		if l.At <= old.Leave.At {
			return errors.New("older than the statement on record")
		}
	}
	s.Said[l.Name] = &Said{Leave: l, Item: raw, Agreed: at}
	return nil
}

// enlist / unenlist: a member's root names a box as theirs, or no longer.
// What makes an enlisted box count (its hardware) is the control plane's
// (docs/dev/joining.md); here, only that the member said it.
func (s *State) enlist(env Envelope, body []byte, raw json.RawMessage, at uint64) error {
	var e struct {
		Member  string `json:"member"`
		Box     string `json:"box"`
		Key     string `json:"key,omitempty"`
		Ticket  string `json:"ticket,omitempty"`
		Expires uint64 `json:"expires,omitempty"`
		At      uint64 `json:"at,omitempty"`
	}
	if err := strict(body, &e); err != nil {
		return fmt.Errorf("not an item: %w", err)
	}
	again, _ := json.Marshal(&e)
	if err := sameAs(body, again); err != nil {
		return err
	}
	var who *Held
	for _, n := range s.names() {
		if h := s.entry(n, at); h != nil && MemberID(h.Signed.Entry.Root) == e.Member {
			who = h
			break
		}
	}
	if who == nil {
		return errors.New("no member with that root")
	}
	if err := VerifyRoot(&env, &who.Signed.Entry); err != nil {
		return fmt.Errorf("refused: %w", err)
	}
	if env.Kind == "unenlist" {
		if cur := s.Enlisted[e.Box]; cur != nil && cur.Member == who.Signed.Entry.Name {
			delete(s.Enlisted, e.Box)
			return nil
		}
		return errors.New("that box is not theirs")
	}
	if e.Expires <= at {
		return errors.New("that enlistment has expired")
	}
	if cur := s.Enlisted[e.Box]; cur != nil && cur.Member != who.Signed.Entry.Name {
		return errors.New("that box is someone else's")
	}
	s.Enlisted[e.Box] = &Enlisted{Member: who.Signed.Entry.Name, Box: e.Box, Key: e.Key, Ticket: e.Ticket, Expires: e.Expires, Agreed: at, Item: raw}
	return nil
}

func contains(xs []string, x string) bool {
	for _, y := range xs {
		if y == x {
			return true
		}
	}
	return false
}
