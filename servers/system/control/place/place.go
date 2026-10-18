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
	// role -> its services, to read what a heartbeat's roles run
	Roles   map[string][]string `json:"roles"`
	Damping Damping             `json:"damping"`
	// bucket -> how many stores keep it, and the service that writes it
	Buckets map[string]Bucket `json:"buckets"`
	// members' remote computers as agreed (state/computers, /computers):
	// not the release's, so dd-control sets them each round
	Computers []*computers.Listed `json:"-"`
}

// Bucket is one a service writes to its box's own store
// (nix/modules/storage/store.nix), copied to as many other stores as the
// plan's Copies name
type Bucket struct {
	Copies  int    `json:"copies"`
	Service string `json:"service"`
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

	// what runs now, by the roles each heartbeat names, and the placed
	// services each box says it runs
	now := map[string]map[string]bool{}
	runs := func(svc, name string) {
		if now[svc] == nil {
			now[svc] = map[string]bool{}
		}
		now[svc][name] = true
	}
	for _, name := range live {
		for _, r := range s.Boxes[name].Roles {
			for _, svc := range in.Roles[r] {
				if !in.Services[svc].Placed {
					runs(svc, name)
				}
			}
		}
		for svc, sv := range in.Services {
			if sv.Placed && s.Boxes[name].Facts["runs/"+svc] > 0 {
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
		if f["can_live"] == 0 {
			canOf[name] = rel.Can
			continue
		}
		barred := map[string]bool{}
		for _, svc := range rel.Barred {
			barred[svc] = true
		}
		c := map[string]float64{}
		for svc, sv := range in.Services {
			v, ok := f["can/"+svc]
			if _, rok := rel.Can[svc]; ok && !barred[svc] && (!sv.Lease || rok) {
				c[svc] = v
			}
		}
		canOf[name] = c
	}
	// a draining box (dd-drain) is given nothing new, and what may move
	// moves off it
	draining := func(name string) bool { return s.Boxes[name].Facts["draining"] > 0 }
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
	// the needs' estimate until one has
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
	for b, bk := range in.Buckets {
		least, any := 0.0, false
		for _, name := range live {
			v := facts(name, "store/"+b+"/bytes") / gib
			if writer(name, bk.Service) {
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
			if writer(w, in.Buckets[b].Service) && facts(w, "store/"+b+"/lost/"+name) > 0 {
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
	room := func(name, svc string) bool {
		// its buckets: a store here with room for what it does not keep
		// yet. Until some box has a store, nothing is judged by them
		if anyStore {
			d := 0.0
			for _, x := range adds(name, svc) {
				for _, b := range bucketsOf[x] {
					if !stored(name) {
						return false
					}
					d += needs(name, b)
				}
			}
			if d > 0 && d > spare(name) {
				return false
			}
		}
		if capacity(name) == 0 {
			return true // a box that has not said its memory: capability judged it
		}
		m := 0.0
		for _, x := range adds(name, svc) {
			m += memory[x]
		}
		return used[name]+m <= capacity(name)
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
		if c := capacity(name); c > 0 {
			m := 0.0
			for _, x := range adds(name, svc) {
				m += memory[x]
			}
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
				used[name] += memory[x]
				for _, b := range bucketsOf[x] {
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
				if !planned[name][svc] && can(name, svc) && !draining(name) && allowed(name, svc) && room(name, svc) {
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
		// the best of from, unless what runs now is within the margin of it
		choose := func(from []string, why0 string) bool {
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
				return false
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
			for n -= count[svc]; n > 0; n-- { // less what came beside what needs it
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
		// where it is written: its room was judged with the service
		for _, name := range live {
			if planned[name][bk.Service] && stored(name) && !lost(name, b) {
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
