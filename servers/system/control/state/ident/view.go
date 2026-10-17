package ident

// What readers get (docs/dev/identity-state.md, section 2): the agreed
// identity state, with each person's status worked out from the release's
// member list. A function of the agreed log and the release, so every box
// and the witness serve the same answer.

import (
	"encoding/json"
	"sort"
	"strings"
	"sync"
)

// Item is one directory item as the log keeps it
type Item struct {
	Time int64           `json:"time"` // agreed, ms
	Box  string          `json:"box"`
	Item json.RawMessage `json:"item"`
}

// Replayer keeps the judged state up to the log's end, judging what is new
type Replayer struct {
	mu   sync.Mutex
	cfg  *Config
	vote []string
	s    *State
	next int
}

func NewReplayer(cfg *Config, voters []string) *Replayer {
	return &Replayer{cfg: cfg, vote: append([]string(nil), voters...), s: New()}
}

// Catch judges items[r.next:] and returns the state; items is the whole
// log so far
func (r *Replayer) Catch(items []Item) *State {
	r.mu.Lock()
	defer r.mu.Unlock()
	if len(items) < r.next {
		// a shorter log than seen: start over, never keep a judgment of
		// items that are not there
		r.s, r.next = New(), 0
	}
	for _, it := range items[r.next:] {
		at := uint64(0)
		if it.Time > 0 {
			at = uint64(it.Time / 1000)
		}
		r.s.Judge(r.cfg, r.vote, it.Box, it.Item, at)
	}
	r.next = len(items)
	return r.s
}

// Member is one person as readers see them
type Member struct {
	ID        string          `json:"id"`
	IDs       []string        `json:"ids"`
	Root      string          `json:"root"`
	Kind      string          `json:"kind"` // ed25519 | passkey
	Recovery  string          `json:"recovery"`
	Devices   []Device        `json:"devices"`
	Passkeys  []Passkey       `json:"passkeys"`
	Libraries []Library       `json:"libraries"`
	Grant     *Grant          `json:"grant,omitempty"`
	Version   uint64          `json:"version"`
	Updated   uint64          `json:"updated"`
	Agreed    uint64          `json:"agreed"`
	Status    string          `json:"status"` // member | none | left | suspended | revoked
	Left      uint64          `json:"left,omitempty"`
	Suspended string          `json:"suspended,omitempty"`
	Boxes     []string        `json:"boxes"`
	Format    int             `json:"format"` // 1 | 2
	Item      json.RawMessage `json:"item"`
}

// View is the whole of what /identity serves
type View struct {
	Height    uint64                     `json:"height"`
	Time      int64                      `json:"time"`
	Members   map[string]*Member         `json:"members"`
	Handovers map[string][]Handover      `json:"handovers"`
	Invites   map[string]json.RawMessage `json:"invites"`
	Outcomes  map[string]string          `json:"outcomes"`
}

// SuspendWindow is leaving.rs WINDOW: how long a suspension keeps
// everything, from its removal date
const SuspendWindow = 90 * 24 * 3600

// Status is a person's standing by the release's list, their own word and
// their boxes: their own leaving first, then the list shutting them out,
// then the list or an invite or a box letting them in
func (s *State) Status(cfg *Config, e *Entry, at uint64, boxCounts func(box string) bool) (status string, left uint64, why string) {
	if said := s.Said[e.Name]; said != nil && !said.Leave.Back {
		return "left", said.Agreed, ""
	}
	ids := e.MemberIDs()
	// shut out by the list: nothing below lets them in again - not the list
	// naming another root of theirs, not an invite, not a box. Suspended
	// (the gate's suspension): a reason, a removal date, and within the
	// window from it; past the window, simply out
	for _, id := range ids {
		if contains(cfg.Members.Revoked, id) {
			for _, sid := range ids {
				why, ok := cfg.Members.Suspended[sid]
				removed, dated := cfg.Members.Removed[sid]
				if ok && dated && contains(cfg.Members.Revoked, sid) && at < removed+SuspendWindow {
					return "suspended", 0, why
				}
			}
			return "revoked", 0, ""
		}
	}
	for _, id := range ids {
		if contains(cfg.Members.Members, id) {
			return "member", 0, ""
		}
	}
	if e.Grant != nil && cfg.Release != "" && VerifyGrant(e, cfg.Release) == nil {
		return "member", 0, ""
	}
	for _, b := range s.boxesOf(e.Name, at) {
		if boxCounts == nil || boxCounts(b) {
			return "member", 0, ""
		}
	}
	return "none", 0, ""
}

func (s *State) boxesOf(name string, at uint64) []string {
	var out []string
	for b, en := range s.Enlisted {
		if en.Member == name && en.Expires > at {
			out = append(out, b)
		}
	}
	sort.Strings(out)
	return out
}

// View: the state as readers get it, at agreed time `at` (seconds).
// boxCounts says whether an enlisted box qualifies (its hardware,
// docs/dev/joining.md); nil counts every one.
func (s *State) View(cfg *Config, height uint64, timeMs int64, boxCounts func(string) bool) *View {
	at := uint64(0)
	if timeMs > 0 {
		at = uint64(timeMs / 1000)
	}
	v := &View{Height: height, Time: timeMs, Members: map[string]*Member{}, Handovers: s.Handovers, Invites: map[string]json.RawMessage{}, Outcomes: s.Outcomes}
	for _, n := range s.names() {
		h := s.entry(n, at)
		if h == nil {
			continue
		}
		e := &h.Signed.Entry
		kind := "ed25519"
		if strings.HasPrefix(e.Root, WebauthnRoot) {
			kind = "passkey"
		}
		st, left, why := s.Status(cfg, e, at, boxCounts)
		format := 1
		if h.Signed.V2() {
			format = 2
		}
		m := &Member{
			ID: MemberID(e.Root), IDs: e.MemberIDs(), Root: e.Root, Kind: kind, Recovery: e.Recovery,
			Devices: nz(e.Devices), Passkeys: nz(e.Passkeys), Libraries: nz(e.Libraries), Grant: e.Grant,
			Version: e.Version, Updated: e.Updated, Agreed: h.Agreed, Status: st, Left: left, Suspended: why,
			Boxes: nz(s.boxesOf(n, at)), Format: format, Item: h.Item,
		}
		v.Members[n] = m
	}
	for k, raw := range s.Invites {
		var probe struct {
			V      int    `json:"v"`
			Body   string `json:"body"`
			Invite struct {
				Invite Invite `json:"invite"`
			} `json:"invite"`
		}
		_ = json.Unmarshal(raw, &probe)
		exp := probe.Invite.Invite.Expires
		if probe.V == 2 {
			var inv Invite
			if b, err := b64.DecodeString(probe.Body); err == nil && json.Unmarshal(b, &inv) == nil {
				exp = inv.Expires
			}
		}
		if exp > at {
			v.Invites[k] = raw
		}
	}
	return v
}

func nz[T any](xs []T) []T {
	if xs == nil {
		return []T{}
	}
	return xs
}
