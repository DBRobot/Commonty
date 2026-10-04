// Package state is the control plane's state machine: what every validator
// agrees on, and the rules a request must pass to change it. It is
// deterministic - the same requests in the same order and the same agreed
// times give the same state on every validator - and it never reads a clock
// of its own: time comes from the batch (the proposer's stamp, checked by
// the others within a tolerance).
package state

import (
	"encoding/json"
	"errors"
	"fmt"
	"sort"
)

// Config is the signed part: who is who and the rules, from the release.
type Config struct {
	// box name -> its house and its TPM-held key (SPKI, base64)
	Boxes map[string]Box `json:"boxes"`
	Rules Rules          `json:"rules"`
}

type Box struct {
	House string `json:"house"`
	Key   string `json:"key"`
}

type Rules struct {
	LeaseTTL  int64 `json:"leaseTtlMs"`  // a lease lasts this long past its grant or renewal
	GoneAfter int64 `json:"goneAfterMs"` // a box unheard this long is not alive
	// role -> who may hold its writer lease; empty means any live box
	Leases map[string]LeaseRule `json:"leases"`
}

type LeaseRule struct {
	Eligible []string `json:"eligible,omitempty"`
}

// Request is what a box asks for, signed by its TPM-held key over Bytes().
type Request struct {
	Kind string          `json:"kind"` // heartbeat | lease
	Box  string          `json:"box"`
	Seq  uint64          `json:"seq"` // strictly increasing per box
	Body json.RawMessage `json:"body"`
	Sig  string          `json:"sig,omitempty"`
}

type Heartbeat struct {
	Facts map[string]float64 `json:"facts"` // cores, ram_bytes, disk_free_bytes, load, temp_c, ...
	Roles []string           `json:"roles"` // what it runs now
}

type LeaseOp struct {
	Role  string `json:"role"`
	Op    string `json:"op"`              // acquire | renew | release
	Epoch uint64 `json:"epoch,omitempty"` // renew and release name the lease they hold
}

// Bytes is what the box signs: everything but the signature, prefixed so
// the key's signature means nothing anywhere else.
func (r Request) Bytes() []byte {
	r.Sig = ""
	b, _ := json.Marshal(r)
	return append([]byte("commonty control v1\x00"), b...)
}

type BoxState struct {
	Seq      uint64             `json:"seq"`
	LastSeen int64              `json:"lastSeen"`
	Facts    map[string]float64 `json:"facts,omitempty"`
	Roles    []string           `json:"roles,omitempty"`
}

type Lease struct {
	Holder  string `json:"holder"`
	Epoch   uint64 `json:"epoch"`
	Expires int64  `json:"expires"`
}

type State struct {
	Height uint64               `json:"height"`
	Time   int64                `json:"time"` // agreed, ms
	Boxes  map[string]*BoxState `json:"boxes"`
	Leases map[string]*Lease    `json:"leases"`
	// the highest epoch ever granted per role, even after a lease is gone:
	// a new holder always gets a higher one
	Epochs map[string]uint64 `json:"epochs"`
}

func New() *State {
	return &State{Boxes: map[string]*BoxState{}, Leases: map[string]*Lease{}, Epochs: map[string]uint64{}}
}

// Verify checks a signature; the validator supplies it (ECDSA P-256 here).
type Verify func(spki string, msg []byte, sig string) bool

var (
	ErrUnknownBox = errors.New("not a box of this fleet")
	ErrBadSig     = errors.New("not signed by that box")
	ErrStale      = errors.New("sequence not above the last")
)

// Check says whether r may be applied at agreed time now. It changes nothing.
func (s *State) Check(cfg *Config, verify Verify, r Request, now int64) error {
	box, ok := cfg.Boxes[r.Box]
	if !ok {
		return ErrUnknownBox
	}
	if !verify(box.Key, r.Bytes(), r.Sig) {
		return ErrBadSig
	}
	if b := s.Boxes[r.Box]; b != nil && r.Seq <= b.Seq {
		return ErrStale
	}
	switch r.Kind {
	case "heartbeat":
		var h Heartbeat
		return json.Unmarshal(r.Body, &h)
	case "lease":
		var op LeaseOp
		if err := json.Unmarshal(r.Body, &op); err != nil {
			return err
		}
		return s.checkLease(cfg, r.Box, op, now)
	}
	return fmt.Errorf("no such request: %q", r.Kind)
}

func (s *State) checkLease(cfg *Config, box string, op LeaseOp, now int64) error {
	rule, ok := cfg.Rules.Leases[op.Role]
	if !ok {
		return fmt.Errorf("no lease called %q", op.Role)
	}
	cur := s.Leases[op.Role]
	held := cur != nil && cur.Expires > now
	switch op.Op {
	case "acquire":
		if held && cur.Holder != box {
			return fmt.Errorf("%s is held by %s until %d", op.Role, cur.Holder, cur.Expires)
		}
		if len(rule.Eligible) > 0 && !contains(rule.Eligible, box) {
			return fmt.Errorf("%s may not hold %s", box, op.Role)
		}
		if !s.Alive(cfg, box, now) {
			return fmt.Errorf("%s is not alive", box)
		}
		return nil
	case "renew", "release":
		if !held || cur.Holder != box || cur.Epoch != op.Epoch {
			return fmt.Errorf("%s does not hold %s at epoch %d", box, op.Role, op.Epoch)
		}
		return nil
	}
	return fmt.Errorf("no such lease operation: %q", op.Op)
}

// Apply makes r's change; call only after Check passed at the same now.
func (s *State) Apply(cfg *Config, r Request, now int64) {
	b := s.Boxes[r.Box]
	if b == nil {
		b = &BoxState{}
		s.Boxes[r.Box] = b
	}
	b.Seq = r.Seq
	b.LastSeen = now
	switch r.Kind {
	case "heartbeat":
		var h Heartbeat
		_ = json.Unmarshal(r.Body, &h)
		b.Facts = h.Facts
		b.Roles = append([]string(nil), h.Roles...)
		sort.Strings(b.Roles)
	case "lease":
		var op LeaseOp
		_ = json.Unmarshal(r.Body, &op)
		cur := s.Leases[op.Role]
		switch op.Op {
		case "acquire":
			if cur != nil && cur.Expires > now && cur.Holder == r.Box {
				cur.Expires = now + cfg.Rules.LeaseTTL // asking again while holding is renewing
				return
			}
			s.Epochs[op.Role]++
			s.Leases[op.Role] = &Lease{Holder: r.Box, Epoch: s.Epochs[op.Role], Expires: now + cfg.Rules.LeaseTTL}
		case "renew":
			cur.Expires = now + cfg.Rules.LeaseTTL
		case "release":
			delete(s.Leases, op.Role)
		}
	}
}

// Block applies a batch in order at its agreed time; requests that fail
// their check (another validator's view may differ until the batch is
// agreed) are skipped, the same way on every validator.
func (s *State) Block(cfg *Config, verify Verify, reqs []Request, now int64) (applied int) {
	if now > s.Time {
		s.Time = now
	}
	s.Height++
	for l, lease := range s.Leases {
		if lease.Expires <= s.Time {
			delete(s.Leases, l)
		}
	}
	for _, r := range reqs {
		if s.Check(cfg, verify, r, s.Time) == nil {
			s.Apply(cfg, r, s.Time)
			applied++
		}
	}
	return applied
}

// Alive: heard from within GoneAfter of now.
func (s *State) Alive(cfg *Config, box string, now int64) bool {
	b := s.Boxes[box]
	return b != nil && now-b.LastSeen <= cfg.Rules.GoneAfter
}

func contains(xs []string, x string) bool {
	for _, y := range xs {
		if y == x {
			return true
		}
	}
	return false
}
