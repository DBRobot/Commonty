package state

// The state machine's version, and the config in force: what validators
// on two releases must agree on before either changes. A validator says
// which version it runs and which config it has (validator/stand.go); the
// agreed state takes a new version and config only in a block every
// validator voted for after saying so, and every live box has said it
// runs that version too. Until then every validator, old or new, applies
// blocks as the version before did and serializes the state the same way,
// so its hash, which each proposal names, is the same on both
// (docs/dev/control-plane.md, "Changing the state machine").

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"sort"
	"sync"

	"commonty/control/state/ident"
)

// Machine is the state machine this code runs. 1 is release 359's (no
// version, no agreed config); 2 adds the agreed config and rollout requests.
// Raise it with every change to what Check, Apply, Block or the state's
// format does, and gate the change on s.Machine.
const Machine = 2

// MachineFact is the heartbeat fact a box says its version in. A box
// that says none runs 1.
const MachineFact = "machine"

// Version is the agreed state machine: 1 until a version is agreed
func (s *State) Version() int { return max(s.Machine, 1) }

// Canonical is the config as validators agree on it: without Before, with
// keys sorted (ident.CanonicalValue), and its sha256
func (c *Config) Canonical() (json.RawMessage, string) {
	x := *c
	x.Before = nil
	raw, _ := json.Marshal(&x)
	b, err := ident.CanonicalValue(raw)
	if err != nil {
		b = raw
	}
	h := sha256.Sum256(b)
	return b, hex.EncodeToString(h[:])
}

// ConfigDigest names the agreed config ("" while none is)
func (s *State) ConfigDigest() string {
	if len(s.Config) == 0 {
		return ""
	}
	h := sha256.Sum256(s.Config)
	return hex.EncodeToString(h[:])
}

var (
	agreedMu sync.Mutex
	// one *Config per agreed config: the computer-lease replay is kept per
	// *Config (computerlease.go)
	agreed = map[string]*Config{}
)

// In is the config a block applies under: the agreed one, once there is
// one; before that, what release 359 ran (own.Before, the config the
// release before this one shipped), or own itself when it says none
func (s *State) In(own *Config) *Config {
	if len(s.Config) == 0 {
		if own.Before != nil {
			return own.Before
		}
		return own
	}
	d := s.ConfigDigest()
	agreedMu.Lock()
	defer agreedMu.Unlock()
	if c := agreed[d]; c != nil {
		return c
	}
	c := &Config{}
	if json.Unmarshal(s.Config, c) != nil {
		// agreed, so every validator read it; not readable here means this
		// code is older than the config, which a stand never lets happen
		return own
	}
	if len(agreed) > 16 {
		clear(agreed)
	}
	agreed[d] = c
	return c
}

// Behind: the live boxes whose heartbeat does not say they run at least
// version m, at now, under the config in force (cur). A box's own word
// can hold a version back only while it may still act under the next
// config too: a box that config drops, or gives another key, never counts.
// The validators bound how long any box counts at all (validator/stand.go)
func (s *State) Behind(cur, next *Config, m int, now int64) []string {
	var out []string
	for name, b := range s.Boxes {
		if !s.Alive(cur, name, now) {
			continue
		}
		k1, ok1 := s.BoxKey(cur, name)
		k2, ok2 := s.BoxKey(next, name)
		if !ok1 || !ok2 || k1 != k2 {
			continue
		}
		if max(int(b.Facts[MachineFact]), 1) < m {
			out = append(out, name)
		}
	}
	sort.Strings(out)
	return out
}

// SameKey: r is signed under one key by both configs (a join never is):
// in the block that changes the config, a request counts only if it
// would under the config before and the one after, so a key one of them
// drops never acts across the change
func (s *State) SameKey(a, b *Config, r Request) bool {
	if r.Kind == "join" {
		return false
	}
	k1, ok1 := s.BoxKey(a, r.Box)
	k2, ok2 := s.BoxKey(b, r.Box)
	return ok1 && ok2 && k1 == k2
}
