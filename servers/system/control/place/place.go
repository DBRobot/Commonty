// Package place decides what each box runs, from what each box can run
// (the release's capability table, nix/lib/capability.nix), what the
// fleet wants of each service, and what the agreed state says now: who is
// alive, how fast each line measured, how much room each has, what each
// runs. A pure function: every box computes the same plan from the same
// state, and starts or stops only its own part.
//
// Greedy, the most constrained service first. A service spread over the
// fleet goes on every live box that can hold it; one per house goes on its
// count of boxes in each house; the rest on their count of boxes, best fit first. What
// runs stays where it is unless another box is better by a margin that grows
// with what moving it would carry.
package place

import (
	"fmt"
	"sort"

	"commonty/control/state"
	"commonty/control/state/computers"
)

// Inputs is what the release says (flake: placement-inputs).
type Inputs struct {
	Boxes    map[string]Box     `json:"boxes"`
	Services map[string]Service `json:"services"`
	Damping  Damping            `json:"damping"`
	// bucket -> how many stores keep it, and the service that writes it
	Buckets map[string]Bucket `json:"buckets"`
	// members' remote computers as agreed (state/computers, /computers):
	// not the release's, so dd-control sets them each round
	Computers []*computers.Listed `json:"-"`
	// what a box that joined on its own key may be given (JoinedRules);
	// absent, nothing but copies
	Joined *JoinedRules `json:"joined,omitempty"`
}

// JoinedRules: what the release lets a box that joined on its own key (a
// member's) run. Such a box is in no release's capability table, so what
// it says of itself can only narrow this, never widen it. Until its
// hardware and boot are attested in the agreed state (docs/dev/attestation.md)
// the release keeps Attested false and it runs nothing, holding copies
// only.
type JoinedRules struct {
	// the switch, on in the release that brings attestation
	Attested bool `json:"attested"`
	// what the policy's trust tiers allow on a member's box: never what
	// only the owner's boxes run (servesCode, trust owner), a
	// writer under a lease, or what reads a secret from the box's own file
	Services []string `json:"services"`
}

// Bucket is one a service writes to its box's own store
// (nix/modules/storage/store.nix), copied to as many other stores as the
// plan's Copies name
type Bucket struct {
	Copies  int    `json:"copies"`
	Service string `json:"service"`
	// the one box that writes it, wherever its service runs (each box's
	// backup); empty: wherever its service runs
	Box string `json:"box,omitempty"`
}

type Box struct {
	Site   string `json:"site"`
	Region string `json:"region"`
	// service -> how well this box suits it; a service not here, it cannot run.
	// What the release judged from the box's report; a heartbeat with its
	// own (can_live, can/<svc>) is used instead
	Can map[string]float64 `json:"can"`
	// what its owner's box.json or the policy keep off it whatever its
	// hardware (trust, a metered line): a box's own word never adds these
	Barred []string `json:"barred"`
	// whose box it is (its box.json owner): a computer whose owner keeps it
	// to their own boxes goes only where this is them
	Owner string `json:"owner"`
	// its hardware by its signed report: what its own word of its room for
	// computers is never taken past
	MemoryGiB float64 `json:"memoryGiB"`
	Threads   float64 `json:"threads"`
	PoolGiB   float64 `json:"poolGiB"`
	// what the release builds it with (nix/lib/fleet-placement.nix built):
	// a service not switched by the plan runs wherever it is built
	Built []string `json:"built"`
}

type Service struct {
	Enabled   bool    `json:"enabled"`
	EveryBox  bool    `json:"everyBox"`
	Spread    bool    `json:"spread"`
	PerHouse  int     `json:"perHouse"`  // this many in each house (a house's DNS); 0: not by house
	Instances int     `json:"instances"` // 0: one
	Class     string  `json:"class"`     // high | medium | low
	UpMbps    float64 `json:"upMbps"`
	DownMbps  float64 `json:"downMbps"`
	// the needs' estimate, until a box measures it
	MemoryGiB float64 `json:"memoryGiB"`
	// what it keeps whatever it measures (a model it may load)
	ReserveGiB float64 `json:"reserveGiB"`
	// what moving it carries: its data, at least its floor
	StorageGiB float64 `json:"storageGiB"`
	// what must share its box
	Local []string `json:"local"`
	// switched by the plan (nix/modules/box/placed.nix): it runs on a box
	// that carries it and that the plan gives it to, and nowhere else. A box
	// says it carries one (carries/<svc>) and runs it (runs/<svc>) in its
	// heartbeat.
	Placed bool `json:"placed"`
	// one writer at a time: a placed service that runs only while its box
	// holds the lease svc/<name> (cmd/dd-control/placed.go)
	Lease bool `json:"lease"`
	// on-failure: it stays on the box that runs it while that box is
	// alive, however much better another suits it; else it moves for a
	// box better by the damping margin
	Moves string `json:"moves"`
}

type Damping struct {
	Base   float64 `json:"base"`   // any move must win by this
	PerGiB float64 `json:"perGiB"` // and by this for each GiB it carries
}

// Plan is what each box runs, with why.
type Plan struct {
	Boxes map[string][]Decision `json:"boxes"`
	// a wanted service no live box can hold, with why; a bucket kept in
	// fewer stores than it asks, as copies/<bucket>
	Unplaced map[string]string `json:"unplaced"`
	// bucket -> the boxes whose stores keep it: where its service runs
	// first, then where it is already, then the boxes with the most room
	Copies map[string][]string `json:"copies"`
	// box -> what stays there (moves on-failure) though it no longer fits
	DoesNotFit map[string][]string `json:"doesNotFit"`
}

type Decision struct {
	Service string  `json:"service"`
	Fit     float64 `json:"fit"`
	Why     string  `json:"why"`
}

const (
	gib = 1 << 30
	mib = 1 << 20
)

// WhereItsDataIs starts the why of a writer kept on, or given back to,
// the box holding its lease (cmd/dd-control/placed.go reads it)
const WhereItsDataIs = "where its data is: "

// ComputerService is the placed service a remote computer is
func ComputerService(id string) string { return "computer-" + id }

// Decide plans the fleet. goneAfter is how long (ms of agreed time) a box
// may go unheard and still count as alive. members: the joined boxes a
// member has enlisted and that qualify (the agreed identity state's), which
// keep copies as a box of the release does.
func Decide(in Inputs, s *state.State, goneAfter int64, members map[string]bool) Plan {
	p := Plan{Boxes: map[string][]Decision{}, Unplaced: map[string]string{}, Copies: map[string][]string{}, DoesNotFit: map[string][]string{}}
	live := []string{}
	for name := range in.Boxes {
		if b := s.Boxes[name]; b != nil && s.Time-b.LastSeen <= goneAfter {
			live = append(live, name)
			p.Boxes[name] = []Decision{}
		}
	}
	// a box that joined on its own key (state/join.go) can run nothing - it
	// is in no release's capability table - and holds copies where its
	// store has room: more places a bucket is kept, never the only one, as
	// a bucket is always kept where its service runs
	for name := range s.Joined {
		if _, listed := in.Boxes[name]; listed {
			continue
		}
		if b := s.Boxes[name]; b != nil && s.Time-b.LastSeen <= goneAfter {
			live = append(live, name)
			p.Boxes[name] = []Decision{}
		}
	}
	sort.Strings(live)

	// what runs now: what the release builds each box with, of what the
	// plan does not switch, and the placed services each box says it runs
	now := map[string]map[string]bool{}
	runs := func(svc, name string) {
		if now[svc] == nil {
			now[svc] = map[string]bool{}
		}
		now[svc][name] = true
	}
	for _, name := range live {
		// a box that joined on its own key runs, as far as the plan
		// believes, only what the release may give it
		rel, listed := in.Boxes[name]
		for _, svc := range rel.Built {
			if listed && !in.Services[svc].Placed {
				runs(svc, name)
			}
		}
		mayRun := joinedCan(in, s.Boxes[name].Facts)
		for svc, sv := range in.Services {
			if _, ok := mayRun[svc]; (listed || ok) && sv.Placed && s.Boxes[name].Facts["runs/"+svc] > 0 {
				runs(svc, name)
			}
		}
		for _, c := range in.Computers {
			if svc := ComputerService(c.ID); s.Boxes[name].Facts["runs/"+svc] > 0 {
				runs(svc, name)
			}
		}
	}
	// what each box can run: what it says in its heartbeat, judged by the
	// release's rules on its own report as it is now (box/capability.nix),
	// or the release's table. Its word never adds what its owner's choices
	// bar, nor a writer whose lease the release does not let it hold (the
	// lease rules, and the keys sealed to those boxes, are the release's)
	canOf := map[string]map[string]float64{}
	for _, name := range live {
		f, rel := s.Boxes[name].Facts, in.Boxes[name]
		if _, listed := in.Boxes[name]; !listed {
			canOf[name] = joinedCan(in, f)
			continue
		}
		if f["can_live"] == 0 {
			canOf[name] = rel.Can
			continue
		}
		barred := map[string]bool{}
		for _, svc := range rel.Barred {
			barred[svc] = true
		}
		c := map[string]float64{}
		for svc := range in.Services {
			// its word only narrows the release's table: a service the
			// release does not give it stays off it, however it judges itself
			v, ok := f["can/"+svc]
			if _, rok := rel.Can[svc]; ok && rok && !barred[svc] {
				c[svc] = v
			}
		}
		canOf[name] = c
	}
	// a draining box (dd-drain) is given nothing new, and what may move
	// moves off it
	draining := func(name string) bool { return s.Boxes[name].Facts["draining"] > 0 }
	// a box switching to a new release (docs/dev/rollout.md) hands what it
	// runs to another first: like draining, but what fits nowhere else
	// stays and restarts in place, and a box already on that release is
	// the better home
	relDrain := func(name string) bool { return s.Boxes[name].Facts["release_drain"] > 0 }
	drainTo := 0.0
	for _, name := range live {
		drainTo = max(drainTo, s.Boxes[name].Facts["release_drain"])
	}
	// a placed service can run only on a box that carries it, and not
	// where the box itself refused to start it for want of room
	// (cmd/dd-control/placed.go)
	can := func(name, svc string) bool {
		if _, ok := canOf[name][svc]; !ok {
			return false
		}
		f := s.Boxes[name].Facts
		return !in.Services[svc].Placed || (f["carries/"+svc] > 0 && f["refused/"+svc] == 0)
	}

	// what a service holds: the most any live box measured it holding
	// (nix/modules/box/service-memory.nix), never under what it reserves;
	// the needs' estimate until one has. On a box that measures it itself,
	// what it holds there: a CI runner takes as many jobs as its box holds,
	// so a small box's runner is not sized by a big box's
	memory := map[string]float64{}
	for svc, sv := range in.Services {
		m, seen := 0.0, false
		for _, name := range live {
			if v, ok := s.Boxes[name].Facts["svc/"+svc+"/mem"]; ok {
				m, seen = max(m, v/gib), true
			}
		}
		if seen {
			memory[svc] = max(m, sv.ReserveGiB)
		} else {
			memory[svc] = sv.MemoryGiB
		}
	}
	memOn := func(name, svc string) float64 {
		if v, ok := s.Boxes[name].Facts["svc/"+svc+"/mem"]; ok {
			return max(v/gib, in.Services[svc].ReserveGiB)
		}
		return memory[svc]
	}

	planned := map[string]map[string]bool{} // box -> services
	used := map[string]float64{}            // box -> GiB of memory planned
	for _, name := range live {
		planned[name] = map[string]bool{}
	}
	facts := func(name, k string) float64 { return s.Boxes[name].Facts[k] }
	capacity := func(name string) float64 { return 0.85 * facts(name, "ram_bytes") / gib }

	// the stores: what each box's has free, and keeps of each bucket. Each
	// box says so of its own, and none is taken on its word past what it
	// can only hurt itself with: a bucket is as big as the box writing it
	// says (where its service runs now), or, with none, the least any
	// store keeps of it - a box claiming more cannot make a service fit
	// nowhere, and one claiming less only gets a copy its store refuses.
	stored := func(name string) bool { _, ok := s.Boxes[name].Facts["store_size_bytes"]; return ok }
	anyStore := false
	for _, name := range live {
		anyStore = anyStore || stored(name)
	}
	size := map[string]float64{}
	// who writes a bucket, as far as anyone else need believe it: for a
	// service with one writer, the holder of its lease the voters agreed
	// on; for another, a box that says it runs it and that the release
	// says can. A box's word on its own room or holdings only moves what
	// it is given; on a bucket's size, or on another box's copy, only the
	// writer's counts
	writer := func(name, svc string) bool {
		if in.Services[svc].Lease {
			l := s.Leases["svc/"+svc]
			return l != nil && l.Holder == name && l.Expires > s.Time
		}
		_, can := canOf[name][svc]
		return can && now[svc][name]
	}
	// whether a box writes a bucket: its own box alone, if it has one
	writes := func(name, b string) bool {
		if bk := in.Buckets[b]; bk.Box != "" {
			return name == bk.Box
		}
		return writer(name, in.Buckets[b].Service)
	}
	for b := range in.Buckets {
		least, any := 0.0, false
		for _, name := range live {
			v := facts(name, "store/"+b+"/bytes") / gib
			if writes(name, b) {
				size[b] = max(size[b], v)
			} else if v > 0 && (!any || v < least) {
				least, any = v, true
			}
		}
		if size[b] == 0 {
			size[b] = least
		}
	}
	// what a store keeps of a bucket, never past its size; and a store the
	// box writing it found without what it was sent (dd-store-copy reads a
	// copy back) keeps nothing
	lost := func(name, b string) bool {
		for _, w := range live {
			if writes(w, b) && facts(w, "store/"+b+"/lost/"+name) > 0 {
				return true
			}
		}
		return false
	}
	held := func(name, b string) float64 {
		if lost(name, b) {
			return 0
		}
		return min(facts(name, "store/"+b+"/bytes")/gib, size[b])
	}
	bucketsOf := map[string][]string{}
	for b, bk := range in.Buckets {
		bucketsOf[bk.Service] = append(bucketsOf[bk.Service], b)
	}
	for _, bs := range bucketsOf {
		sort.Strings(bs)
	}
	// a service's buckets on a box: a box's own bucket only on that box
	bucketsAt := func(name, svc string) []string {
		out := []string{}
		for _, b := range bucketsOf[svc] {
			if bx := in.Buckets[b].Box; bx == "" || bx == name {
				out = append(out, b)
			}
		}
		return out
	}
	disk := map[string]float64{} // box -> GiB of its store the plan fills
	// what a store has to give: free, less what the plan fills, less a
	// tenth of it kept back
	spare := func(name string) float64 {
		return facts(name, "store_free_bytes")/gib - disk[name] - 0.1*facts(name, "store_size_bytes")/gib
	}
	// what keeping b here adds to a store: what it does not keep yet
	needs := func(name, b string) float64 { return max(size[b]-held(name, b), 0) }

	// what placing svc on name adds: itself and what must share its box,
	// and what that must share its box with
	adds := func(name, svc string) []string {
		out, seen := []string{svc}, map[string]bool{svc: true}
		for i := 0; i < len(out); i++ {
			for _, d := range in.Services[out[i]].Local {
				if !seen[d] && !planned[name][d] {
					seen[d] = true
					out = append(out, d)
				}
			}
		}
		return out
	}
	// what a releasing box runs, by group (a service and what must share
	// its box with it), and whether some other box could take the whole
	// group; a group that fits nowhere stays, so a drain never leaves a
	// service with nowhere to run
	// whether some box other than from, not itself switching or draining,
	// can run all of group and has the memory and store room for it
	groupFits := func(group []string, from string) bool {
		for _, t := range live {
			if t == from || relDrain(t) || draining(t) {
				continue
			}
			ok, mem, d := true, 0.0, 0.0
			for _, x := range group {
				if !can(t, x) {
					ok = false
					break
				}
				for _, dep := range in.Services[x].Local {
					if dd := in.Services[dep]; dd.Enabled && !dd.EveryBox && !contains(group, dep) && !can(t, dep) {
						ok = false
					}
				}
				mem += memory[x]
				for _, b := range bucketsAt(t, x) {
					if anyStore && !stored(t) {
						ok = false
					}
					d += needs(t, b)
				}
			}
			if !ok {
				continue
			}
			if anyStore && d > 0 && d > spare(t) {
				continue
			}
			if c := capacity(t); c > 0 {
				for svc := range now {
					if now[svc][t] && !contains(group, svc) {
						mem += memory[svc]
					}
				}
				if mem > c {
					continue
				}
			}
			return true
		}
		return false
	}
	stays := map[string]map[string]bool{} // box -> services staying put
	movingOff := map[string]bool{}
	for _, d := range live {
		if !relDrain(d) {
			continue
		}
		stays[d] = map[string]bool{}
		mine := []string{}
		for svc := range now {
			if now[svc][d] && in.Services[svc].Placed && in.Services[svc].Enabled {
				mine = append(mine, svc)
			}
		}
		sort.Strings(mine)
		seen := map[string]bool{}
		for _, start := range mine {
			if seen[start] {
				continue
			}
			group := []string{start}
			seen[start] = true
			for i := 0; i < len(group); i++ {
				for _, x := range mine {
					if seen[x] {
						continue
					}
					if contains(in.Services[group[i]].Local, x) || contains(in.Services[x].Local, group[i]) {
						seen[x] = true
						group = append(group, x)
					}
				}
			}
			if !groupFits(group, d) {
				for _, x := range group {
					stays[d][x] = true
				}
			} else {
				for _, x := range group {
					movingOff[x] = true
				}
			}
		}
	}
	count := map[string]int{} // service -> boxes planned to run it
	// whether what svc brings with it may run one more time: a service
	// that needs one which is already at its count goes where that one is
	allowed := func(name, svc string) bool {
		for _, x := range adds(name, svc)[1:] {
			if c := in.Services[x].Instances; c > 0 && count[x] >= c {
				return false
			}
		}
		return true
	}
	// its buckets: a store here with room for what it does not keep yet.
	// Until some box has a store, nothing is judged by them
	storeRoom := func(name, svc string) bool {
		if !anyStore {
			return true
		}
		d := 0.0
		for _, x := range adds(name, svc) {
			for _, b := range bucketsAt(name, x) {
				if !stored(name) {
					return false
				}
				d += needs(name, b)
			}
		}
		return d == 0 || d <= spare(name)
	}
	// what placing svc here adds to the memory planned for the box
	memAdds := func(name, svc string) float64 {
		m := 0.0
		for _, x := range adds(name, svc) {
			m += memOn(name, x)
		}
		return m
	}
	room := func(name, svc string) bool {
		if !storeRoom(name, svc) {
			return false
		}
		if capacity(name) == 0 {
			return true // a box that has not said its memory: capability judged it
		}
		return used[name]+memAdds(name, svc) <= capacity(name)
	}
	// how well this box suits svc now: what it is for it, its line as
	// measured against what the service wants, what is already beside it,
	// and whether it is filling up
	fit := func(name, svc string) (float64, string) {
		sv, f := in.Services[svc], canOf[name][svc]
		why := fmt.Sprintf("suits %.2f", f)
		if sv.Class != "low" {
			k := 1.0
			if sv.Class == "medium" {
				k = 0.5
			}
			line := 0.0
			if up := facts(name, "net_up_mbps"); sv.UpMbps > 0 && up > 0 {
				line += 0.7 * min(up/sv.UpMbps, 1)
			} else if sv.UpMbps == 0 {
				line += 0.7
			}
			if down := facts(name, "net_down_mbps"); sv.DownMbps > 0 && down > 0 {
				line += 0.3 * min(down/sv.DownMbps, 1)
			} else if sv.DownMbps == 0 {
				line += 0.3
			}
			f += 2 * k * line
			why += fmt.Sprintf(", line %.0f/%.0f Mb/s", facts(name, "net_up_mbps"), facts(name, "net_down_mbps"))
		}
		near := 0
		for _, d := range sv.Local {
			if planned[name][d] {
				near++
			}
		}
		for x := range planned[name] {
			for _, d := range in.Services[x].Local {
				if d == svc {
					near++
				}
			}
		}
		f += 0.4 * float64(min(near, 3))
		// leaving a box that is switching: one already on that release
		// takes it, so it moves once
		if movingOff[svc] && drainTo > 0 && facts(name, "release") >= drainTo {
			f += 1
			why += ", on the new release"
		}
		if c := capacity(name); c > 0 {
			m := memAdds(name, svc)
			// room weighs only as a box fills: below 60% one box is as good
			// as another, and what suits a service best keeps it
			if fill := (used[name] + m) / c; fill > 0.6 {
				f -= 4 * (fill - 0.6)
				why += fmt.Sprintf(", %.0f%% full", 100*fill)
			}
		}
		return f, why
	}
	place := func(name, svc string, f float64, why string) {
		for _, x := range adds(name, svc) {
			if !planned[name][x] {
				planned[name][x] = true
				count[x]++
				used[name] += memOn(name, x)
				for _, b := range bucketsAt(name, x) {
					disk[name] += needs(name, b)
				}
				w := why
				if x != svc {
					w = "beside " + svc
				}
				p.Boxes[name] = append(p.Boxes[name], Decision{Service: x, Fit: f, Why: w})
			}
		}
	}
	capable := func(svc string) []string {
		out := []string{}
		for _, name := range live {
			if can(name, svc) {
				out = append(out, name)
			}
		}
		return out
	}

	// what every box runs comes first: it is why the box could join
	every := []string{}
	for svc, sv := range in.Services {
		if sv.Enabled && sv.EveryBox {
			every = append(every, svc)
		}
	}
	sort.Strings(every)
	for _, name := range live {
		for _, svc := range every {
			place(name, svc, 0, "every box")
		}
	}

	// what is wanted, the most constrained first
	wanted := []string{}
	for svc, sv := range in.Services {
		if sv.Enabled && !sv.EveryBox {
			wanted = append(wanted, svc)
		}
	}
	sort.Slice(wanted, func(i, j int) bool {
		a, b := len(capable(wanted[i])), len(capable(wanted[j]))
		if a != b {
			return a < b
		}
		return wanted[i] < wanted[j]
	})

	// what must share a box is decided before what needs it, so a service
	// goes where it suits best and what needs it follows
	done := map[string]bool{}
	var decide func(svc string)
	decide = func(svc string) {
		if done[svc] {
			return
		}
		done[svc] = true
		sv := in.Services[svc]
		for _, d := range sv.Local {
			if in.Services[d].Enabled && !in.Services[d].EveryBox {
				decide(d)
			}
		}
		cands := capable(svc)
		// what may stay: the boxes that run it now, able or not
		sticky := sv.Moves == "on-failure"
		if sticky {
			for _, name := range live {
				if now[svc][name] && !can(name, svc) {
					cands = append(cands, name)
				}
			}
			sort.Strings(cands)
		}
		if len(cands) == 0 {
			p.Unplaced[svc] = "no live box can run it"
			return
		}
		best := func(from []string) []string {
			out := []string{}
			for _, name := range from {
				if !planned[name][svc] && can(name, svc) && !draining(name) && (!relDrain(name) || stays[name][svc]) && allowed(name, svc) && room(name, svc) {
					out = append(out, name)
				}
			}
			sort.Slice(out, func(i, j int) bool {
				fi, _ := fit(out[i], svc)
				fj, _ := fit(out[j], svc)
				if fi != fj {
					return fi > fj
				}
				return out[i] < out[j]
			})
			return out
		}
		// no box in from has the memory by the plan's sum - what each
		// service measured at its most or keeps aside, added up, which a
		// box's free memory is often far from (node1, 2026-10-24: 54.5G
		// planned of 53G, with 45G of its 62G free). Rather than stop it,
		// the box it runs on, else the one nearest to holding it, takes it,
		// flagged; the box starts it only with that memory free
		// (cmd/dd-control/placed.go fits), and the plan gives a box that
		// refuses it nothing of it. No box checks its store, so that binds.
		squeeze := func(from []string, why0 string) bool {
			top, head := "", 0.0
			for _, name := range from {
				if planned[name][svc] || !can(name, svc) || draining(name) || (relDrain(name) && !stays[name][svc]) || !allowed(name, svc) || !storeRoom(name, svc) {
					continue
				}
				h := capacity(name) - used[name] - memAdds(name, svc)
				switch {
				case top == "", now[svc][name] && !now[svc][top], now[svc][name] == now[svc][top] && h > head:
					top, head = name, h
				}
			}
			if top == "" {
				return false
			}
			f, why := fit(top, svc)
			over := adds(top, svc)
			place(top, svc, f, fmt.Sprintf("%sno box has room for it by the plan's sum; here it is %.1f GiB over; %s", why0, -head, why))
			p.DoesNotFit[top] = append(p.DoesNotFit[top], over...)
			return true
		}
		// the best of from, unless what runs now is within the margin of it
		choose := func(from []string, why0 string) bool {
			// a releasing box keeps what no other box can hold: it
			// restarts there with the switch
			for _, name := range from {
				if stays[name][svc] && now[svc][name] && !planned[name][svc] {
					f, why := fit(name, svc)
					place(name, svc, f, why0+"restarts in place: no other box holds its group; "+why)
					return true
				}
			}
			// one that moves on failure stays on a live box that runs it;
			// fit and room are for choosing a new one
			if sticky {
				for _, name := range from {
					if now[svc][name] && !planned[name][svc] {
						f, why := fit(name, svc)
						why = why0 + "stays: it moves only when its box is gone; " + why
						if !room(name, svc) {
							why += "; it no longer fits here"
							p.DoesNotFit[name] = append(p.DoesNotFit[name], svc)
						}
						place(name, svc, f, why)
						return true
					}
				}
			}
			order := best(from)
			if len(order) == 0 {
				return squeeze(from, why0)
			}
			top := order[0]
			ft, why := fit(top, svc)
			var keep string
			for _, name := range order {
				if now[svc][name] {
					keep = name
					break
				}
			}
			if keep == "" || keep == top {
				place(top, svc, ft, why0+why)
				return true
			}
			fk, kwhy := fit(keep, svc)
			margin := in.Damping.Base + in.Damping.PerGiB*sv.StorageGiB
			if ft-fk < margin {
				place(keep, svc, fk, fmt.Sprintf("%sstays: %s is better by %.2f, under the margin %.2f; %s", why0, top, ft-fk, margin, kwhy))
			} else {
				place(top, svc, ft, fmt.Sprintf("%smoves from %s: better by %.2f, over the margin %.2f; %s", why0, keep, ft-fk, margin, why))
			}
			return true
		}
		switch {
		case sv.Spread:
			for _, name := range best(cands) {
				f, why := fit(name, svc)
				place(name, svc, f, "spread: "+why)
			}
		case sv.PerHouse > 0:
			houses := map[string][]string{}
			for _, name := range cands {
				h := in.Boxes[name].Site
				houses[h] = append(houses[h], name)
			}
			hs := []string{}
			for h := range houses {
				hs = append(hs, h)
			}
			sort.Strings(hs)
			for _, h := range hs {
				n := sv.PerHouse
				for _, name := range houses[h] {
					if planned[name][svc] {
						n--
					}
				}
				for ; n > 0; n-- {
					if !choose(houses[h], "its house: ") {
						p.Unplaced[svc+"@"+h] = "no box in the house with room"
						break
					}
				}
			}
		default:
			n := sv.Instances
			if n == 0 {
				n = 1
			}
			// a writer stays with the box holding its lease, or comes back
			// to it once its lease lapsed, while that box is alive and can
			// run it: its data is there, the newest epoch's, and a box
			// that only failed to renew (2026-10-24: node1's TPM stopped
			// signing; node2, with no base backup, took postgres) gets it
			// back. The agreed state says who holds it; no box's word on
			// what it keeps steers a writer. A holder that cannot start it
			// refuses it (cmd/dd-control/placed.go), and then, or with its
			// holder gone, any box may have it
			first := []string{}
			if l := s.Leases["svc/"+svc]; sv.Lease && l != nil && len(cands) > 1 {
				for _, name := range cands {
					if name == l.Holder && can(name, svc) {
						first = append(first, name)
					}
				}
			}
			for n -= count[svc]; n > 0; n-- { // less what came beside what needs it
				if len(first) > 0 && choose(first, WhereItsDataIs) {
					continue
				}
				if !choose(cands, "") {
					p.Unplaced[svc] = "no live box that can run it has room"
					break
				}
			}
		}
	}
	for _, svc := range wanted {
		decide(svc)
	}
	placeComputers(in, s, &p, live, now, draining, in.Damping.PerGiB)

	// the copies, once the services are placed: a bucket stays in the
	// stores that keep it, and goes to more only where it fits
	if !anyStore {
		return p
	}
	names := []string{}
	for b := range in.Buckets {
		names = append(names, b)
	}
	sort.Strings(names)
	for _, b := range names {
		bk := in.Buckets[b]
		n := max(bk.Copies, 1)
		keep := []string{}
		kept := map[string]bool{}
		add := func(name string) {
			if !kept[name] && len(keep) < n {
				kept[name] = true
				keep = append(keep, name)
			}
		}
		// where it is written: its room was judged with the service. A
		// box's own bucket only on that box, though every box runs its service
		for _, name := range live {
			at := planned[name][bk.Service]
			if bk.Box != "" {
				at = name == bk.Box
			}
			if at && stored(name) && !lost(name, b) {
				add(name)
			}
		}
		// where it is already, the most of it first; a draining box's
		// copy only when no other store has room for it
		have := []string{}
		for _, name := range live {
			if stored(name) && held(name, b) > 0 && !kept[name] && !draining(name) {
				have = append(have, name)
			}
		}
		// a holder able to take the service over first, then a box of
		// the release or a member's, then a stranger: a stranger's word
		// that it keeps a copy never pushes out either
		takesOver := func(name string) bool { _, ok := in.Boxes[name].Can[bk.Service]; return ok }
		listed := func(name string) bool { _, ok := in.Boxes[name]; return ok || members[name] }
		sort.SliceStable(have, func(i, j int) bool {
			if a, b := takesOver(have[i]), takesOver(have[j]); a != b {
				return a
			}
			if a, b := listed(have[i]), listed(have[j]); a != b {
				return a
			}
			return held(have[i], b) > held(have[j], b)
		})
		for _, name := range have {
			add(name)
		}
		// then the most room, and only where it fits
		more := []string{}
		for _, name := range live {
			if stored(name) && !kept[name] && !draining(name) {
				more = append(more, name)
			}
		}
		// a box able to take the service over first: a copy is what it
		// would start from; then any other, a stranger's too, by room
		sort.SliceStable(more, func(i, j int) bool {
			if a, b := takesOver(more[i]), takesOver(more[j]); a != b {
				return a
			}
			return spare(more[i]) > spare(more[j])
		})
		for _, name := range more {
			if len(keep) >= n {
				break
			}
			if lost(name, b) {
				continue
			}
			if d := needs(name, b); d <= spare(name) {
				disk[name] += d
				add(name)
			}
		}
		for _, name := range live {
			if draining(name) && stored(name) && held(name, b) > 0 && !lost(name, b) {
				add(name)
			}
		}
		sort.Strings(keep)
		p.Copies[b] = keep
		if len(keep) < n {
			p.Unplaced["copies/"+b] = fmt.Sprintf("kept in %d of %d stores: no other live box has room for its %.1f GiB", len(keep), n, size[b])
		}
	}
	return p
}

func contains(xs []string, x string) bool {
	for _, y := range xs {
		if y == x {
			return true
		}
	}
	return false
}

// Placed: of a plan, each placed service (needs placed or lease) and the
// boxes it is given, sorted. What the DNS Worker points a service's names
// at (servers/system/dns): the agreed placement, never a box's own word.
func Placed(in Inputs, p Plan) map[string][]string {
	out := map[string][]string{}
	for box, ds := range p.Boxes {
		for _, d := range ds {
			if sv, ok := in.Services[d.Service]; ok && sv.Placed {
				out[d.Service] = append(out[d.Service], box)
			}
		}
	}
	for _, bs := range out {
		sort.Strings(bs)
	}
	return out
}

// joinedCan: what a box that joined on its own key may be given, from the
// release's rules for such boxes narrowed by its own word (can/<svc>, a
// service it says it cannot run stays off it). Nothing until the release
// says joined boxes are attested.
func joinedCan(in Inputs, f map[string]float64) map[string]float64 {
	c := map[string]float64{}
	if in.Joined == nil || !in.Joined.Attested || f["can_live"] == 0 {
		return c
	}
	for _, svc := range in.Joined.Services {
		sv, ok := in.Services[svc]
		if !ok || sv.Lease {
			continue
		}
		if v, said := f["can/"+svc]; said {
			c[svc] = v
		}
	}
	return c
}
