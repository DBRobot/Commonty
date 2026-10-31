package place

import (
	"maps"
	"slices"
	"sort"

	"commonty/control/state"
	"commonty/control/state/ident"
)

// Owners: whose each joined box is, by the agreed identity state: an
// enlistment that stands, of someone who is a member now. A ban, their leaving
// or an unenlist takes it out the moment it is agreed, and every box and the
// witness read it from here, so each plans alike.
func Owners(v *ident.View) map[string]string {
	out := map[string]string{}
	if v == nil {
		return out
	}
	for name, m := range v.Members {
		if m.Status != "member" {
			continue
		}
		for _, b := range m.Enlisted {
			out[b.Box] = name
		}
	}
	return out
}

// ownerCan: what a joined box may be given in the plan the fleet shares, by
// whose it is. Nobody's: nothing. A member's: what reads nobody's data. The
// fleet owner's: what they keep to their own boxes too, as far as it says it
// can run it, with no lease until leases are for such boxes. What a box says of
// itself only narrows this; whose it is comes from the agreed state alone.
func ownerCan(in Inputs, f map[string]float64, owner string) map[string]float64 {
	if owner == "" {
		return map[string]float64{}
	}
	c := joinedCan(in, f)
	if owner != in.Owner || in.Joined == nil || !in.Joined.Attested || f["can_live"] == 0 {
		return c
	}
	for svc, sv := range in.Services {
		if v, said := f["can/"+svc]; said && sv.Placed && !sv.Lease && !sv.EveryBox && !slices.Contains(in.Joined.Named, svc) {
			c[svc] = v
		}
	}
	return c
}

// siteOf: the house a box is in. A joined box's house is what it said when it
// joined, among its owner's boxes alone, so nobody's word puts a box in another
// member's house; one that said nothing it can name is a house of its own.
func siteOf(in Inputs, s *state.State, owners map[string]string, name string) string {
	if b, listed := in.Boxes[name]; listed {
		return b.Site
	}
	if j := s.Joined[name]; j != nil && j.House != "" && j.House != JoinedNoHouse && owners[name] != "" {
		return owners[name] + "/" + j.House
	}
	return name
}

// ForgetHold is how long after an enlistment ended a box removes what is on
// its disk of that member's: services stop at once, by the plan; a disk goes
// only once the word that ended it has stood this long (ms)
const ForgetHold = int64(3600_000)

// MayForget: of what a plan says, what any box must also hold true by the same
// state before it removes anything: the word has stood long enough, and the box
// is not that member's now. Decide and the box each ask, so neither alone can
// remove what the other would keep.
func MayForget(e ident.Ended, now int64, owner string) bool {
	return e.Member != "" && e.Member != owner && e.At > 0 && now-e.At >= ForgetHold
}

// forgets: which live joined boxes remove which member's plaintext. Nothing
// before the switch that lets joined boxes run anything, as they have none
func forgets(in Inputs, s *state.State, live []string, owners map[string]string) map[string]ident.Ended {
	if in.Joined == nil || !in.Joined.Attested {
		return nil
	}
	var out map[string]ident.Ended
	for _, name := range live {
		e := in.Ended[name]
		_, listed := in.Boxes[name]
		if e == nil || listed || s.Joined[name] == nil || !MayForget(*e, s.Time, owners[name]) {
			continue
		}
		if out == nil {
			out = map[string]ident.Ended{}
		}
		out[name] = *e
	}
	return out
}

// JoinedNoHouse is the house a box says when it can name none
// (nix/modules/box/control.nix)
const JoinedNoHouse = "joined"

// what placeOwn uses of Decide's judgement
type ownPlace struct {
	can      func(name, svc string) bool
	room     func(name, svc string) bool
	fit      func(name, svc string) (float64, string)
	place    func(name, svc string, f float64, why string)
	adds     func(name, svc string) []string
	draining func(name string) bool
	planned  map[string]map[string]bool
}

func isOwn(p Plan, name, svc string) bool {
	return slices.ContainsFunc(p.Boxes[name], func(d Decision) bool { return d.Service == svc && d.Own })
}

// placeOwn gives each member their own of what reads only its box's owner's
// data, on their own boxes and no others': as many as the fleet asks of it, for
// each member apart, or one per house of theirs, or on every box of theirs.
// Once the fleet's plan is made, so none of this can move a service off a box of
// the release, and it is never the instance the fleet shares. Once on a box, it
// stays while the box can run it. The fleet owner's boxes are in the fleet's
// plan already. Only ever a box's owner by the agreed state: losing that, it is
// not given, and the box stops it (cmd/dd-control/placed.go).
func placeOwn(in Inputs, s *state.State, p *Plan, live []string, owners map[string]string, canOf map[string]map[string]float64, o ownPlace) {
	if in.Joined == nil || !in.Joined.Attested {
		return
	}
	theirs := map[string][]string{}
	for _, name := range live {
		_, listed := in.Boxes[name]
		if owner := owners[name]; !listed && owner != "" && owner != in.Owner {
			theirs[owner] = append(theirs[owner], name)
		}
	}
	people := make([]string, 0, len(theirs))
	for owner := range theirs {
		people = append(people, owner)
	}
	sort.Strings(people)
	own := slices.Sorted(slices.Values(in.Joined.Own))
	for _, owner := range people {
		for _, name := range theirs[owner] {
			f := s.Boxes[name].Facts
			if f["can_live"] == 0 {
				continue
			}
			for _, svc := range own {
				sv, known := in.Services[svc]
				if v, said := f["can/"+svc]; known && said && sv.Enabled && !sv.Lease {
					canOf[name][svc] = v
				}
			}
		}
		for _, svc := range own {
			sv := in.Services[svc]
			able := []string{}
			for _, name := range theirs[owner] {
				if _, ok := canOf[name][svc]; !ok || !o.can(name, svc) {
					continue
				}
				// what it brings beside it must be able to run here too
				if !slices.ContainsFunc(o.adds(name, svc), func(d string) bool {
					return d != svc && !in.Services[d].EveryBox && !o.can(name, d)
				}) {
					able = append(able, name)
				}
			}
			if len(able) == 0 {
				continue
			}
			give := func(boxes []string, n int) {
				runs := func(name string) bool { return s.Boxes[name].Facts["runs/"+svc] > 0 }
				for _, name := range boxes {
					if o.planned[name][svc] && isOwn(*p, name, svc) {
						n--
					}
				}
				for ; n > 0; n-- {
					order := []string{}
					for _, name := range boxes {
						if !o.planned[name][svc] && !o.draining(name) && (runs(name) || o.room(name, svc)) {
							order = append(order, name)
						}
					}
					if len(order) == 0 {
						p.Unplaced[svc+"@"+owner] = "none of its owner's live boxes has room for it"
						return
					}
					sort.SliceStable(order, func(i, j int) bool {
						if a, b := runs(order[i]), runs(order[j]); a != b {
							return a
						}
						fi, _ := o.fit(order[i], svc)
						fj, _ := o.fit(order[j], svc)
						return fi > fj
					})
					top := order[0]
					f, why := o.fit(top, svc)
					from := len(p.Boxes[top])
					o.place(top, svc, f, "its owner's: "+why)
					for i := from; i < len(p.Boxes[top]); i++ {
						p.Boxes[top][i].Own = true
					}
				}
			}
			switch {
			case sv.Spread:
				give(able, len(able))
			case sv.PerHouse > 0:
				houses := map[string][]string{}
				for _, name := range able {
					h := siteOf(in, s, owners, name)
					houses[h] = append(houses[h], name)
				}
				for _, h := range slices.Sorted(maps.Keys(houses)) {
					give(houses[h], sv.PerHouse)
				}
			default:
				give(able, max(sv.Instances, 1))
			}
		}
	}
}
