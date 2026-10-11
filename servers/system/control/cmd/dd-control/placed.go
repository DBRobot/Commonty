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

import (
	"encoding/json"
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sort"
	"time"

	"commonty/control/place"
	"commonty/control/state"
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
	// service -> its units on this box, the services this box carries
	carries map[string][]string
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
}

// what a leased service's flag holds
type held struct {
	Epoch uint64 `json:"epoch"`
	Until int64  `json:"until"` // ms since boot (/proc/uptime): when it stops without a renewal
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

// facts this box reports about its placed services: what it carries, and
// what it runs (a flag up)
func (p *placer) facts() map[string]float64 {
	f := map[string]float64{}
	for svc := range p.carries {
		f["carries/"+svc] = 1
		if p.up(svc) {
			f["runs/"+svc] = 1
		}
	}
	return f
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
	if err := p.run(append([]string{"start", "--no-block"}, p.carries[svc]...)...); err != nil {
		log.Printf("placed: starting %s: %v", svc, err)
	} else {
		log.Printf("placed: %s runs here now", svc)
	}
}

// stop takes the flag down. A writer is stopped before its flag goes, and
// waited for: its lease is given back only once it no longer writes
func (p *placer) stop(svc, why string) error {
	args := []string{"stop", "--no-block"}
	if p.leased[svc] {
		args = []string{"stop"}
	}
	if err := p.run(append(args, p.carries[svc]...)...); err != nil {
		log.Printf("placed: stopping %s: %v", svc, err)
		return err
	}
	_ = os.Remove(p.flag(svc))
	delete(p.since, svc)
	log.Printf("placed: %s no longer runs here (%s)", svc, why)
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
	for _, svc := range p.names() {
		if p.leased[svc] && p.up(svc) && now >= p.holding(svc).Until {
			_ = p.stop(svc, "its lease was not seen renewed")
		}
	}
}

// reconcile makes this box run what the plan gives it of what it carries,
// and nothing else of it. A plan this box is not in (it does not count
// itself alive) changes nothing: it is the plan's view that is behind.
func (p *placer) reconcile(plan place.Plan, s *state.State) {
	mine, in := plan.Boxes[p.box]
	if !in {
		return
	}
	want := map[string]bool{}
	for _, d := range mine {
		want[d.Service] = true
	}
	for _, svc := range p.names() {
		if p.leased[svc] {
			p.reconcileLeased(svc, want[svc], s)
			continue
		}
		switch {
		case want[svc] && !p.up(svc):
			if err := p.raise(svc, nil); err != nil {
				log.Printf("placed: %s: %v", svc, err)
				continue
			}
			p.start(svc)
		case !want[svc] && p.up(svc):
			_ = p.stop(svc, "the plan moved it")
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
			_ = p.stop(svc, "its lease is not this box's")
		}
		if want {
			p.send(svc, state.LeaseOp{Role: leaseOf(svc), Op: "acquire"})
		}
		return
	}
	if !want {
		if p.up(svc) && p.stop(svc, "the plan moved it") != nil {
			return // still running: the lease stays, and lapses if it must
		}
		p.send(svc, state.LeaseOp{Role: leaseOf(svc), Op: "release", Epoch: l.Epoch})
		return
	}
	// The lease was last set by this box's request l.Seq, and cannot end
	// sooner than when that was sent plus the TTL: kept by this box's own
	// clock since boot, whatever its wall clock says. A request this box
	// has no record of (it restarted since) gives nothing: the flag's own
	// until stands until a renewal it sent is agreed.
	var until int64
	if at, ok := p.sent[svc][l.Seq]; ok {
		until = at + p.ttl.Milliseconds() - leaseMargin.Milliseconds()
		for q := range p.sent[svc] {
			if q < l.Seq {
				delete(p.sent[svc], q)
			}
		}
	}
	p.send(svc, state.LeaseOp{Role: leaseOf(svc), Op: "renew", Epoch: l.Epoch})
	if _, ok := p.since[svc]; !ok {
		p.since[svc] = now
	}
	if p.up(svc) && p.holding(svc).Epoch != l.Epoch {
		// started under another lease: it starts again under this one
		_ = p.stop(svc, "it runs under an older lease")
		return
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
	content, _ := json.Marshal(held{Epoch: l.Epoch, Until: until})
	if err := p.raise(svc, content); err != nil {
		log.Printf("placed: %s: %v", svc, err)
		return
	}
	if first {
		p.start(svc)
	}
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
