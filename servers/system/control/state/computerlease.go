package state

import (
	"bytes"
	"fmt"
	"regexp"
	"sync"

	"commonty/control/state/computers"
	"commonty/control/state/ident"
)

// A remote computer's lease, svc/computer-<id> (docs/dev/remote-computers.md),
// has no rule in the release: the computers are the members', agreed in the
// log. A box may hold one only while the computer is agreed and up, and
// only a box placement may give it to; a renewal is refused once that stops
// being so, and the holder's fence stops it.
var computerLease = regexp.MustCompile(`^svc/computer-([0-9a-f]{16})$`)

// one replay of the log per config, every validator the same: it carries
// on from where it was while the log it was given is the start of this one
type replay struct {
	mu   sync.Mutex
	r    *computers.Replayer
	n    int
	last DirItem
}

var (
	replaysMu sync.Mutex
	replays   = map[*Config]*replay{}
)

func (c *Config) replay() *replay {
	replaysMu.Lock()
	defer replaysMu.Unlock()
	r := replays[c]
	if r == nil {
		r = &replay{}
		replays[c] = r
	}
	return r
}

func sameItem(a, b DirItem) bool {
	return a.Height == b.Height && a.Time == b.Time && a.Box == b.Box && bytes.Equal(a.Item, b.Item)
}

// computerAt: the computer as the log s holds says it is at now
func (s *State) computerAt(cfg *Config, id string, now int64) (*computers.Held, *ident.State, error) {
	if cfg.Identity == nil || cfg.Computers == nil {
		return nil, nil, fmt.Errorf("no remote computers in this release")
	}
	rp := cfg.replay()
	rp.mu.Lock()
	defer rp.mu.Unlock()
	n := len(s.Directory)
	if rp.r == nil || n < rp.n || (rp.n > 0 && !sameItem(s.Directory[rp.n-1], rp.last)) {
		rp.r = computers.NewReplayer(cfg.Identity, cfg.Computers, cfg.BoxVoters())
	}
	ids, cs := rp.r.CatchN(n, func(i int) ident.Item {
		d := s.Directory[i]
		return ident.Item{Time: d.Time, Box: d.Box, Item: d.Item}
	})
	rp.n = n
	if n > 0 {
		rp.last = s.Directory[n-1]
	}
	h := cs.Computers[id]
	if h == nil {
		return nil, nil, fmt.Errorf("no remote computer %s", id)
	}
	return h, ids, nil
}

// checkComputerLease: whether box may hold (acquire) or keep (renew) the
// computer's lease at now
func (s *State) checkComputerLease(cfg *Config, box, id string, now int64) error {
	h, ids, err := s.computerAt(cfg, id, now)
	if err != nil {
		return err
	}
	at := uint64(max(now, 0) / 1000)
	e := ids.Entries[h.Owner]
	member := false
	if e != nil && e.Signed != nil {
		st, _, _ := ids.Status(cfg.Identity, &e.Signed.Entry, at, nil)
		member = st == "member"
	}
	if !h.Up || !member {
		return fmt.Errorf("remote computer %s is not up", id)
	}
	// the boxes placement may give it (place/computers.go), never more: the
	// release's table gives the box computers and its owner does not bar
	// them, it says it has KVM on x86_64, and for "own" it is the owner's by
	// the release (box.json) - an enlistment is the member's word alone
	owner, host := cfg.Rules.ComputerHosts[box]
	if !host {
		return fmt.Errorf("%s may not host remote computers", box)
	}
	if b := s.Boxes[box]; b == nil || b.Facts["kvm"] <= 0 || b.Facts["x86_64"] <= 0 {
		return fmt.Errorf("%s has not said it has KVM on x86_64", box)
	}
	if h.Consent != "any" && (owner == "" || owner != h.Owner) {
		return fmt.Errorf("%s is not %s's box", box, h.Owner)
	}
	return nil
}
