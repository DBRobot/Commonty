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
)

// Inputs is what the release says (flake: placement-inputs).
type Inputs struct {
	Boxes    map[string]Box     `json:"boxes"`
	Services map[string]Service `json:"services"`
	// role -> its services, to read what a heartbeat's roles run
	Roles   map[string][]string `json:"roles"`
	Damping Damping             `json:"damping"`
}

type Box struct {
	Site   string `json:"site"`
	Region string `json:"region"`
	// service -> how well this box suits it; a service not here, it cannot run
	Can map[string]float64 `json:"can"`
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
}

type Damping struct {
	Base   float64 `json:"base"`   // any move must win by this
	PerGiB float64 `json:"perGiB"` // and by this for each GiB it carries
}

// Plan is what each box runs, with why.
type Plan struct {
	Boxes map[string][]Decision `json:"boxes"`
	// a wanted service no live box can hold, with why
	Unplaced map[string]string `json:"unplaced"`
}

type Decision struct {
	Service string  `json:"service"`
	Fit     float64 `json:"fit"`
	Why     string  `json:"why"`
}

const gib = 1 << 30

// Decide plans the fleet. goneAfter is how long (ms of agreed time) a box
// may go unheard and still count as alive.
func Decide(in Inputs, s *state.State, goneAfter int64) Plan {
	p := Plan{Boxes: map[string][]Decision{}, Unplaced: map[string]string{}}
	live := []string{}
	for name := range in.Boxes {
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
	}
	// a placed service can run only on a box that carries it
	can := func(name, svc string) bool {
		if _, ok := in.Boxes[name].Can[svc]; !ok {
			return false
		}
		return !in.Services[svc].Placed || s.Boxes[name].Facts["carries/"+svc] > 0
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
		sv, f := in.Services[svc], in.Boxes[name].Can[svc]
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
		if len(cands) == 0 {
			p.Unplaced[svc] = "no live box can run it"
			return
		}
		best := func(from []string) []string {
			out := []string{}
			for _, name := range from {
				if !planned[name][svc] && allowed(name, svc) && room(name, svc) {
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
	return p
}
