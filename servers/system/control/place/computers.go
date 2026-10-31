package place

import (
	"fmt"
	"slices"
	"sort"

	"commonty/control/state"
	"commonty/control/state/computers"
)

// ComputersService: the needs' name for hosting remote computers, which
// the release's table must give a box (and its owner not bar) before any
// computer goes there
const ComputersService = "computers"

// placeComputers puts each remote computer that is up on one box
// (docs/dev/remote-computers.md): sized against what the box says it has
// for computers (computers.prom), less what is placed there already; on
// the owner's boxes only for "own", and first on a joined box of theirs for
// any; and once on a box, kept there while that box is alive, whatever it now
// has room for (moves on failure)
func placeComputers(in Inputs, s *state.State, p *Plan, live []string, now map[string]map[string]bool, draining func(string) bool, perGiB float64, owners map[string]string) {
	type room struct{ mem, threads, disk float64 }
	// a joined box its enlisted owner keeps: it may hold only theirs (may)
	joinedHost := func(name string) bool {
		b := s.Boxes[name]
		if b == nil {
			return false
		}
		_, can := b.Facts["can/"+ComputersService]
		return in.Joined != nil && in.Joined.Attested && owners[name] != "" && b.Facts["can_live"] > 0 && can
	}
	// a computer is plaintext to its host: a box of the release whose table
	// gives it computers and whose owner does not bar them, or its owner's own
	// box by the agreed state; never a stranger nor a box on its own word
	host := func(name string) bool {
		rel, listed := in.Boxes[name]
		if !listed {
			return joinedHost(name)
		}
		_, can := rel.Can[ComputersService]
		return can && !slices.Contains(rel.Barred, ComputersService)
	}
	left := map[string]*room{}
	for _, name := range live {
		if !host(name) {
			continue
		}
		// its room is its own word until attestation: never past the
		// hardware its signed report gives the release, nor what its
		// heartbeat measures
		f, rel := s.Boxes[name].Facts, in.Boxes[name]
		// (a joined box has no report in the release: its word on its own
		// room only hurts its owner's own computers)
		mem, threads, disk := f["computer_max_mem_bytes"], f["computer_max_threads"], f["computer_max_disk_bytes"]
		if _, listed := in.Boxes[name]; listed {
			mem, threads, disk = min(mem, rel.MemoryGiB*gib), min(threads, rel.Threads), min(disk, rel.PoolGiB*gib)
		}
		if r := f["ram_bytes"]; r > 0 {
			mem = min(mem, r)
		}
		if c := f["cores"]; c > 0 {
			threads = min(threads, c)
		}
		if f["kvm"] > 0 && f["x86_64"] > 0 {
			left[name] = &room{mem, threads, disk}
		}
	}
	// what one adds to a box: its memory and threads, and the disk it may
	// still grow into there, less what it already keeps on that box
	used := func(name, id string) float64 { return s.Boxes[name].Facts["computer/"+id+"/used_bytes"] }
	need := func(name string, c *computers.Listed) room {
		return room{float64(c.Memory) * mib, float64(c.Threads), max(float64(c.Disk)*gib-used(name, c.ID), 0)}
	}
	fits := func(name string, c *computers.Listed) bool {
		r, n := left[name], need(name, c)
		return r != nil && r.mem > 0 && r.threads > 0 && r.disk > 0 && n.mem <= r.mem && n.threads <= r.threads && n.disk <= r.disk
	}
	take := func(name string, c *computers.Listed, f float64, why string) {
		if r := left[name]; r != nil {
			n := need(name, c)
			r.mem, r.threads, r.disk = r.mem-n.mem, r.threads-n.threads, r.disk-n.disk
		}
		p.Boxes[name] = append(p.Boxes[name], Decision{Service: ComputerService(c.ID), Fit: f, Why: why})
	}
	// what a move carries: what a box of the release that last ran it
	// said it kept, gone or not, less what the new box keeps of it. No
	// third box's word counts
	size := func(id string) float64 {
		m := 0.0
		for name, b := range s.Boxes {
			if host(name) && b.Facts["runs/"+ComputerService(id)] > 0 {
				m = max(m, b.Facts["computer/"+id+"/used_bytes"])
			}
		}
		return m
	}

	up := []*computers.Listed{}
	for _, c := range in.Computers {
		if c != nil && c.Up {
			up = append(up, c)
		}
	}
	sort.Slice(up, func(i, j int) bool { return up[i].ID < up[j].ID })
	// first those that stay on the live box that runs them: they hold
	// their room whatever comes after
	// whether c may be on this box at all: one the release lets hold
	// computers and, for "own", whose box it is by the release's word
	// (box.json); a joined box enlisted by c's own owner, whatever c says; or,
	// for a computer whose owner said it may run on anyone's, another member's
	mine := func(name string, c *computers.Listed) bool {
		_, listed := in.Boxes[name]
		return !listed && owners[name] == c.Owner
	}
	rank := func(name string, c *computers.Listed) int {
		_, listed := in.Boxes[name]
		switch {
		case mine(name, c):
			return 2
		case listed:
			return 1
		}
		return 0
	}
	may := func(name string, c *computers.Listed) bool {
		if _, listed := in.Boxes[name]; !listed {
			return left[name] != nil && (mine(name, c) || c.Consent == "any")
		}
		return left[name] != nil && (c.Consent == "any" || in.Boxes[name].Owner == c.Owner)
	}
	// a box's word that it runs one keeps it only where it may be
	held := func(c *computers.Listed) string {
		for _, name := range live {
			if now[ComputerService(c.ID)][name] && may(name, c) {
				return name
			}
		}
		return ""
	}
	rest := []*computers.Listed{}
	for _, c := range up {
		name := held(c)
		if name == "" {
			rest = append(rest, c)
			continue
		}
		why := "stays: it moves only when its box is gone"
		switch {
		case draining(name):
			why += "; the box is draining, it stays until the box leaves"
		case !fits(name, c):
			why += "; it no longer fits here"
			p.DoesNotFit[name] = append(p.DoesNotFit[name], ComputerService(c.ID))
		}
		take(name, c, 0, why)
	}
	for _, c := range rest {
		svc := ComputerService(c.ID)
		best, bestF, bestWhy := "", 0.0, ""
		for _, name := range live {
			if draining(name) || !may(name, c) || !fits(name, c) {
				continue
			}
			r, n := left[name], need(name, c)
			carry := max(size(c.ID)-min(used(name, c.ID), size(c.ID)), 0) / gib
			// memory to spare after it, less what moving it here carries
			f := 0.5*(r.mem-n.mem)/max(s.Boxes[name].Facts["computer_max_mem_bytes"], 1) - perGiB*carry
			// a box of their own first, then the release's as ever, then
			// another member's
			if best == "" || rank(name, c) > rank(best, c) || (rank(name, c) == rank(best, c) && f > bestF) {
				best, bestF = name, f
				bestWhy = fmt.Sprintf("fits: %.1f GiB, %d threads, %d GiB; carries %.1f GiB", n.mem/gib, c.Threads, c.Disk, carry)
			}
		}
		if best == "" {
			p.Unplaced[svc] = "no live box it may run on has room for it"
			if c.Consent != "any" {
				p.Unplaced[svc] = "none of its owner's live boxes has room for it"
			}
			continue
		}
		take(best, c, bestF, bestWhy)
	}
}
