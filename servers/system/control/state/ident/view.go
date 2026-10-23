package ident

// What readers get (docs/dev/identity-state.md, section 2): the agreed
// identity state, with each person's status worked out from the release's
// member list. A function of the agreed log and the release, so every box
// and the witness serve the same answer.

import (
	"encoding/json"
	"strings"
	"sync"
)

// Item is one directory item as the log keeps it
type Item struct {
	Time int64           `json:"time"` // agreed, ms
	Box  string          `json:"box"`
	Item json.RawMessage `json:"item"`
	// the joined boxes as the agreed state had them when the item was
	// ordered, for a judge whose rules count a member's boxes (a remote
	// computer's owner): state.DirItem's Boxes
	Boxes map[string]BoxInfo `json:"boxes,omitempty"`
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
	return r.CatchN(len(items), func(i int) Item { return items[i] })
}

// CatchN is Catch over a log of n items read one at a time, so a caller
// holding its own kind of item copies only the new ones
func (r *Replayer) CatchN(n int, item func(int) Item) *State {
	r.mu.Lock()
	defer r.mu.Unlock()
	if n < r.next {
		// a shorter log than seen: start over, never keep a judgment of
		// items that are not there
		r.s, r.next = New(), 0
	}
	for i := r.next; i < n; i++ {
		it := item(i)
		at := uint64(0)
		if it.Time > 0 {
			at = uint64(it.Time / 1000)
		}
		r.s.Judge(r.cfg, r.vote, it.Box, it.Item, at)
	}
	r.next = n
	return r.s
}

// Member is one person as readers see them
type Member struct {
	ID        string    `json:"id"`
	IDs       []string  `json:"ids"`
	Root      string    `json:"root"`
	Kind      string    `json:"kind"` // ed25519 | passkey
	Recovery  string    `json:"recovery"`
	Devices   []Device  `json:"devices"`
	Passkeys  []Passkey `json:"passkeys"`
	Libraries []Library `json:"libraries"`
	Grant     *Grant    `json:"grant,omitempty"`
	Version   uint64    `json:"version"`
	Updated   uint64    `json:"updated"` // the entry's own, seconds
	Agreed    uint64    `json:"agreed"`  // ms, as every time the judge adds
	Status    string    `json:"status"`  // member | none | left | suspended | revoked
	Left      uint64    `json:"left,omitempty"`
	Suspended string    `json:"suspended,omitempty"`
	// the release's removal date for any of their ids (fleet/members.json
	// removed), what a reader's forgetting counts from
	Removed uint64          `json:"removed,omitempty"`
	Boxes   []string        `json:"boxes"`
	Format  int             `json:"format"` // 1 | 2
	Item    json.RawMessage `json:"item"`
	// what they may use: "invite" (the release's list, or an invite it
	// signed), "box" (a box of theirs that counts), or "none"
	Access   string    `json:"access"`
	Enlisted []BoxView `json:"enlisted"`
	// the admin's extension, while it runs (ms)
	GraceUntil uint64 `json:"grace_until,omitempty"`
	// every sign-in of theirs begun before this (ms) is over, on every box,
	// but the one session id in SignedOutKeep (signout.go)
	SignedOut     uint64 `json:"signed_out,omitempty"`
	SignedOutKeep string `json:"signed_out_keep,omitempty"`
}

// Owner is whose an enlisted box is, and the ticket it was enlisted with
type Owner struct {
	Ticket string `json:"ticket"`
	Owner  string `json:"owner"`
	Since  uint64 `json:"since"` // agreed, ms
}

// View is the whole of what /identity serves
type View struct {
	Height uint64 `json:"height"`
	Time   int64  `json:"time"`
	// the height this answer last changed at: what ?since= is held to
	Changed   uint64                `json:"changed"`
	Members   map[string]*Member    `json:"members"`
	Handovers map[string][]Handover `json:"handovers"`
	Invites   map[string]*Open      `json:"invites"`
	Outcomes  map[string]string     `json:"outcomes"`
	// box -> whose it is: every enlistment that stands
	Enlisted map[string]*Owner `json:"enlisted"`
	// box -> whose ticket it joined with, while no one has enlisted it and
	// its ticket is good: theirs to enlist
	Waiting map[string]*Owner `json:"waiting"`
}

// Open is an invite not yet expired
type Open struct {
	Issued  uint64          `json:"issued"`
	Expires uint64          `json:"expires"`
	Item    json.RawMessage `json:"item"`
}

// SuspendWindow is leaving.rs WINDOW: how long a suspension keeps
// everything, from its removal date
const SuspendWindow = 90 * 24 * 3600

// standing: a person by the release's list and their own word - their own
// leaving first, then the list shutting them out, then the list or an
// invite letting them in ("invite"); "" for none of these
func (s *State) standing(cfg *Config, e *Entry, at uint64) (status string, left uint64, why string) {
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
			return "invite", 0, ""
		}
	}
	if e.Grant != nil && cfg.Release != "" && VerifyGrant(e, cfg.Release) == nil {
		return "invite", 0, ""
	}
	return "", 0, ""
}

// Status is a person's standing as readers see it: left, suspended or
// revoked; member while they have access (an invite or a box that
// counts); none otherwise (signed up, not let in)
func (s *State) Status(cfg *Config, e *Entry, at uint64, boxes Boxes) (status string, left uint64, why string) {
	st, left, why := s.standing(cfg, e, at)
	switch st {
	case "left", "suspended", "revoked":
		return st, left, why
	}
	if acc, _, _ := s.Access(cfg, e, at, boxes); acc != "none" {
		return "member", 0, ""
	}
	return "none", 0, ""
}

// BoxesOf: the boxes a member's enlistments stand for at `at`, for the
// other judges beside this one (state/computers: "my boxes only")
func (s *State) BoxesOf(name string, at uint64) []string { return s.boxesOf(name, at) }

// View: the state as readers get it, at agreed time `at` (seconds). boxes
// is what the agreed state says of each box now (its facts, when it was
// last heard); nil knows none, so no box counts.
func (s *State) View(cfg *Config, height uint64, timeMs int64, boxes Boxes) *View {
	at := uint64(0)
	if timeMs > 0 {
		at = uint64(timeMs / 1000)
	}
	v := &View{Height: height, Time: timeMs, Members: map[string]*Member{}, Handovers: s.Handovers, Invites: map[string]*Open{}, Outcomes: s.Outcomes, Enlisted: map[string]*Owner{}, Waiting: s.waiting(cfg, at)}
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
		st, left, why := s.Status(cfg, e, at, boxes)
		access, enlisted, grace := s.Access(cfg, e, at, boxes)
		format := 1
		if h.Signed.V2() {
			format = 2
		}
		m := &Member{
			ID: MemberID(e.Root), IDs: e.MemberIDs(), Root: e.Root, Kind: kind, Recovery: e.Recovery,
			Devices: nz(e.Devices), Passkeys: nz(e.Passkeys), Libraries: nz(e.Libraries), Grant: e.Grant,
			Version: e.Version, Updated: e.Updated, Agreed: h.Agreed * 1000, Status: st, Left: left * 1000, Suspended: why,
			Removed: removedOf(cfg, e) * 1000, Boxes: nz(s.boxesOf(n, at)), Format: format, Item: h.Item,
			Access: access, Enlisted: nz(enlisted), GraceUntil: grace * 1000,
		}
		m.SignedOut, m.SignedOutKeep = s.signedOut(n, e)
		v.Members[n] = m
		for _, b := range m.Boxes {
			en := s.Enlisted[b]
			v.Enlisted[b] = &Owner{Ticket: en.Ticket, Owner: n, Since: en.Since * 1000}
		}
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
		inv := probe.Invite.Invite
		if probe.V == 2 {
			inv = Invite{}
			if b, err := b64.DecodeString(probe.Body); err == nil {
				_ = json.Unmarshal(b, &inv)
			}
		}
		if inv.Expires > at {
			v.Invites[k] = &Open{Issued: inv.Issued, Expires: inv.Expires, Item: raw}
		}
	}
	return v
}

// waiting: the joined boxes whose ticket a device of its member's signed,
// for them, good now, and used by no enlistment yet; Since is when the box
// joined
func (s *State) waiting(cfg *Config, at uint64) map[string]*Owner {
	out := map[string]*Owner{}
	for b, j := range s.Joins {
		if _, used := s.Tickets[j.Ticket]; used || j.Until <= at {
			continue
		}
		if en := s.Enlisted[b]; en != nil && s.owns(en, at) {
			continue
		}
		h := s.entry(j.User, at)
		if h == nil || s.banned(cfg, &h.Signed.Entry, at) != "" {
			continue
		}
		var keys []string
		for _, d := range h.Signed.Entry.Devices {
			keys = append(keys, d.PublicKey)
		}
		if t, err := ReadClaim(j.Block, keys); err != nil || t.User != j.User {
			continue
		}
		out[b] = &Owner{Ticket: j.Ticket, Owner: j.User, Since: j.At * 1000}
	}
	return out
}

// removedOf: the release's removal date for any id of theirs, newest
func removedOf(cfg *Config, e *Entry) uint64 {
	var t uint64
	for _, id := range e.MemberIDs() {
		t = max(t, cfg.Members.Removed[id])
	}
	return t
}

func nz[T any](xs []T) []T {
	if xs == nil {
		return []T{}
	}
	return xs
}
