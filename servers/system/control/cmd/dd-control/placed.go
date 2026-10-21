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
}

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
		p.reconcileLeased(svc, want[svc] && ok[svc], s)
	}
	for _, svc := range p.names() {
		if p.leased[svc] {
			p.reconcileLeased(svc, want[svc], s)
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

func (p *placer) reconcileLeased(svc string, want bool, s *state.State) {
	l := s.Leases[leaseOf(svc)]
	mine := l != nil && l.Holder == p.box && l.Expires > s.Time
	now := p.uptime()
	if !mine {
		delete(p.since, svc)
		if p.up(svc) {
			// agreed and fresh: someone else's, or nobody's
			_ = p.stop(svc, "its lease is not this box's", false)
		}
		if want && p.fits(svc, s) {
			p.send(svc, state.LeaseOp{Role: leaseOf(svc), Op: "acquire"})
		}
		return
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
				p.send(svc, state.LeaseOp{Role: leaseOf(svc), Op: "renew", Epoch: l.Epoch})
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
		p.send(svc, state.LeaseOp{Role: leaseOf(svc), Op: "release", Epoch: l.Epoch})
		return
	}
	until := p.leaseUntil(svc, l)
	p.send(svc, state.LeaseOp{Role: leaseOf(svc), Op: "renew", Epoch: l.Epoch})
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
		log.Printf("placed: lease %s %s: %v", op.Op, op.Role, err)
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
