package validator

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"errors"
	"sync"

	"commonty/control/state"
	"commonty/control/wire"
)

// Reader is a box that does not vote: it asks the validators for their
// latest, and keeps what holds together - a decision signed by a quorum and
// the state that decision names, applied. No validator is believed alone.
type Reader struct {
	cfg  *state.Config
	pubs map[uint64]ed25519.PublicKey
	t    Transport
	mu   sync.Mutex
	st   *state.State
}

func NewReader(cfg *state.Config, t Transport) (*Reader, error) {
	r := &Reader{cfg: cfg, t: t, pubs: map[uint64]ed25519.PublicKey{}, st: state.New()}
	for _, val := range cfg.Validators {
		k, err := base64.StdEncoding.DecodeString(val.Key)
		if err != nil || len(k) != ed25519.PublicKeySize {
			return nil, errors.New("a validator's key is not ed25519")
		}
		r.pubs[val.ID] = k
	}
	return r, nil
}

// Ask sends "send me your latest" to the validators.
func (r *Reader) Ask() { r.t.Send(wire.ToValidators, wire.SyncAsk, nil) }

// Receive takes answers; anything else is not for a reader.
func (r *Reader) Receive(_ uint64, kind byte, payload []byte) {
	if kind != wire.SyncState {
		return
	}
	var s Saved
	if json.Unmarshal(payload, &s) != nil || len(s.Proposal.Header) == 0 {
		return
	}
	st, err := Settle(r.cfg, r.pubs, s)
	if err != nil {
		return
	}
	r.mu.Lock()
	defer r.mu.Unlock()
	if st.Height > r.st.Height {
		r.st = st
	}
}

func (r *Reader) State() *state.State {
	r.mu.Lock()
	defer r.mu.Unlock()
	return clone(r.st)
}

// Settle checks a saved state and decision against the validators' keys and
// returns the state after that decision.
func Settle(cfg *state.Config, pubs map[uint64]ed25519.PublicKey, s Saved) (*state.State, error) {
	v := &Validator{o: Options{Config: cfg}, pubs: pubs, st: state.New()}
	if err := v.adopt(s); err != nil {
		return nil, err
	}
	return v.st, nil
}
