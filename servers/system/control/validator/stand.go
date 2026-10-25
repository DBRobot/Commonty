package validator

// What each validator runs, said to the others, and the block that makes
// it agreed. Every validator sends its stand to the others every few
// seconds: its state machine version, the digest of its config, when it
// first stood for that config, and its clock, signed. A proposer that
// holds a fresh stand from every live validator, all for its own config,
// puts them in its proposal's header with that config; the block sets the
// agreed version and config before its requests are applied. A validator
// of release 359 never sends a stand and never sees such a header, so
// nothing new is agreed while one still votes (docs/dev/control-plane.md,
// "Changing the state machine").
//
// A validator that is off sends nothing, and is left out: one that every
// voter has heard nothing from for absentAfter, while it delivered
// absentDecisions decisions, takes no part in deciding, whatever release it
// runs. Each voter judges that by what it has heard itself, and a quorum
// of stands is needed all the same.
//
// The validators decide it. A box's word counts only for a while: a live
// box that says it runs an older version holds a version back for at most
// boxGrace after the last validator stood for it, and a box the new config
// drops or rekeys never holds anything back.

import (
	"bytes"
	"crypto/ed25519"
	"encoding/json"
	"errors"
	"fmt"
	"sync/atomic"
	"time"

	"commonty/control/state"
	"commonty/control/state/ident"
	"commonty/control/wire"
)

const (
	// how often a validator says what it runs
	standEvery = 5 * time.Second
	// how old a stand may be in the block that agrees it, ms
	standFresh = 60_000
	// how long, after the last validator stood for a config, a live box on
	// an older version holds it back, ms
	boxGrace = 30 * 60_000
	// a validator heard from by nothing for this long, while this one
	// delivered absentDecisions decisions, is off: a live one says
	// something in every decision (its prepare and commit) and every
	// standEvery (its stand)
	absentAfter     = 20 * time.Second
	absentDecisions = 10
)

type Stand struct {
	ID      uint64 `json:"id"`
	Machine int    `json:"m"`
	Config  string `json:"c"` // Config.Canonical's digest
	Since   int64  `json:"f"` // its clock when it first stood for Config, ms
	Time    int64  `json:"t"` // its clock, ms
	Sig     []byte `json:"s"`
}

func (s Stand) bytes() []byte {
	return fmt.Appendf(nil, "commonty control stand v1\x00%d\x00%d\x00%s\x00%d\x00%d", s.ID, s.Machine, s.Config, s.Since, s.Time)
}

// activation: a version and config every validator stood for
type activation struct {
	Machine int             `json:"m"`
	Config  json.RawMessage `json:"c"`
	Stands  []Stand         `json:"s"`
}

func (v *Validator) machine() int {
	if v.o.Machine > 0 {
		return v.o.Machine
	}
	return state.Machine
}

func (v *Validator) stand(since int64) Stand {
	_, d := v.o.Config.Canonical()
	s := Stand{ID: v.o.ID, Machine: v.machine(), Config: d, Since: since, Time: v.o.Now().UnixMilli()}
	s.Sig = ed25519.Sign(v.o.Key, s.bytes())
	return s
}

// say sends this validator's stand now and every standEvery; a validator
// of version 1 (tests: Options.Machine) says nothing, as release 359's.
// Its own newest stand is what it proposes with; the one it keeps, which
// a proposal must carry or better, is the one before: by then it has
// reached the others, so a proposer that heard it is never refused for a
// stand still on its way
func (v *Validator) say() {
	if v.machine() < 2 {
		return
	}
	since := v.o.Now().UnixMilli()
	t := time.NewTicker(standEvery)
	defer t.Stop()
	for {
		s := v.stand(since)
		v.standsMu.Lock()
		if v.mine.Sig != nil {
			v.stands[v.o.ID] = v.mine
		}
		v.mine = s
		v.standsMu.Unlock()
		b, _ := json.Marshal(s)
		v.o.Transport.Send(wire.ToValidators, wire.Stand, b)
		select {
		case <-v.done:
			return
		case <-t.C:
		}
	}
}

// heardStand keeps another validator's newest stand that it signed
func (v *Validator) heardStand(s Stand) {
	pub, ok := v.pubs[s.ID]
	if !ok || s.ID == v.o.ID || !ed25519.Verify(pub, s.bytes(), s.Sig) {
		return
	}
	v.standsMu.Lock()
	defer v.standsMu.Unlock()
	if old, ok := v.stands[s.ID]; !ok || s.Time > old.Time {
		v.stands[s.ID] = s
	}
}

// activate: what a proposal at agreed time t on st makes agreed, if
// anything; caller holds v.mu
func (v *Validator) activate(st *state.State, t int64) *activation {
	if v.machine() < 2 {
		return nil
	}
	a := &activation{Machine: v.machine()}
	now := v.o.Now()
	v.standsMu.Lock()
	for _, val := range v.o.Config.Validators {
		s, ok := v.stands[val.ID]
		if val.ID == v.o.ID {
			s, ok = v.mine, v.mine.Sig != nil
		}
		if !ok {
			if v.absent(val.ID, now) {
				continue
			}
			v.standsMu.Unlock()
			return nil
		}
		a.Stands = append(a.Stands, s)
		a.Machine = min(a.Machine, s.Machine)
	}
	v.standsMu.Unlock()
	if len(a.Stands) < v.quorum() {
		return nil
	}
	a.Config, _ = v.o.Config.Canonical()
	if a.check(v, st, t) != nil {
		return nil
	}
	return a
}

// check, by the validator about to vote: the config is its own; every
// validator it has not found off (absent) stood for at least a.Machine
// with it, fresh at t and after the block that agreed the config in force,
// its own stand no older than the one it keeps, and those are a quorum;
// this code runs a.Machine; and no box may still hold it back
func (a *activation) check(v *Validator, st *state.State, t int64) error {
	if a.Machine < 2 || a.Machine < st.Version() {
		return fmt.Errorf("version %d after %d", a.Machine, st.Version())
	}
	if a.Machine > v.machine() {
		return fmt.Errorf("version %d; this validator runs %d", a.Machine, v.machine())
	}
	canon, err := ident.CanonicalValue(a.Config)
	if err != nil || !bytes.Equal(canon, a.Config) {
		return errors.New("the config is not canonical")
	}
	d := (&state.State{Config: a.Config}).ConfigDigest()
	// only ever the config this validator runs: never one it left behind
	if _, own := v.o.Config.Canonical(); d != own {
		return fmt.Errorf("config %.12s is not this validator's %.12s", d, own)
	}
	if a.Machine == st.Version() && d == st.ConfigDigest() {
		return errors.New("nothing new")
	}
	v.standsMu.Lock()
	kept, hasKept := v.stands[v.o.ID]
	v.standsMu.Unlock()
	seen := map[uint64]bool{}
	var lastSince int64
	for _, s := range a.Stands {
		pub, ok := v.pubs[s.ID]
		if !ok || seen[s.ID] || !ed25519.Verify(pub, s.bytes(), s.Sig) {
			return fmt.Errorf("a stand from %d", s.ID)
		}
		seen[s.ID] = true
		if s.Machine < a.Machine || s.Config != d {
			return fmt.Errorf("validator %d stands for version %d and config %.12s", s.ID, s.Machine, s.Config)
		}
		if s.Time < t-standFresh || s.Time > t+ClockTolerance {
			return fmt.Errorf("validator %d's stand is not fresh", s.ID)
		}
		// a stand from before the config in force was agreed speaks of
		// then, not now
		if st.ConfigTime > 0 && s.Time <= st.ConfigTime {
			return fmt.Errorf("validator %d's stand is from before the config in force", s.ID)
		}
		if s.ID == v.o.ID && hasKept && s.Time < kept.Time {
			return errors.New("this validator's own stand is older than its latest")
		}
		if s.Since > s.Time {
			return fmt.Errorf("validator %d stood for it after it said so", s.ID)
		}
		lastSince = max(lastSince, s.Since)
	}
	now := v.o.Now()
	for _, val := range v.o.Config.Validators {
		if !seen[val.ID] && !v.absent(val.ID, now) {
			return fmt.Errorf("no stand from validator %d, which is not off", val.ID)
		}
	}
	if len(seen) < v.quorum() {
		return fmt.Errorf("stands from %d validators; a quorum is %d", len(seen), v.quorum())
	}
	if t < lastSince+boxGrace {
		if behind := st.Behind(st.In(v.o.Config), v.o.Config, a.Machine, t); len(behind) > 0 {
			return fmt.Errorf("%v run a version below %d", behind, a.Machine)
		}
	}
	return nil
}

type heardOf struct {
	at        atomic.Int64 // unix ms; 0: not since this one started
	decisions atomic.Uint64
}

// heardFrom: a frame from another validator, of any kind: from the relay's
// reader, and a vote as consensus takes it
func (v *Validator) heardFrom(id uint64) {
	h := v.lastHeard[id]
	if h == nil || id == v.o.ID {
		return
	}
	h.decisions.Store(v.decisions.Load())
	h.at.Store(v.o.Now().UnixMilli())
}

// delivered: one more decision this validator delivered
func (v *Validator) delivered() { v.decisions.Add(1) }

// absent: validator id is off, as far as this one knows. Nothing came from
// it for absentAfter since it was last heard or this one started, and this
// one delivered absentDecisions decisions in that time: a decision took a
// quorum's votes, and a live validator votes in each and stands every few
// seconds, so one heard from in none of them is not deciding. A validator
// is never absent to itself
func (v *Validator) absent(id uint64, now time.Time) bool {
	h := v.lastHeard[id]
	if h == nil || id == v.o.ID {
		return false
	}
	since := v.pulse.since().UnixMilli()
	if at := h.at.Load(); at > since {
		since = at
	}
	return now.UnixMilli()-since >= absentAfter.Milliseconds() && v.decisions.Load()-h.decisions.Load() >= absentDecisions
}
