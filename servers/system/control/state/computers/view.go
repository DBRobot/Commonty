package computers

// /computers, the same on every box (dd-control): every member's configs
// and remote computers as agreed, for the page, the placement and the
// follower on each box. A function of the log and the release alone.

import (
	"crypto/sha256"
	"encoding/json"
	"net/http"
	"sort"
	"sync"

	"commonty/control/state/ident"
)

// Replayer judges the log once, in order: every item to the identity judge,
// and the ones of these kinds to this one too, against identity as it
// stood at that item
type Replayer struct {
	mu   sync.Mutex
	cfg  *ident.Config
	lim  *Limits
	vote []string
	id   *ident.State
	s    *State
	next int
}

func NewReplayer(cfg *ident.Config, lim *Limits, voters []string) *Replayer {
	return &Replayer{cfg: cfg, lim: lim, vote: append([]string(nil), voters...), id: ident.New(), s: New()}
}

// CatchN judges items[r.next:n] and returns both states
func (r *Replayer) CatchN(n int, item func(int) ident.Item) (*ident.State, *State) {
	r.mu.Lock()
	defer r.mu.Unlock()
	if n < r.next {
		r.id, r.s, r.next = ident.New(), New(), 0
	}
	for i := r.next; i < n; i++ {
		it := item(i)
		at := uint64(0)
		if it.Time > 0 {
			at = uint64(it.Time / 1000)
		}
		r.id.Judge(r.cfg, r.vote, it.Box, it.Item, at)
		var probe struct {
			V    int    `json:"v"`
			Kind string `json:"kind"`
		}
		if json.Unmarshal(it.Item, &probe) == nil && probe.V == 2 && Kind(probe.Kind) {
			r.s.Judge(r.id, r.cfg, r.lim, it.Item, at)
		}
	}
	r.next = n
	return r.id, r.s
}

// Listed is one remote computer as readers see it
type Listed struct {
	Owner    string    `json:"owner"`
	ID       string    `json:"id"`
	Name     string    `json:"name"`
	Config   string    `json:"config"`
	Memory   uint64    `json:"memory"`
	Threads  uint64    `json:"threads"`
	Disk     uint64    `json:"disk"`
	Nix      string    `json:"nix"`
	Players  []string  `json:"players"`
	Keys     []string  `json:"keys"`
	Consent  string    `json:"consent"`
	Anywhere bool      `json:"anywhere"`
	Port     uint64    `json:"port"`
	Forwards []Forward `json:"forwards"`
	// what its owner asked: up or not
	Asked bool `json:"asked"`
	// whether it should run: asked up, and its owner still a member. A
	// member who leaves or is shut out has theirs stopped, kept until the
	// leaving window deletes them, not deleted here
	Up     bool            `json:"up"`
	Boxes  []string        `json:"boxes"` // for "own": where it may run
	Agreed uint64          `json:"agreed"`
	Item   json.RawMessage `json:"item"`
}

// View is the whole of what /computers serves
type View struct {
	Height    uint64                        `json:"height"`
	Time      int64                         `json:"time"`
	Changed   uint64                        `json:"changed"`
	Configs   map[string]map[string]*Config `json:"configs"`
	Computers map[string]*Listed            `json:"computers"`
	Outcomes  map[string]string             `json:"outcomes"`
}

func (s *State) View(id *ident.State, cfg *ident.Config, height uint64, timeMs int64) *View {
	at := uint64(0)
	if timeMs > 0 {
		at = uint64(timeMs / 1000)
	}
	v := &View{Height: height, Time: timeMs, Configs: map[string]map[string]*Config{}, Computers: map[string]*Listed{}, Outcomes: s.Outcomes}
	for o, cs := range s.Configs {
		for n, c := range cs {
			if c.Gone {
				continue
			}
			if v.Configs[o] == nil {
				v.Configs[o] = map[string]*Config{}
			}
			v.Configs[o][n] = c
		}
	}
	for _, k := range s.IDs() {
		h := s.Computers[k]
		member := false
		if e := id.Entries[h.Owner]; e != nil && e.Signed != nil {
			st, _, _ := id.Status(cfg, &e.Signed.Entry, at, nil)
			member = st == "member"
		}
		c := &Listed{
			Owner: h.Owner, ID: h.ID, Name: h.Name, Config: h.Config,
			Memory: h.Memory, Threads: h.Threads, Disk: h.Disk, Nix: h.Nix,
			Players: h.Players, Keys: h.Keys, Consent: h.Consent, Anywhere: h.Anywhere,
			Port: h.Port, Forwards: h.Forwards, Asked: h.Up, Up: h.Up && member,
			Agreed: h.Agreed, Item: h.Item,
		}
		if c.Forwards == nil {
			c.Forwards = []Forward{}
		}
		if c.Players == nil {
			c.Players = []string{}
		}
		if c.Keys == nil {
			c.Keys = []string{}
		}
		c.Boxes = []string{}
		if h.Consent == "own" {
			if bs := id.BoxesOf(h.Owner, at); bs != nil {
				c.Boxes = bs
			}
		}
		v.Computers[k] = c
	}
	return v
}

// Server answers /computers from a replayer over the agreed log
type Server struct {
	cfg     *ident.Config
	r       *Replayer
	mu      sync.Mutex
	last    [32]byte
	changed uint64
	// the view as last made, for as long as the log and the minute are the
	// same: every box's gate asks every second, and nothing changes between
	key  [2]uint64
	view *View
}

func NewServer(cfg *ident.Config, lim *Limits, voters []string) *Server {
	return &Server{cfg: cfg, r: NewReplayer(cfg, lim, voters)}
}

// current is the view at a log of n items, made again only when the log
// or the minute changes; the caller holds sv.mu
func (sv *Server) current(n int, item func(int) ident.Item, height uint64, timeMs int64) *View {
	// a status can change with the time alone (a suspension's window), so a
	// view is kept for a minute at most
	key := [2]uint64{uint64(n), uint64(timeMs / 60000)}
	if sv.view == nil || key != sv.key {
		id, s := sv.r.CatchN(n, item)
		v := s.View(id, sv.cfg, 0, timeMs)
		v.Height, v.Time = 0, 0
		b, _ := json.Marshal(v)
		if h := sha256.Sum256(b); h != sv.last || sv.changed == 0 {
			sv.last, sv.changed = h, height
		}
		sv.key, sv.view = key, v
	}
	return sv.view
}

// Computers is every computer as /computers lists it, for placement in the
// same process; the view's own, not to be changed
func (sv *Server) Computers(n int, item func(int) ident.Item, height uint64, timeMs int64) []*Listed {
	sv.mu.Lock()
	defer sv.mu.Unlock()
	v := sv.current(n, item, height, timeMs)
	out := make([]*Listed, 0, len(v.Computers))
	for _, c := range v.Computers {
		out = append(out, c)
	}
	sort.Slice(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out
}

// Answer: the log is n items; owner, if not empty, asks for one member's
// alone; since, as /identity's, answers 304 while nothing changed
func (sv *Server) Answer(n int, item func(int) ident.Item, height uint64, timeMs int64, owner string, owners bool, since uint64) (int, []byte) {
	// one at a time from the catching up to the answer: the states are the
	// replayer's own, and another request catching up would change them
	// under this one's reading
	sv.mu.Lock()
	defer sv.mu.Unlock()
	v := *sv.current(n, item, height, timeMs)
	v.Height, v.Time, v.Changed = height, timeMs, sv.changed
	if since != 0 && since >= sv.changed {
		return http.StatusNotModified, nil
	}
	if owners {
		m := map[string]string{}
		for k, c := range v.Computers {
			m[k] = c.Owner
		}
		b, _ := json.Marshal(map[string]any{"height": height, "time": timeMs, "changed": sv.changed, "owners": m, "outcomes": v.Outcomes})
		return http.StatusOK, b
	}
	if owner != "" {
		mine := map[string]*Listed{}
		for k, c := range v.Computers {
			if c.Owner == owner {
				mine[k] = c
			}
		}
		cs := v.Configs[owner]
		if cs == nil {
			cs = map[string]*Config{}
		}
		b, _ := json.Marshal(map[string]any{"height": height, "time": timeMs, "changed": sv.changed, "owner": owner, "configs": cs, "computers": mine})
		return http.StatusOK, b
	}
	b, _ := json.Marshal(&v)
	return http.StatusOK, b
}
