package main

// The plan, carried out on this box: a placed service's units start only
// where /run/dd-placed/<svc> exists (nix/modules/box/placed.nix makes it
// their condition), and this is what puts the flag there and takes it
// away, by the plan every box computes from the agreed state. Only this
// box's own services, and only from a plan made from agreement that is
// fresh: with none, what runs keeps running.
//
// A service with one writer (its needs' lease) runs only while this box
// holds the lease svc/<name> in the agreed state: asked for when the plan
// gives it the service, renewed while it runs, given back when the plan
// moves it. Its flag holds the lease's epoch and when it lapses, and the
// service stops before then unless a renewal is seen agreed, fresh or not:
// a box cut off stops its writer before anyone else may take the lease.
//
// A service is not started where it would not fit: when this box has less
// memory free than it holds, the box refuses it and says so in its
// heartbeat (refused/<svc>), and the plan gives it to another box. And the
// plan's copies (which boxes' stores keep each bucket) go to a file the
// store's copier reads (nix/modules/storage/store.nix).
//
// A remote computer the plan gives this box is a writer like these: the
// lease svc/computer-<id>, granted only while the computer is agreed up
// (state/computerlease.go), its flag /run/dd-placed/computer-<id>, and its
// one unit dd-computer@<id>, which fetches the agreed record as it starts.

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"time"

	"commonty/control/place"
	"commonty/control/state"
	"commonty/control/state/computers"
)

// a leased service stops this long before its lease could lapse, and a new
// holder starts it this long after the lease is first seen its own: for
// how much faster the agreed clock may run than a box's, and a stop's own
// time
const leaseMargin = 3 * time.Second

type placer struct {
	box string
	// where the flags are (/run/dd-placed): a tmpfs, so nothing placed
	// starts at boot before the plan says so
	dir string
	// service -> its units on this box, the services this box carries; a
	// template's instances as a pattern (dd-game@*.service), for stopping
	carries map[string][]string
	// service -> the units of it that start it (those that start at boot):
	// a timer, not the run it starts; none of a template's instances. A
	// service not here starts all its units
	starts map[string][]string
	// the carried services with one writer
	leased map[string]bool
	// systemctl, or a test's stand-in
	run func(args ...string) error
	// a lease request, signed and sent like a heartbeat: its sequence
	ask func(op state.LeaseOp) (uint64, error)
	// ms since boot (uptime below), or a test's clock
	uptime func() int64
	// how long a lease lasts past its grant or renewal
	ttl time.Duration
	// a leased service: when (uptime) this box first saw the lease its own,
	// and when it sent each request still unanswered
	since map[string]int64
	sent  map[string]map[uint64]int64
	// GiB a service holds, as the release estimates it (place.Service),
	// and what it keeps aside whatever it holds
	memory  map[string]float64
	reserve map[string]float64
	// bytes of memory free now (MemAvailable), or a test's
	free func() float64
	// a service refused for want of room: when (uptime), told for refuseFor
	refused map[string]int64
	// where the plan's copies are written; empty: not at all
	copies string
	// a joined box holds no lease yet (leases on such boxes are for later):
	// a computer it is given runs by the plan alone, on its owner's box and
	// its disk there, for as long as fresh plans keep giving it (ownComputer)
	noLeases bool
	// the computers agreed now (computer-<id>), from the same state as the
	// plan; nil, or an error, and none runs here
	agreed func(s *state.State) (map[string]*computers.Listed, error)
	// what one member's computers may take together, and the ports' pool
	// (fleet/policy.json computers): held to on this box too
	limits *computers.Limits
	// the services checked once for what of them runs without a flag
	swept map[string]bool
	// a writer handed over: its last writes out to the other stores, done
	// within the time given (finalCopy); nil: none
	final func(svc string, within time.Duration) error
	// a writer being handed over whose final copy has not succeeded yet:
	// since when (uptime). Its flag stays, and its lease is renewed
	handing map[string]int64
	// a writer taken over from another box (or from none): when (uptime)
	// it was started here under its lease, until it is up; and one that
	// failed to start since, refused: a box that took a writer it cannot
	// run (no base backup, a key it cannot open) says so, and the plan
	// gives it to a box that can (2026-10-24, node2). One taking back its
	// own lapsed lease has the writer's data, and is never refused. A
	// refusal is kept beside the flags (.refused-<svc>), so a dd-control
	// started again keeps it, and a boot forgets it
	started, failed map[string]int64
	// who held a writer's lease when this box asked for it
	before map[string]string
	// a writer's own unit as systemd has it now, or a test's stand-in
	show func(unit string) (unitState, error)
	// a writer whose lease is held by no one: since when (uptime) this box
	// has seen it so
	absent map[string]int64
}

// what systemd says of a unit
type unitState struct {
	Type, Active, Job string
}

// how long a box that did not hold a writer's lapsed lease waits before it
// asks for it: the box that held it, which has its data, asks at once
// (placed.go takeBack), and gets it back first
const takeBack = 20 * time.Second

// how long a handover asks again for its final copy before it gives the
// lease back without it, its last writes kept here alone
const handoverFor = 2 * time.Minute

var errNotCopied = errors.New("not handed over yet: its last writes not copied out, or it still runs")

// how long a box says it refused a service: the plan gives it elsewhere,
// and it may come back once that has passed
const refuseFor = 10 * time.Minute

// what is kept free whatever a service asks
const keepFree = 512 << 20

// what a leased service's flag holds
type held struct {
	Epoch uint64 `json:"epoch"`
	Until int64  `json:"until"` // ms since boot (/proc/uptime): when it stops without a renewal
	// being handed over: its units refuse to start (box/placed.nix), so
	// nothing writes while or after its last writes are copied out
	Handing bool `json:"handing,omitempty"`
}

func leaseOf(svc string) string { return "svc/" + svc }

func loadCarries(path string) (map[string][]string, error) {
	out := map[string][]string{}
	raw, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return out, nil
	}
	if err != nil {
		return nil, err
	}
	return out, json.Unmarshal(raw, &out)
}

func systemctl(args ...string) error {
	out, err := exec.Command("systemctl", args...).CombinedOutput()
	if err != nil {
		return fmt.Errorf("systemctl %v: %v: %s", args, err, out)
	}
	return nil
}

// a remote computer's flag: its id is 16 lowercase hex (state/computers)
var computerFlag = regexp.MustCompile(`^computer-[0-9a-f]{16}$`)

// a placed service's units: a computer's is its own, the rest the release's
func (p *placer) units(svc string) []string {
	if computerFlag.MatchString(svc) {
		return []string{"dd-computer@" + svc[len("computer-"):] + ".service"}
	}
	return p.carries[svc]
}

// a writer: under a lease, stopped before its lease is given back
func (p *placer) writer(svc string) bool { return p.leased[svc] || computerFlag.MatchString(svc) }

// the computers whose flags are up here
func (p *placer) computers() []string {
	out := []string{}
	es, _ := os.ReadDir(p.dir)
	for _, e := range es {
		if computerFlag.MatchString(e.Name()) {
			out = append(out, e.Name())
		}
	}
	return out
}

// facts this box reports about its placed services: what it carries, and
// what it runs (a flag up)
func (p *placer) facts() map[string]float64 {
	f := map[string]float64{}
	for _, svc := range p.computers() {
		f["runs/"+svc] = 1
	}
	now := p.uptime()
	for svc := range p.carries {
		f["carries/"+svc] = 1
		if p.up(svc) {
			f["runs/"+svc] = 1
		}
		if at, ok := p.refused[svc]; ok && now-at < refuseFor.Milliseconds() {
			f["refused/"+svc] = 1
		}
		if p.failedLately(svc) {
			f["refused/"+svc] = 1
		}
	}
	return f
}

// fits: whether svc may start here now, by what it holds - the most any
// box measured it holding, never under the release's estimate - against
// what this box has free. A refusal is remembered and told.
func (p *placer) fits(svc string, s *state.State) bool {
	// a computer is sized by the plan, against this box's own room for them
	if p.free == nil || computerFlag.MatchString(svc) {
		return true
	}
	// what it held here, when it ran here (a CI runner takes as many jobs
	// as its box holds), never under what it keeps aside; else the most
	// any box measured it holding, never under the release's estimate
	need := p.memory[svc] * (1 << 30)
	if b := s.Boxes[p.box]; b != nil && b.Facts["svc/"+svc+"/mem"] > 0 {
		need = max(b.Facts["svc/"+svc+"/mem"], p.reserve[svc]*(1<<30))
	} else {
		for _, b := range s.Boxes {
			need = max(need, b.Facts["svc/"+svc+"/mem"])
		}
	}
	if free := p.free(); free > 0 && free-need < keepFree {
		if _, ok := p.refused[svc]; !ok || p.uptime()-p.refused[svc] >= refuseFor.Milliseconds() {
			log.Printf("placed: refusing %s: it holds %.1f GiB, %.1f GiB free here", svc, need/(1<<30), free/(1<<30))
		}
		p.refused[svc] = p.uptime()
		return false
	}
	delete(p.refused, svc)
	return true
}

// writeCopies puts the plan's copies where the store's copier reads them,
// when they change
func (p *placer) writeCopies(c map[string][]string) {
	if p.copies == "" {
		return
	}
	raw, _ := json.Marshal(c)
	if old, err := os.ReadFile(p.copies); err == nil && string(old) == string(raw) {
		return
	}
	tmp := p.copies + ".new"
	if err := os.WriteFile(tmp, raw, 0o644); err != nil {
		log.Printf("placed: copies: %v", err)
		return
	}
	if err := os.Rename(tmp, p.copies); err != nil {
		log.Printf("placed: copies: %v", err)
	}
}

func (p *placer) flag(svc string) string { return filepath.Join(p.dir, svc) }

func (p *placer) up(svc string) bool {
	_, err := os.Stat(p.flag(svc))
	return err == nil
}

// a leased service's flag: zero when there is none, or it is not readable
func (p *placer) holding(svc string) held {
	var h held
	raw, err := os.ReadFile(p.flag(svc))
	if err == nil {
		_ = json.Unmarshal(raw, &h)
	}
	return h
}

func (p *placer) raise(svc string, content []byte) error {
	if err := os.MkdirAll(p.dir, 0o755); err != nil {
		return err
	}
	tmp := p.flag(svc) + ".new"
	if err := os.WriteFile(tmp, content, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, p.flag(svc))
}

func (p *placer) start(svc string) {
	units := p.units(svc)
	if us, ok := p.starts[svc]; ok && !computerFlag.MatchString(svc) {
		units = us
	}
	if len(units) == 0 {
		log.Printf("placed: %s runs here now (nothing of it starts by itself)", svc)
		return
	}
	if err := p.run(append([]string{"start", "--no-block"}, units...)...); err != nil {
		log.Printf("placed: starting %s: %v", svc, err)
	} else {
		log.Printf("placed: %s runs here now", svc)
	}
}

// stop takes the flag down. A writer is stopped before its flag goes, and
// waited for: its lease is given back only once it no longer writes. One
// handed over sends its last writes on first, while the flag still says the
// lease is this box's: the store copies only for the box that writes
func (p *placer) stop(svc, why string, handover bool) error {
	args := []string{"stop", "--no-block"}
	if p.writer(svc) {
		args = []string{"stop"}
	}
	if err := p.run(append(args, p.units(svc)...)...); err != nil {
		log.Printf("placed: stopping %s: %v", svc, err)
		return err
	}
	if handover && p.writer(svc) {
		// a service with buckets: its last writes out first, with its units
		// refusing to start meanwhile, then stopped again. A computer has
		// none to copy (its disk is its box's)
		if p.leased[svc] && p.final != nil {
			if err := p.mark(svc, true); err != nil {
				log.Printf("placed: %s: %v", svc, err)
				return errNotCopied
			}
			if err := p.copyOut(svc); err != nil {
				return err
			}
			if err := p.run(append([]string{"stop"}, p.units(svc)...)...); err != nil {
				log.Printf("placed: stopping %s: %v", svc, err)
				return errNotCopied
			}
		}
		// none of it runs when the flag goes and the lease is given back
		if p.run(append([]string{"is-active", "--quiet"}, p.units(svc)...)...) == nil {
			log.Printf("placed: %s: still running after its stop", svc)
			return errNotCopied
		}
	}
	_ = os.Remove(p.flag(svc))
	delete(p.since, svc)
	delete(p.handing, svc)
	log.Printf("placed: %s no longer runs here (%s)", svc, why)
	return nil
}

// mark sets whether a leased service's flag says it is being handed over,
// keeping its epoch and until
func (p *placer) mark(svc string, handing bool) error {
	h := p.holding(svc)
	h.Handing = handing
	content, _ := json.Marshal(h)
	return p.raise(svc, content)
}

// copyOut runs the final copy, bounded by the flag's until: nothing is sent
// once another box may hold the lease
func (p *placer) copyOut(svc string) error {
	left := time.Duration(p.holding(svc).Until-p.uptime()) * time.Millisecond
	if left <= 0 {
		log.Printf("placed: %s: no time left for its final copy", svc)
		return errNotCopied
	}
	if err := p.final(svc, left); err != nil {
		log.Printf("placed: %s: final copy: %v", svc, err)
		// whatever of it the copy started (the forge's own copy) is stopped
		_ = p.run(append([]string{"stop"}, p.units(svc)...)...)
		return errNotCopied
	}
	return nil
}

// finalCopy is the store's final copy of a service's buckets
// (nix/modules/storage/store.nix), stopped if not done in time
func finalCopy(svc string, within time.Duration) error {
	unit := "dd-store-copy-final@" + svc + ".service"
	ctx, cancel := context.WithTimeout(context.Background(), within)
	defer cancel()
	out, err := exec.CommandContext(ctx, "systemctl", "start", unit).CombinedOutput()
	if ctx.Err() != nil {
		_ = systemctl("stop", unit)
		return fmt.Errorf("%s: not done within %v", unit, within)
	}
	if err != nil {
		return fmt.Errorf("systemctl start %s: %v: %s", unit, err, out)
	}
	return nil
}

func (p *placer) names() []string {
	names := make([]string, 0, len(p.carries))
	for svc := range p.carries {
		names = append(names, svc)
	}
	sort.Strings(names)
	return names
}

// fence stops every leased service whose lease may have lapsed: called
// every round, whatever the agreed state's age
func (p *placer) fence() {
	now := p.uptime()
	for _, svc := range append(p.names(), p.computers()...) {
		if p.writer(svc) && p.up(svc) && now >= p.holding(svc).Until {
			_ = p.stop(svc, "its lease was not seen renewed", false)
		}
	}
}

// allowed: the computers the plan gives this box that it runs - agreed up
// now, and within the limits on this box as well as in the judge: each
// owner's here together within their totals, every port in the pool and
// none twice. In id order, so a box over a limit refuses the same ones
// every round
func (p *placer) allowed(want map[string]bool, s *state.State) map[string]bool {
	out := map[string]bool{}
	if p.agreed == nil || p.limits == nil {
		return out
	}
	agreed, err := p.agreed(s)
	if err != nil {
		log.Printf("placed: no computer runs here: %v", err)
		return out
	}
	type sum struct{ mem, threads, disk uint64 }
	owners := map[string]*sum{}
	ports := map[uint64]bool{}
	l := p.limits
	inPool := func(port uint64) bool { return port >= l.PortBase && port < l.PortBase+l.PortCount }
	for _, svc := range slices.Sorted(maps.Keys(want)) {
		c := agreed[svc]
		if !computerFlag.MatchString(svc) || c == nil || !c.Up || place.ComputerService(c.ID) != svc {
			continue
		}
		t := owners[c.Owner]
		if t == nil {
			t = &sum{}
		}
		n := sum{t.mem + c.Memory, t.threads + c.Threads, t.disk + c.Disk}
		mine := []uint64{c.Port}
		for _, f := range c.Forwards {
			mine = append(mine, f.Host)
		}
		why := ""
		switch {
		case n.mem > l.Memory || n.threads > l.Threads || n.disk > l.Disk:
			why = fmt.Sprintf("%s's computers here would take more than %d MiB, %d threads, %d GiB", c.Owner, l.Memory, l.Threads, l.Disk)
		case slices.ContainsFunc(mine, func(port uint64) bool { return !inPool(port) || ports[port] }):
			why = "a port outside the pool, or one another computer here has"
		}
		if why != "" {
			log.Printf("placed: %s does not run here: %s", svc, why)
			continue
		}
		for _, port := range mine {
			ports[port] = true
		}
		owners[c.Owner] = &n
		out[svc] = true
	}
	return out
}

// reconcile makes this box run what the plan gives it of what it carries,
// and nothing else of it. A plan this box is not in (it does not count
// itself alive) changes nothing: it is the plan's view that is behind.
func (p *placer) reconcile(plan place.Plan, s *state.State) {
	mine, in := plan.Boxes[p.box]
	if !in {
		return
	}
	p.writeCopies(plan.Copies)
	want := map[string]bool{}
	for _, d := range mine {
		want[d.Service] = true
	}
	// the computers given here, running here, or whose lease is this box's
	cs := map[string]bool{}
	for svc := range want {
		if computerFlag.MatchString(svc) {
			cs[svc] = true
		}
	}
	for _, svc := range p.computers() {
		cs[svc] = true
	}
	for role, l := range s.Leases {
		if svc, ok := strings.CutPrefix(role, "svc/"); ok && computerFlag.MatchString(svc) && l.Holder == p.box {
			cs[svc] = true
		}
	}
	ok := p.allowed(want, s)
	for _, svc := range slices.Sorted(maps.Keys(cs)) {
		if p.noLeases {
			p.ownComputer(svc, want[svc] && ok[svc])
			continue
		}
		p.reconcileLeased(svc, want[svc] && ok[svc], false, s)
	}
	// what the plan gives any box
	anywhere := map[string]bool{}
	for _, ds := range plan.Boxes {
		for _, d := range ds {
			anywhere[d.Service] = true
		}
	}
	for _, svc := range p.names() {
		if p.leased[svc] {
			p.reconcileLeased(svc, want[svc], anywhere[svc], s)
			continue
		}
		switch {
		case want[svc] && !p.up(svc):
			if !p.fits(svc, s) {
				continue
			}
			if err := p.raise(svc, nil); err != nil {
				log.Printf("placed: %s: %v", svc, err)
				continue
			}
			p.start(svc)
		case !want[svc] && p.up(svc):
			_ = p.stop(svc, "the plan moved it", false)
		case !want[svc] && !p.swept[svc]:
			// once, the first time the plan does not give it here: what
			// of it still runs without a flag stops. A switch that first
			// gives a running service its flag's condition leaves its
			// sockets listening (switch-to-configuration does not touch a
			// changed socket)
			if p.swept == nil {
				p.swept = map[string]bool{}
			}
			p.swept[svc] = true
			if p.run(append([]string{"is-active", "--quiet"}, p.units(svc)...)...) == nil {
				log.Printf("placed: %s runs here without its flag; stopping it", svc)
				_ = p.run(append([]string{"stop", "--no-block"}, p.units(svc)...)...)
			}
		}
	}
}

// how long a computer on a box without leases runs past the last fresh plan
// that gave it: its flag's until, which the fence stops it at
const ownComputerFor = 2 * freshFor

// ownComputer: a computer on a joined box, run while fresh plans give it here
func (p *placer) ownComputer(svc string, want bool) {
	if !want {
		if p.up(svc) {
			_ = p.stop(svc, "the plan does not give it here", false)
		}
		return
	}
	first := !p.up(svc)
	content, _ := json.Marshal(held{Until: p.uptime() + ownComputerFor.Milliseconds()})
	if err := p.raise(svc, content); err != nil {
		log.Printf("placed: %s: %v", svc, err)
		return
	}
	if first {
		p.start(svc)
	}
}

func (p *placer) failedLately(svc string) bool {
	at, ok := p.failed[svc]
	return ok && p.uptime()-at < refuseFor.Milliseconds()
}

func (p *placer) refusedPath(svc string) string { return filepath.Join(p.dir, ".refused-"+svc) }

// refuse remembers a writer this box failed to start, beside the flags
func (p *placer) refuse(svc string, at int64) {
	if p.failed == nil {
		p.failed = map[string]int64{}
	}
	p.failed[svc] = at
	if err := os.MkdirAll(p.dir, 0o755); err == nil {
		_ = os.WriteFile(p.refusedPath(svc), []byte(strconv.FormatInt(at, 10)), 0o644)
	}
}

func (p *placer) unrefuse(svc string) {
	delete(p.failed, svc)
	_ = os.Remove(p.refusedPath(svc))
}

// loadRefusals takes back the refusals a dd-control before this one made
// since boot: kept by the same clock, they last as long as they would have
func (p *placer) loadRefusals() {
	es, _ := os.ReadDir(p.dir)
	for _, e := range es {
		svc, ok := strings.CutPrefix(e.Name(), ".refused-")
		if !ok {
			continue
		}
		raw, err := os.ReadFile(filepath.Join(p.dir, e.Name()))
		at, perr := strconv.ParseInt(strings.TrimSpace(string(raw)), 10, 64)
		if err != nil || perr != nil || p.uptime()-at >= refuseFor.Milliseconds() || at > p.uptime() {
			_ = os.Remove(filepath.Join(p.dir, e.Name()))
			continue
		}
		if p.failed == nil {
			p.failed = map[string]int64{}
		}
		p.failed[svc] = at
	}
}

func (p *placer) heldPath(svc string) string { return filepath.Join(p.dir, ".held-"+svc) }

// lastHeld: the epoch of a writer's lease this box held last, kept beside
// the flags while its lease lapses and a dd-control starts again, and
// cleared when it gives the lease back; 0, none
func (p *placer) lastHeld(svc string) uint64 {
	raw, err := os.ReadFile(p.heldPath(svc))
	if err != nil {
		return 0
	}
	e, _ := strconv.ParseUint(strings.TrimSpace(string(raw)), 10, 64)
	return e
}

func (p *placer) setHeld(svc string, epoch uint64) {
	if epoch == 0 {
		_ = os.Remove(p.heldPath(svc))
		return
	}
	if p.lastHeld(svc) == epoch {
		return
	}
	if err := os.MkdirAll(p.dir, 0o755); err == nil {
		_ = os.WriteFile(p.heldPath(svc), []byte(strconv.FormatUint(epoch, 10)), 0o644)
	}
}

// startUnits: what starts a service, of its units
func (p *placer) startUnits(svc string) []string {
	if us, ok := p.starts[svc]; ok && !computerFlag.MatchString(svc) {
		return us
	}
	return p.units(svc)
}

// judge: whether a writer started here has failed to start, or is up. Only
// its own unit, the first service of what starts it, counts: a helper of
// it failing while it comes up (a handover's archive at the wrong epoch,
// museum's health check before museum answers; 2026-10-25/26, each of
// which refused a holder's own writer 5 s after it started) says nothing
// of whether this box can run it. Coming up, restarting, or waiting on a
// job (a restore can take many minutes) is neither; failed, or stopped
// with nothing queued (what it needs failed), is failed
func (p *placer) judge(svc string) (failed, up bool) {
	unit := ""
	for _, u := range p.startUnits(svc) {
		if strings.HasSuffix(u, ".service") {
			unit = u
			break
		}
	}
	if unit == "" || p.show == nil {
		return false, true
	}
	st, err := p.show(unit)
	if err != nil {
		return false, false
	}
	switch {
	case st.Active == "active":
		return false, true
	case st.Job != "":
		return false, false
	case st.Active == "failed":
		return true, false
	case st.Active == "inactive":
		// a oneshot that ran to its end is done
		return st.Type != "oneshot", st.Type == "oneshot"
	}
	return false, false
}

// systemdShow: what systemd says of a unit now
func systemdShow(unit string) (unitState, error) {
	out, err := exec.Command("systemctl", "show", "-p", "Type,ActiveState,Job", unit).Output()
	if err != nil {
		return unitState{}, fmt.Errorf("systemctl show %s: %v", unit, err)
	}
	var st unitState
	for _, line := range strings.Split(string(out), "\n") {
		k, v, _ := strings.Cut(line, "=")
		switch k {
		case "Type":
			st.Type = v
		case "ActiveState":
			st.Active = v
		case "Job":
			st.Job = v
		}
	}
	return st, nil
}

// whether this box is being emptied (dd-drain, or switching release): it
// takes no writer back
func (p *placer) leaving(s *state.State) bool {
	b := s.Boxes[p.box]
	return b != nil && (b.Facts["draining"] > 0 || b.Facts["release_drain"] > 0)
}

// reconcileLeased: want, the plan gives it here; anywhere, it gives it to
// some box
func (p *placer) reconcileLeased(svc string, want, anywhere bool, s *state.State) {
	role := leaseOf(svc)
	l := s.Leases[role]
	taken := l != nil && l.Expires > s.Time
	mine := taken && l.Holder == p.box
	now := p.uptime()
	// given here though this box refused it: the plan does that only when
	// every box able to run it refused it (place.Decide), so it is tried
	// again rather than left nowhere
	if want && p.failedLately(svc) && s.Boxes[p.box] != nil && s.Boxes[p.box].Facts["refused/"+svc] > 0 {
		log.Printf("placed: %s: given here though this box refused it, as no box able to run it would; trying it again", svc)
		p.unrefuse(svc)
	}
	if !mine {
		delete(p.since, svc)
		if p.up(svc) {
			// agreed and fresh: someone else's, or nobody's
			_ = p.stop(svc, "its lease is not this box's", false)
		}
		if p.absent == nil {
			p.absent = map[string]int64{}
		}
		if _, seen := p.absent[svc]; taken {
			delete(p.absent, svc)
		} else if !seen {
			p.absent[svc] = now
		}
		// its lease lapsed, and this box held it last (the agreed newest
		// epoch was its own): it has the writer's data, and asks for it
		// again at once, given here or not, unless it is being emptied.
		// Holding it, the plan keeps it here
		last := p.leased[svc] && !taken && p.lastHeld(svc) > 0 && p.lastHeld(svc) == s.Epochs[role] && !p.leaving(s)
		if !(want || (last && anywhere)) || !p.fits(svc, s) || p.failedLately(svc) {
			return
		}
		// held by no one, and by another box last: that box may still take
		// it back, and goes first
		if p.leased[svc] && !taken && !last && s.Epochs[role] > 0 && now-p.absent[svc] < takeBack.Milliseconds() {
			return
		}
		if p.before == nil {
			p.before = map[string]string{}
		}
		p.before[svc] = ""
		if l != nil {
			p.before[svc] = l.Holder
		}
		if last {
			p.before[svc] = p.box
		}
		p.send(svc, state.LeaseOp{Role: role, Op: "acquire"})
		return
	}
	delete(p.absent, svc)
	if p.leased[svc] {
		p.setHeld(svc, l.Epoch)
	}
	if !want {
		if p.up(svc) {
			if p.handing == nil {
				p.handing = map[string]int64{}
			}
			if _, ok := p.handing[svc]; !ok {
				p.handing[svc] = now
			}
			// past handoverFor it goes without its final copy
			copyOut := now-p.handing[svc] < handoverFor.Milliseconds()
			err := p.stop(svc, "the plan moved it", copyOut)
			if errors.Is(err, errNotCopied) {
				// stopped, the flag kept: renewed, and asked again next round
				p.send(svc, state.LeaseOp{Role: role, Op: "renew", Epoch: l.Epoch})
				// its flag still says it is handed over: nothing starts
				if h := p.holding(svc); h.Epoch == l.Epoch {
					if u := p.leaseUntil(svc, l); u > h.Until {
						h.Until = u
					}
					content, _ := json.Marshal(h)
					_ = p.raise(svc, content)
				}
				return
			}
			if err != nil {
				return // still running: the lease stays, and lapses if it must
			}
		}
		// given back: the next holder has its data, and this box no
		// longer asks for it first
		p.setHeld(svc, 0)
		p.send(svc, state.LeaseOp{Role: role, Op: "release", Epoch: l.Epoch})
		return
	}
	until := p.leaseUntil(svc, l)
	p.send(svc, state.LeaseOp{Role: role, Op: "renew", Epoch: l.Epoch})
	if _, ok := p.since[svc]; !ok {
		p.since[svc] = now
	}
	if p.up(svc) && p.holding(svc).Epoch != l.Epoch {
		// started under another lease: it starts again under this one
		_ = p.stop(svc, "it runs under an older lease", false)
		return
	}
	if p.up(svc) && p.holding(svc).Handing {
		// stopped mid-handover (by this dd-control or one before it), and
		// the plan gives it back here
		delete(p.handing, svc)
		if err := p.mark(svc, false); err != nil {
			log.Printf("placed: %s: %v", svc, err)
			return
		}
		p.start(svc)
	}
	if _, ok := p.started[svc]; ok && p.up(svc) {
		switch failed, up := p.judge(svc); {
		case failed:
			log.Printf("placed: %s failed to start here under its lease; refusing it, so the plan gives it to a box that can run it", svc)
			p.refuse(svc, now)
			delete(p.started, svc)
		case up:
			delete(p.started, svc)
		}
	}
	if p.up(svc) {
		// the newer of what the flag has and what is now known
		if h := p.holding(svc); h.Until >= until {
			return
		}
	} else if now-p.since[svc] < leaseMargin.Milliseconds() || now >= until {
		return // a holder before it may still be stopping; or no time left
	}
	first := !p.up(svc)
	if first && !p.fits(svc, s) {
		// held and not started: the plan moves it, and it is given back
		return
	}
	content, _ := json.Marshal(held{Epoch: l.Epoch, Until: until})
	if err := p.raise(svc, content); err != nil {
		log.Printf("placed: %s: %v", svc, err)
		return
	}
	if first {
		p.start(svc)
		if b, asked := p.before[svc]; asked && b != p.box {
			if p.started == nil {
				p.started = map[string]int64{}
			}
			p.started[svc] = now
		}
		delete(p.before, svc)
	}
}

// leaseUntil is when this box's writer stops unless a renewal is seen: the
// lease was last set by this box's request l.Seq, and cannot end sooner than
// when that was sent plus the TTL, kept by this box's own clock since boot,
// whatever its wall clock says. A request this box has no record of (it
// restarted since) gives nothing: the flag's own until stands until a
// renewal it sent is agreed.
func (p *placer) leaseUntil(svc string, l *state.Lease) int64 {
	at, ok := p.sent[svc][l.Seq]
	if !ok {
		return 0
	}
	for q := range p.sent[svc] {
		if q < l.Seq {
			delete(p.sent[svc], q)
		}
	}
	return at + p.ttl.Milliseconds() - leaseMargin.Milliseconds()
}

// send asks, and keeps when (since boot) each request went
func (p *placer) send(svc string, op state.LeaseOp) {
	at := p.uptime()
	seq, err := p.ask(op)
	if err != nil {
		// the signer says once when the TPM fails, and when it is back
		if !errors.As(err, new(errSigning)) {
			log.Printf("placed: lease %s %s: %v", op.Op, op.Role, err)
		}
		return
	}
	if p.sent[svc] == nil {
		p.sent[svc] = map[uint64]int64{}
	}
	p.sent[svc][seq] = at
	// what was sent longer ago than a lease lasts can set none any more
	for q, t := range p.sent[svc] {
		if at-t > 2*p.ttl.Milliseconds() {
			delete(p.sent[svc], q)
		}
	}
}

// uptime is ms since boot, suspend included: what /proc/uptime says, so
// dd-placed-fence reads the same clock
func uptime() int64 {
	raw, err := os.ReadFile("/proc/uptime")
	if err != nil {
		return 0
	}
	var secs float64
	if _, err := fmt.Sscan(string(raw), &secs); err != nil {
		return 0
	}
	return int64(secs * 1000)
}
