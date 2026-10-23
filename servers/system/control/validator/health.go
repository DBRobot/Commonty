package validator

import (
	"fmt"
	"sync"
	"time"

	"commonty/control/wire"
)

// pulse: what the validator has seen of its own progress and its peers',
// for the watchdog. Every method is safe on the zero value.
type pulse struct {
	mu       sync.Mutex
	start    time.Time
	height   uint64
	progress time.Time // the height last moved, or the start
	waiting  time.Time // the first request since then came in; zero: none
	// since when the call into consensus in hand, and the delivery of a
	// decision, have run; zero when none is
	calling, delivering time.Time
	peers               map[uint64]seen
	dropped, said       uint64
	peerDropped         uint64
	saidAt              time.Time
}

type seen struct {
	height uint64
	at     time.Time
}

func (p *pulse) init(now time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.start, p.progress = now, now
}

func (p *pulse) started(now time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.start = now
	if p.progress.Before(now) {
		p.progress = now
	}
}

func (p *pulse) request(now time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.waiting.IsZero() {
		p.waiting = now
	}
}

// idle: no request is waiting yet, so the next one is worth checking
func (p *pulse) idle() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.waiting.IsZero()
}

func (p *pulse) enter(at *time.Time, now time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	*at = now
}

func (p *pulse) leave(at *time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	*at = time.Time{}
}

func (p *pulse) moved(height uint64, now time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if height > p.height {
		p.height, p.progress, p.waiting = height, now, time.Time{}
	}
}

func (p *pulse) peer(id, height uint64, now time.Time) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.peers == nil {
		p.peers = map[uint64]seen{}
	}
	p.peers[id] = seen{height, now}
}

// drop counts a dropped frame, and returns how many to say were dropped:
// at most once every 10 s
func (p *pulse) drop(now time.Time, peer bool) uint64 {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.dropped++
	if peer {
		p.peerDropped++
	}
	if now.Sub(p.saidAt) < 10*time.Second {
		return 0
	}
	n := p.dropped - p.said
	p.said, p.saidAt = p.dropped, now
	return n
}

// Health is a validator's own progress as it stands.
type Health struct {
	Started  time.Time
	Height   uint64
	Progress time.Time // the height last moved (or the start)
	Waiting  time.Time // the first request since then came in; zero: none
	// the call into consensus in hand, and a decision being delivered:
	// since when, zero when none is
	Calling, Delivering time.Time
	// the highest height a quorum signed, of peers heard within Fresh, and
	// how many peers were heard within Fresh at all
	PeerHeight uint64
	PeersHeard int
	// frames dropped from a full queue, and of them the validators' votes
	Dropped, PeerDropped uint64
}

func (v *Validator) Health() Health {
	p := &v.pulse
	p.mu.Lock()
	defer p.mu.Unlock()
	h := Health{Started: p.start, Height: p.height, Progress: p.progress, Waiting: p.waiting,
		Calling: p.calling, Delivering: p.delivering, Dropped: p.dropped, PeerDropped: p.peerDropped}
	now := v.o.Now()
	for _, s := range p.peers {
		if now.Sub(s.at) <= Fresh {
			h.PeersHeard++
			h.PeerHeight = max(h.PeerHeight, s.height)
		}
	}
	return h
}

// Limits: how long each sign of a wedge must last before it counts.
type Limits struct {
	// one call into consensus, or the delivery of one decision, not back
	Hung time.Duration
	// a peer's quorum-signed height above ours, and ours not moving
	Behind time.Duration
	// verified requests waiting, peers answering, and no height moving
	Stalled time.Duration
	// after a start, nothing counts: the first catch-up takes a moment
	Grace time.Duration
	// after a self-heal, nothing counts for this long, whatever the sign:
	// whoever can make it look wedged can't make it restart over and over
	Quiet time.Duration
	// the validators take turns: one may restart itself only in its own
	// slot of this length, so no two restart at once
	Slot time.Duration
}

var DefaultLimits = Limits{Hung: 30 * time.Second, Behind: 60 * time.Second, Stalled: 120 * time.Second,
	Grace: 60 * time.Second, Quiet: 10 * time.Minute, Slot: 30 * time.Second}

// how recently a peer must have answered to count as heard
const Fresh = 30 * time.Second

// Who: this validator among the others. Turn is its place in the order of
// the validators' ids, Of how many there are.
type Who struct {
	Quorum, Turn, Of int
}

// Diagnose: why this validator is wedged and should restart now, or ""
// while it is not or must not. lastHeal is the last time it restarted
// itself (zero: never). It never restarts while no peer it heard from is
// as far as it is: then it may be the only one up to date.
func Diagnose(h Health, now, lastHeal time.Time, w Who, l Limits) string {
	if h.Started.IsZero() || now.Sub(h.Started) < l.Grace {
		return ""
	}
	if !lastHeal.IsZero() && now.Sub(lastHeal) < l.Quiet {
		return ""
	}
	if w.Of > 1 && l.Slot > 0 && int(now.UnixNano()/int64(l.Slot)%int64(w.Of)) != w.Turn {
		return ""
	}
	if h.PeersHeard == 0 || h.PeerHeight < h.Height {
		return ""
	}
	if !h.Calling.IsZero() && now.Sub(h.Calling) >= l.Hung {
		return fmt.Sprintf("a call into consensus has not returned for %s", now.Sub(h.Calling).Round(time.Second))
	}
	if !h.Delivering.IsZero() && now.Sub(h.Delivering) >= l.Hung {
		return fmt.Sprintf("delivering decision %d has not finished for %s", h.Height+1, now.Sub(h.Delivering).Round(time.Second))
	}
	still := now.Sub(h.Progress)
	if h.PeerHeight > h.Height && still >= l.Behind {
		return fmt.Sprintf("peers are at height %d, signed by a quorum, and this validator has been at %d for %s", h.PeerHeight, h.Height, still.Round(time.Second))
	}
	if !h.Waiting.IsZero() && now.Sub(h.Waiting) >= l.Stalled && h.PeersHeard >= w.Quorum-1 {
		return fmt.Sprintf("signed requests are waiting, %d peers answer, and no height has moved past %d for %s", h.PeersHeard, h.Height, still.Round(time.Second))
	}
	return ""
}

// Quorum: how many validators a decision needs.
func (v *Validator) Quorum() int { return v.quorum() }

// who: this validator's place among them, by id
func (v *Validator) who() Who {
	w := Who{Quorum: v.quorum(), Of: len(v.o.Config.Validators)}
	for _, val := range v.o.Config.Validators {
		if val.ID < v.o.ID {
			w.Turn++
		}
	}
	return w
}

// Watch checks every `every` whether this validator is wedged, and calls
// wedged with the reason once it is. While a sign of a wedge stands (a
// call or a delivery not back, or a verified request waiting undecided) it
// asks the peers for their latest, every `every`, so it knows where they
// are: nothing is asked while decisions come.
func (v *Validator) Watch(l Limits, every time.Duration, lastHeal time.Time, wedged func(reason string)) {
	t := time.NewTicker(every)
	defer t.Stop()
	since := func(at, now time.Time) bool { return !at.IsZero() && now.Sub(at) >= every*2 }
	for {
		select {
		case <-v.done:
			return
		case <-t.C:
		}
		now := v.o.Now()
		h := v.Health()
		if since(h.Waiting, now) || since(h.Calling, now) || since(h.Delivering, now) {
			v.o.Transport.Send(wire.ToValidators, wire.SyncAsk, nil)
		}
		if why := Diagnose(h, now, lastHeal, v.who(), l); why != "" {
			wedged(why)
			return
		}
	}
}

// limiter: how many requests a sender may hand in. Validators and the
// fleet's boxes have a bucket each, boxes still joining share one, and
// every sender but the validators shares a cap besides; a sender that is
// none of these gets no bucket and nothing in
type limiter struct {
	mu        sync.Mutex
	buckets   map[uint64]*bucket
	strangers bucket
	all       bucket
}

type bucket struct {
	tokens float64
	at     time.Time
}

type sender int

const (
	unknown sender = iota
	aValidator
	aBox
	aStranger
)

// a box sends a heartbeat every 30 s, a lease renewal every 5 s, at most
// 60 directory items and 20 build results a minute: far under this. Joins
// are a few an hour (state.JoinsPerHour)
const (
	perSecond = 20
	burst     = 200
	senders   = 4096
	// every box together, and every box still joining together
	allPerSecond      = 500
	allBurst          = 2000
	strangerPerSecond = 2
	strangerBurst     = 20
)

func (b *bucket) take(rate, size float64, now time.Time) bool {
	if b.at.IsZero() {
		b.tokens = size
	} else {
		b.tokens = min(size, b.tokens+now.Sub(b.at).Seconds()*rate)
	}
	b.at = now
	if b.tokens < 1 {
		return false
	}
	b.tokens--
	return true
}

func (l *limiter) allow(from uint64, who sender, now time.Time) bool {
	l.mu.Lock()
	defer l.mu.Unlock()
	switch who {
	case unknown:
		return false
	case aStranger:
		return l.strangers.take(strangerPerSecond, strangerBurst, now) && l.all.take(allPerSecond, allBurst, now)
	}
	if l.buckets == nil {
		l.buckets = map[uint64]*bucket{}
	}
	b := l.buckets[from]
	if b == nil {
		if len(l.buckets) >= senders {
			l.evict(now)
		}
		b = &bucket{}
		l.buckets[from] = b
	}
	// its own bucket first: a sender past its rate spends none of the cap
	return b.take(perSecond, burst, now) && (who == aValidator || l.all.take(allPerSecond, allBurst, now))
}

// evict drops the buckets that have refilled, or else the fullest: never
// every bucket at once, which handed a flooding sender a full one again,
// nor one that is spent while another has more left
func (l *limiter) evict(now time.Time) {
	var fullest uint64
	most := -1.0
	for id, b := range l.buckets {
		left := b.tokens + now.Sub(b.at).Seconds()*perSecond
		if left >= burst {
			delete(l.buckets, id)
		} else if left > most {
			fullest, most = id, left
		}
	}
	if len(l.buckets) >= senders {
		delete(l.buckets, fullest)
	}
}
