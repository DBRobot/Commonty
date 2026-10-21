// dd-plan-check: what a release would run where, by its placement, against
// what runs on the boxes now. Read-only: it reads the agreed state (a
// box's /state), the release's placement-inputs.json and needs.json, what
// each box would build in under that release (its roles and its
// carries.json), and each box's `systemctl list-units`, and prints, for
// each box, every service that runs there now and would not, and every one
// that would and does not now. It touches no box (scripts/plan-check
// gathers the files).
//
// The first round after the switch is what it plans: no placed service's
// flag is up yet (/run/dd-placed is set by the release's dd-control), so
// nothing placed counts as running anywhere, and the plan alone says where
// each starts. With -sticky, what runs now counts as running, as it does
// once every placed service has its flag.
package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"sort"
	"strconv"
	"strings"

	"commonty/control/place"
	"commonty/control/state"
)

// a box under the release: the roles it is built with (dd.control.roles)
// and the placed services it carries, with their units
type built struct {
	Roles   []string            `json:"roles"`
	Carries map[string][]string `json:"carries"`
}

// the needs' parts read here
type need struct {
	Units    []string `json:"units"`
	EveryBox bool     `json:"everyBox"`
}

type list map[string]string

func (l list) String() string { return fmt.Sprint(map[string]string(l)) }
func (l list) Set(v string) error {
	k, p, ok := strings.Cut(v, "=")
	if !ok {
		return fmt.Errorf("%q: want box=path", v)
	}
	l[k] = p
	return nil
}

func readJSON(path string, v any) {
	raw, err := os.ReadFile(path)
	if err != nil {
		fail(err)
	}
	if err := json.Unmarshal(raw, v); err != nil {
		fail(fmt.Errorf("%s: %w", path, err))
	}
}

func fail(err error) {
	fmt.Fprintln(os.Stderr, "dd-plan-check:", err)
	os.Exit(2)
}

// claims: whether unit u (with its suffix) is one of the service's: its
// own name, a unit its needs name, a name ending in * a prefix, one ending
// in @ a template's instances
func claims(svc string, n need, u string) bool {
	base := u
	for _, suf := range []string{".service", ".timer", ".socket", ".path"} {
		base = strings.TrimSuffix(base, suf)
	}
	for _, c := range append([]string{svc}, n.Units...) {
		switch {
		case strings.HasSuffix(c, "*"):
			if strings.HasPrefix(base, strings.TrimSuffix(c, "*")) {
				return true
			}
		case strings.HasSuffix(c, "@"):
			if strings.HasPrefix(base, c) {
				return true
			}
		case base == c:
			return true
		}
	}
	return false
}

// running: the services with a unit active now, by `systemctl list-units
// --all --plain --no-legend` (UNIT LOAD ACTIVE SUB ...)
func running(path string, needs map[string]need) map[string][]string {
	f, err := os.Open(path)
	if err != nil {
		fail(err)
	}
	defer f.Close()
	out := map[string][]string{}
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		fs := strings.Fields(sc.Text())
		if len(fs) < 4 || fs[1] != "loaded" {
			continue
		}
		if fs[2] != "active" && fs[2] != "activating" && fs[2] != "reloading" {
			continue
		}
		for svc, n := range needs {
			if claims(svc, n, fs[0]) {
				out[svc] = append(out[svc], fs[0])
			}
		}
	}
	if err := sc.Err(); err != nil {
		fail(err)
	}
	return out
}

func main() {
	statePath := flag.String("state", "", "the agreed state, as a box's 127.0.0.1:4195/state serves it")
	inputsPath := flag.String("placement", "", "the release's placement-inputs.json")
	needsPath := flag.String("needs", "", "the release's needs.json")
	builds, units := list{}, list{}
	flag.Var(builds, "built", "box=path: what the box builds in under the release ({roles, carries})")
	flag.Var(units, "units", "box=path: the box's systemctl list-units now")
	facts := list{}
	flag.Var(facts, "fact", "box:fact=value: a fact a box would report under the release, over what it reports now (svc/<service>/mem in bytes, ...)")
	planPath := flag.String("plan", "", "a plan a box serves (127.0.0.1:4195/plan): what it would change were it carried out whole, every service where it says, in place of planning with -placement")
	sticky := flag.Bool("sticky", false, "count what runs now as running: the plan once every placed service has its flag, not the first round after the switch")
	asJSON := flag.Bool("json", false, "print the plan and the differences as JSON")
	flag.Parse()
	if *statePath == "" || *inputsPath == "" || *needsPath == "" || len(units) == 0 {
		flag.Usage()
		os.Exit(2)
	}

	s := state.New()
	readJSON(*statePath, s)
	in := place.Inputs{}
	readJSON(*inputsPath, &in)
	needs := map[string]need{}
	readJSON(*needsPath, &needs)

	now := map[string]map[string][]string{}
	for box, p := range units {
		now[box] = running(p, needs)
	}

	// each box as the release would have it: its roles, what it carries,
	// and (first round) no placed service running
	rel := map[string]built{}
	for box, p := range builds {
		var b built
		readJSON(p, &b)
		rel[box] = b
	}
	before := map[string][]string{}
	for name, b := range s.Boxes {
		before[name] = b.Roles
	}
	for name, b := range s.Boxes {
		r, ok := rel[name]
		if !ok {
			continue
		}
		f := map[string]float64{}
		for k, v := range b.Facts {
			if !strings.HasPrefix(k, "carries/") && !strings.HasPrefix(k, "runs/") && !strings.HasPrefix(k, "refused/") {
				f[k] = v
			}
		}
		for svc := range r.Carries {
			f["carries/"+svc] = 1
			if _, on := now[name][svc]; *sticky && on {
				f["runs/"+svc] = 1
			}
		}
		b.Facts = f
		b.Roles = r.Roles
	}
	for k, v := range facts {
		box, fact, ok := strings.Cut(k, ":")
		n, err := strconv.ParseFloat(v, 64)
		if !ok || err != nil || s.Boxes[box] == nil {
			fail(fmt.Errorf("-fact %s=%s: want box:fact=number, for a box in the state", k, v))
		}
		s.Boxes[box].Facts[fact] = n
	}
	goneAfter := int64(90_000)
	var plan place.Plan
	if *planPath != "" {
		readJSON(*planPath, &plan)
	} else {
		plan = place.Decide(in, s, goneAfter, nil)
	}

	// what the release runs on each box: what the plan gives it of what
	// it carries or runs on every box, and what its roles bring that the
	// control plane does not switch
	after := map[string]map[string]string{}
	for box := range units {
		if _, ok := rel[box]; !ok {
			rel[box] = built{Roles: before[box]}
		}
		a := map[string]string{}
		for _, d := range plan.Boxes[box] {
			sv := in.Services[d.Service]
			if *planPath != "" {
				// a live plan as if it were carried out whole
				a[d.Service] = d.Why
				continue
			}
			if !sv.Placed {
				// not switched: it runs where its roles bring it, whatever
				// the plan says
				continue
			}
			if _, ok := rel[box].Carries[d.Service]; ok {
				a[d.Service] = d.Why
			}
		}
		for svc, sv := range in.Services {
			if sv.Placed || *planPath != "" {
				continue
			}
			if sv.EveryBox {
				a[svc] = "every box"
				continue
			}
			for _, r := range rel[box].Roles {
				for _, x := range in.Roles[r] {
					if x == svc {
						a[svc] = "its role " + r + " (not switched)"
					}
				}
			}
		}
		after[box] = a
	}

	type diff struct {
		Stops  map[string][]string `json:"stops"`  // service -> the units active now
		Starts map[string]string   `json:"starts"` // service -> the plan's why
		Same   []string            `json:"same"`
		// built in by roles it has now as well, or on every box, with no
		// unit active now: a service of nginx's or the gate's, a oneshot
		Quiet []string `json:"quiet"`
	}
	out := map[string]diff{}
	boxes := []string{}
	for box := range units {
		boxes = append(boxes, box)
	}
	sort.Strings(boxes)
	for _, box := range boxes {
		d := diff{Stops: map[string][]string{}, Starts: map[string]string{}}
		for svc, us := range now[box] {
			if _, ok := after[box][svc]; ok {
				d.Same = append(d.Same, svc)
			} else {
				d.Stops[svc] = us
			}
		}
		had := map[string]bool{}
		for _, r := range before[box] {
			for _, x := range in.Roles[r] {
				had[x] = true
			}
		}
		for svc, why := range after[box] {
			if _, ok := now[box][svc]; ok {
				continue
			}
			if !in.Services[svc].Placed && (had[svc] || in.Services[svc].EveryBox) {
				d.Quiet = append(d.Quiet, svc)
				continue
			}
			d.Starts[svc] = why
		}
		sort.Strings(d.Same)
		sort.Strings(d.Quiet)
		out[box] = d
	}
	if *asJSON {
		_ = json.NewEncoder(os.Stdout).Encode(map[string]any{"plan": plan, "boxes": out})
		return
	}
	bad := false
	for _, box := range boxes {
		d := out[box]
		fmt.Printf("%s: %d services run now and would run\n", box, len(d.Same))
		for _, svc := range sorted(d.Stops) {
			bad = true
			fmt.Printf("  STOPS  %-22s runs now (%s)\n", svc, strings.Join(d.Stops[svc], " "))
		}
		for _, svc := range sorted(d.Starts) {
			fmt.Printf("  starts %-22s %s\n", svc, d.Starts[svc])
		}
		if len(d.Quiet) > 0 {
			fmt.Printf("  built in as before, no unit active now: %s\n", strings.Join(d.Quiet, " "))
		}
	}
	for svc, why := range plan.Unplaced {
		fmt.Printf("unplaced %s: %s\n", svc, why)
	}
	if bad {
		os.Exit(1)
	}
}

func sorted[V any](m map[string]V) []string {
	out := make([]string, 0, len(m))
	for k := range m {
		out = append(out, k)
	}
	sort.Strings(out)
	return out
}
