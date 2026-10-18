package computers

// Members' remote computers, judged (docs/dev/computers.md): each config
// and computer a member signs, judged once, as it is agreed, beside the
// identity judge (state/ident) in the same pass over the directory log, and
// against the identity state at that same point. Deterministic like the
// rest: no clock but the agreed time, no I/O, every map walked in order.

import (
	"encoding/json"
	"errors"
	"fmt"
	"regexp"
	"sort"

	"commonty/control/state/ident"
)

// Limits are the policy's (fleet/policy.json vms): what one member's
// computers may take together, and the pool their ports come from
type Limits struct {
	Memory    uint64 `json:"memory"`  // MiB, every computer of theirs, running or not
	Threads   uint64 `json:"threads"` // likewise
	Disk      uint64 `json:"disk"`    // GiB, likewise
	PortBase  uint64 `json:"portBase"`
	PortCount uint64 `json:"portCount"`
}

const (
	MinMemory = 512 // MiB
	MinDisk   = 8   // GiB
	MaxNix    = 64 * 1024
	MaxPorts  = 8 // one computer's, besides SSH
	// what one member may hold, so nobody fills the state or the pool
	MaxConfigs     = 32  // saved, deleted ones counted
	MaxPlayers     = 32  // let in, per config or computer
	MaxMemberPorts = 16  // SSH and forwards, every computer of theirs
	MaxMade        = 256 // computers ever, deleted ones counted
	powerAhead     = 300
	powerAgo       = 3600
)

var nameRe = regexp.MustCompile(`^[a-z0-9][a-z0-9-]{0,31}$`)
var idRe = regexp.MustCompile(`^[0-9a-f]{16}$`)

// Config is a member's saved config, as they signed it
type Config struct {
	Owner   string   `json:"owner"`
	Name    string   `json:"name"`
	Memory  uint64   `json:"memory"`
	Threads uint64   `json:"threads"`
	Disk    uint64   `json:"disk"`
	Nix     string   `json:"nix"`
	Ports   []uint64 `json:"ports"`
	Players []string `json:"players"`
	Consent string   `json:"consent"` // own | any
	Version uint64   `json:"version"`
	Gone    bool     `json:"gone,omitempty"` // deleted
}

// Computer is one computer, as its owner signed it
type Computer struct {
	Owner   string   `json:"owner"`
	ID      string   `json:"id"`
	Name    string   `json:"name"`
	Config  string   `json:"config"`
	Memory  uint64   `json:"memory"`
	Threads uint64   `json:"threads"`
	Disk    uint64   `json:"disk"`
	Nix     string   `json:"nix"`
	Ports   []uint64 `json:"ports"`
	Players []string `json:"players"`
	Consent string   `json:"consent"`
	// their yes to the warning, for anyone's box
	Anywhere bool   `json:"anywhere"`
	Version  uint64 `json:"version"`
}

// Power starts or stops one; a device of the owner's may sign it
type Power struct {
	ID string `json:"id"`
	Up bool   `json:"up"`
	At uint64 `json:"at"`
}

// Delete ends one, its disks with it
type Delete struct {
	Owner string `json:"owner"`
	ID    string `json:"id"`
	At    uint64 `json:"at"`
}

// Forward is a guest port, reached at a port of the pool
type Forward struct {
	Guest uint64 `json:"guest"`
	Host  uint64 `json:"host"`
}

// Held is a computer as agreed: what was signed, and what the judge gave it
type Held struct {
	Computer
	Port     uint64          `json:"port"` // SSH
	Forwards []Forward       `json:"forwards"`
	Up       bool            `json:"up"`
	PowerAt  uint64          `json:"powerAt"`
	Agreed   uint64          `json:"agreed"`
	Item     json.RawMessage `json:"item"`
}

// State is the agreed state of every member's configs and computers
type State struct {
	Configs   map[string]map[string]*Config `json:"configs"` // owner -> name -> config
	Computers map[string]*Held              `json:"computers"`
	// ids deleted, for good: an old item for one, sent again, never brings
	// it back (ids are made fresh by the signer, never reused). Bounded:
	// a member makes at most MaxMade computers, deleted ones counted
	Deleted  map[string]string `json:"deleted"` // id -> owner
	Outcomes map[string]string `json:"outcomes"`
	Order    []string          `json:"outcomeOrder"`
}

func New() *State {
	return &State{Configs: map[string]map[string]*Config{}, Computers: map[string]*Held{}, Deleted: map[string]string{}, Outcomes: map[string]string{}}
}

const maxOutcomes = 1024

func (s *State) outcome(hash string, err error) {
	if _, seen := s.Outcomes[hash]; !seen {
		s.Order = append(s.Order, hash)
	}
	if err == nil {
		s.Outcomes[hash] = "ok"
	} else {
		s.Outcomes[hash] = "refused: " + err.Error()
	}
	for len(s.Order) > maxOutcomes {
		delete(s.Outcomes, s.Order[0])
		s.Order = s.Order[1:]
	}
}

// Kinds this judges
func Kind(kind string) bool {
	switch kind {
	case "computer-config", "computer", "computer-power", "computer-delete":
		return true
	}
	return false
}

// who signed: the owner's entry, and whether they are a member now
func (s *State) owner(id *ident.State, cfg *ident.Config, name string, at uint64) (*ident.Entry, error) {
	h := id.Entry(name, at)
	if h == nil || h.Signed == nil {
		return nil, errors.New("no such member")
	}
	e := &h.Signed.Entry
	if st, _, _ := id.Status(cfg, e, at, nil); st != "member" {
		return nil, fmt.Errorf("%s is not a member (%s)", name, st)
	}
	return e, nil
}

// signed by one of the owner's current devices, or their root
func byDevice(env *ident.Envelope, body []byte, e *ident.Entry) error {
	if ident.VerifyRoot(env, e) == nil {
		return nil
	}
	m := ident.Message(env.Kind, body)
	for _, d := range e.Devices {
		if ident.VerifyEd25519(d.PublicKey, m, env.Sig) == nil {
			return nil
		}
	}
	return errors.New("not signed by them or a device of theirs")
}

// read: strictly, and nothing in the body but what the kind has
func read(body []byte, v any) error {
	if err := ident.Strict(body, v); err != nil {
		return fmt.Errorf("not an item: %w", err)
	}
	again, _ := json.Marshal(v)
	return ident.SameAs(body, again)
}

// the size and contents both kinds share
func (s *State) shape(id *ident.State, cfg *ident.Config, lim *Limits, owner string, memory, threads, disk uint64, nix string, ports []uint64, players []string, consent string, at uint64) error {
	if memory < MinMemory || memory > lim.Memory {
		return fmt.Errorf("memory is %d to %d MiB", MinMemory, lim.Memory)
	}
	if threads < 1 || threads > lim.Threads {
		return fmt.Errorf("threads are 1 to %d", lim.Threads)
	}
	if disk < MinDisk || disk > lim.Disk {
		return fmt.Errorf("a disk is %d to %d GiB", MinDisk, lim.Disk)
	}
	if len(nix) > MaxNix {
		return errors.New("the Nix file is over 64 KiB")
	}
	if len(ports) > MaxPorts {
		return fmt.Errorf("at most %d ports besides SSH", MaxPorts)
	}
	seen := map[uint64]bool{}
	for _, p := range ports {
		if p == 0 || p == 22 || p > 65535 || seen[p] {
			return errors.New("a port is 1 to 65535, once, and SSH is always there")
		}
		seen[p] = true
	}
	if len(players) > MaxPlayers {
		return fmt.Errorf("at most %d people let in", MaxPlayers)
	}
	let := map[string]bool{}
	for _, p := range players {
		if let[p] {
			return fmt.Errorf("%s twice", p)
		}
		let[p] = true
		if p == owner || !nameRe.MatchString(p) {
			return fmt.Errorf("not someone to let in: %s", p)
		}
		if _, err := s.owner(id, cfg, p, at); err != nil {
			return fmt.Errorf("not someone to let in: %w", err)
		}
	}
	if consent != "own" && consent != "any" {
		return errors.New("where it runs is own or any")
	}
	if consent == "own" && len(id.BoxesOf(owner, at)) == 0 {
		return errors.New("they have no box of their own: only anyone's box")
	}
	return nil
}

// Judge takes one item of a kind this judges, agreed at `at` (seconds),
// against the identity state at the same point in the log
func (s *State) Judge(id *ident.State, cfg *ident.Config, lim *Limits, raw json.RawMessage, at uint64) {
	hash := ident.ItemHash(raw)
	env, err := ident.ReadEnvelope(raw)
	if err != nil {
		s.outcome(hash, err)
		return
	}
	body, err := env.BodyBytes()
	if err != nil {
		s.outcome(hash, err)
		return
	}
	switch env.Kind {
	case "computer-config":
		s.outcome(hash, s.config(id, cfg, lim, env, body, at))
	case "computer":
		s.outcome(hash, s.computer(id, cfg, lim, env, body, raw, at))
	case "computer-power":
		s.outcome(hash, s.power(id, cfg, env, body, at))
	case "computer-delete":
		s.outcome(hash, s.del(id, cfg, env, body, at))
	}
}

func (s *State) config(id *ident.State, cfg *ident.Config, lim *Limits, env *ident.Envelope, body []byte, at uint64) error {
	var c Config
	if err := read(body, &c); err != nil {
		return err
	}
	e, err := s.owner(id, cfg, c.Owner, at)
	if err != nil {
		return err
	}
	if err := ident.VerifyRoot(env, e); err != nil {
		return fmt.Errorf("refused: %w", err)
	}
	if !nameRe.MatchString(c.Name) {
		return errors.New("a name is lowercase letters, digits and dashes, up to 32")
	}
	cur := s.Configs[c.Owner][c.Name]
	if cur != nil && c.Version <= cur.Version {
		return errors.New("older than the config on file")
	}
	if cur == nil && len(s.Configs[c.Owner]) >= MaxConfigs {
		return fmt.Errorf("at most %d configs, deleted ones counted", MaxConfigs)
	}
	if !c.Gone {
		if err := s.shape(id, cfg, lim, c.Owner, c.Memory, c.Threads, c.Disk, c.Nix, c.Ports, c.Players, c.Consent, at); err != nil {
			return err
		}
	}
	if s.Configs[c.Owner] == nil {
		s.Configs[c.Owner] = map[string]*Config{}
	}
	s.Configs[c.Owner][c.Name] = &c
	return nil
}

func (s *State) computer(id *ident.State, cfg *ident.Config, lim *Limits, env *ident.Envelope, body []byte, raw json.RawMessage, at uint64) error {
	var n Computer
	if err := read(body, &n); err != nil {
		return err
	}
	e, err := s.owner(id, cfg, n.Owner, at)
	if err != nil {
		return err
	}
	if err := ident.VerifyRoot(env, e); err != nil {
		return fmt.Errorf("refused: %w", err)
	}
	if !idRe.MatchString(n.ID) || !nameRe.MatchString(n.Name) || !nameRe.MatchString(n.Config) {
		return errors.New("an id is 16 hex; a name and a config's name lowercase letters, digits and dashes")
	}
	if _, gone := s.Deleted[n.ID]; gone {
		return errors.New("that remote computer was deleted")
	}
	cur := s.Computers[n.ID]
	if cur != nil {
		if cur.Owner != n.Owner {
			return errors.New("that id is someone else's")
		}
		if n.Version <= cur.Version {
			return errors.New("older than the computer on file")
		}
		if n.Disk < cur.Disk {
			return errors.New("a disk only grows")
		}
	}
	for _, other := range s.Computers {
		if other.Owner == n.Owner && other.Name == n.Name && other.ID != n.ID {
			return fmt.Errorf("they have a remote computer called %s", n.Name)
		}
	}
	if err := s.shape(id, cfg, lim, n.Owner, n.Memory, n.Threads, n.Disk, n.Nix, n.Ports, n.Players, n.Consent, at); err != nil {
		return err
	}
	if n.Consent == "any" && !n.Anywhere {
		return errors.New("running on someone else's box needs their yes first")
	}
	if n.Consent == "own" && n.Anywhere {
		return errors.New("a yes to anyone's box, for one that stays on theirs")
	}
	// every computer of theirs, running or not, this one as it would be
	var mem, thr, disk, ports, made uint64
	for _, other := range s.Computers {
		if other.Owner == n.Owner && other.ID != n.ID {
			mem += other.Memory
			thr += other.Threads
			disk += other.Disk
			ports += 1 + uint64(len(other.Forwards))
		}
	}
	if mem+n.Memory > lim.Memory || thr+n.Threads > lim.Threads || disk+n.Disk > lim.Disk {
		return fmt.Errorf("their remote computers would take %d MiB, %d threads and %d GiB together; the limit is %d MiB, %d and %d GiB", mem+n.Memory, thr+n.Threads, disk+n.Disk, lim.Memory, lim.Threads, lim.Disk)
	}
	if ports+1+uint64(len(n.Ports)) > MaxMemberPorts {
		return fmt.Errorf("at most %d ports across their remote computers, SSH counted", MaxMemberPorts)
	}
	if cur == nil {
		for _, o := range s.Computers {
			if o.Owner == n.Owner {
				made++
			}
		}
		for _, o := range s.Deleted {
			if o == n.Owner {
				made++
			}
		}
		if made >= MaxMade {
			return fmt.Errorf("at most %d remote computers made, deleted ones counted", MaxMade)
		}
	}
	// ports: its SSH and the forwards it keeps stay; new ones are the
	// lowest free in the pool, in agreed order, so every reader agrees
	taken := map[uint64]bool{}
	for _, other := range s.Computers {
		taken[other.Port] = true
		for _, f := range other.Forwards {
			taken[f.Host] = true
		}
	}
	free := func() (uint64, error) {
		for p := lim.PortBase; p < lim.PortBase+lim.PortCount; p++ {
			if !taken[p] {
				taken[p] = true
				return p, nil
			}
		}
		return 0, errors.New("no ports left in the pool")
	}
	h := &Held{Computer: n, Up: true, Agreed: at, Item: raw}
	if cur != nil {
		h.Port, h.Up, h.PowerAt = cur.Port, cur.Up, cur.PowerAt
	} else if h.Port, err = free(); err != nil {
		return err
	}
	for _, g := range n.Ports {
		host := uint64(0)
		if cur != nil {
			for _, f := range cur.Forwards {
				if f.Guest == g {
					host = f.Host
				}
			}
		}
		if host == 0 {
			if host, err = free(); err != nil {
				return err
			}
		}
		h.Forwards = append(h.Forwards, Forward{Guest: g, Host: host})
	}
	s.Computers[n.ID] = h
	return nil
}

func (s *State) power(id *ident.State, cfg *ident.Config, env *ident.Envelope, body []byte, at uint64) error {
	var p Power
	if err := read(body, &p); err != nil {
		return err
	}
	h := s.Computers[p.ID]
	if h == nil {
		return errors.New("no such remote computer")
	}
	e, err := s.owner(id, cfg, h.Owner, at)
	if err != nil {
		return err
	}
	if err := byDevice(env, body, e); err != nil {
		return fmt.Errorf("refused: %w", err)
	}
	if p.At > at+powerAhead || p.At+powerAgo < at {
		return errors.New("not dated now")
	}
	if p.At <= h.PowerAt {
		return errors.New("older than the last start or stop")
	}
	h.Up, h.PowerAt = p.Up, p.At
	return nil
}

func (s *State) del(id *ident.State, cfg *ident.Config, env *ident.Envelope, body []byte, at uint64) error {
	var d Delete
	if err := read(body, &d); err != nil {
		return err
	}
	h := s.Computers[d.ID]
	if h == nil || h.Owner != d.Owner {
		return errors.New("no such remote computer")
	}
	// a member who left still deletes their own
	hd := id.Entries[d.Owner]
	if hd == nil || hd.Signed == nil {
		return errors.New("no such member")
	}
	if err := ident.VerifyRoot(env, &hd.Signed.Entry); err != nil {
		return fmt.Errorf("refused: %w", err)
	}
	if d.At > at+powerAhead || d.At+powerAgo < at {
		return errors.New("not dated now")
	}
	delete(s.Computers, d.ID)
	s.Deleted[d.ID] = d.Owner
	return nil
}

// IDs in order, for whoever walks them
func (s *State) IDs() []string {
	out := make([]string, 0, len(s.Computers))
	for k := range s.Computers {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
