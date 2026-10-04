// Package validator is one voter: SmartBFT orders the boxes' signed
// requests, and every agreed batch is applied to the state machine
// (package state) the same way on every validator. The same code runs as a
// process on a box and as wasm in the relay's Durable Object; what differs
// is the transport, the write-ahead log and where the state is kept.
package validator

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"sync"
	"time"

	"commonty/control/state"
	"commonty/control/wire"

	"github.com/hyperledger/SmartBFT/pkg/api"
	"github.com/hyperledger/SmartBFT/pkg/consensus"
	"github.com/hyperledger/SmartBFT/pkg/metrics/disabled"
	bft "github.com/hyperledger/SmartBFT/pkg/types"
	"github.com/hyperledger/SmartBFT/smartbftprotos"
	"google.golang.org/protobuf/proto"
)

// how far a proposer's clock may be from ours
const ClockTolerance = 2000 // ms

type Transport interface {
	Send(to uint64, kind byte, payload []byte)
}

type Options struct {
	Config    *state.Config
	ID        uint64
	Key       ed25519.PrivateKey
	Transport Transport
	Logger    api.Logger
	WAL       api.WriteAheadLog
	// a saved state and the decision that led to it, from a previous run
	Saved *Saved
	// called after every decision with the new state, for a box to keep
	Persist func(Saved)
	Now     func() time.Time
}

// Saved is what a validator keeps across restarts, and what it hands a
// node that asks to catch up: the state before the last decision, and that
// decision with its signatures. The decision names the earlier state's
// hash, so a node can check the state against signatures it trusts.
type Saved struct {
	Before   json.RawMessage `json:"before"`
	Proposal bft.Proposal    `json:"proposal"`
	Sigs     []bft.Signature `json:"sigs"`
}

type header struct {
	Height uint64 `json:"h"`
	Time   int64  `json:"t"` // the agreed clock, ms
	Before string `json:"b"` // hash of the state this batch applies to
	Data   string `json:"d"` // hash of the payload
}

// an answer to "send me your latest", and who gave it
type answer struct {
	from  uint64
	saved Saved
}

type Validator struct {
	o     Options
	c     *consensus.Consensus
	pubs  map[uint64]ed25519.PublicKey
	mu    sync.Mutex
	st    *state.State
	saved Saved
	syncs chan answer
	// what every decision told the outside, for whoever runs this
	Decided func(*state.State)
}

func New(o Options) (*Validator, error) {
	if o.Now == nil {
		o.Now = time.Now
	}
	v := &Validator{o: o, pubs: map[uint64]ed25519.PublicKey{}, st: state.New(), syncs: make(chan answer, 64)}
	var nodes []uint64
	for _, val := range o.Config.Validators {
		k, err := base64.StdEncoding.DecodeString(val.Key)
		if err != nil || len(k) != ed25519.PublicKeySize {
			return nil, fmt.Errorf("validator %d: bad key", val.ID)
		}
		v.pubs[val.ID] = ed25519.PublicKey(k)
		nodes = append(nodes, val.ID)
	}
	if _, ok := v.pubs[o.ID]; !ok {
		return nil, fmt.Errorf("%d is not a validator", o.ID)
	}
	cfg := bft.DefaultConfig
	cfg.SelfID = o.ID
	cfg.RequestBatchMaxInterval = 500 * time.Millisecond
	cfg.RequestForwardTimeout = time.Second
	cfg.RequestComplainTimeout = 4 * time.Second
	cfg.RequestAutoRemoveTimeout = time.Minute
	cfg.ViewChangeResendInterval = 2 * time.Second
	cfg.ViewChangeTimeout = 4 * time.Second
	cfg.LeaderHeartbeatTimeout = 3 * time.Second
	cfg.LeaderHeartbeatCount = 3
	cfg.SyncOnStart = true
	v.c = &consensus.Consensus{
		Config: cfg, Logger: o.Logger, Metrics: api.NewMetrics(&disabled.Provider{}, "control"),
		// finer than the leader heartbeat (1 s): a tick as long as the
		// interval lets a heartbeat look late and starts needless leader changes
		ViewChangerTicker: time.NewTicker(250 * time.Millisecond).C,
		Scheduler:         time.NewTicker(250 * time.Millisecond).C,
		Comm:              v, Signer: v, MembershipNotifier: v, Verifier: v,
		Application: v, Assembler: v, RequestInspector: v, Synchronizer: v,
		WAL: o.WAL, Metadata: &smartbftprotos.ViewMetadata{},
	}
	if o.Saved != nil && len(o.Saved.Proposal.Header) > 0 {
		if err := v.adopt(*o.Saved); err != nil {
			return nil, fmt.Errorf("the saved state: %w", err)
		}
		md := &smartbftprotos.ViewMetadata{}
		if proto.Unmarshal(o.Saved.Proposal.Metadata, md) == nil {
			v.c.Metadata = md
		}
		v.c.LastProposal = o.Saved.Proposal
		v.c.LastSignatures = o.Saved.Sigs
	}
	_ = nodes
	return v, nil
}

func (v *Validator) Start() error { return v.c.Start() }
func (v *Validator) Stop()        { v.c.Stop() }

// State is a copy of the agreed state.
func (v *Validator) State() *state.State {
	v.mu.Lock()
	defer v.mu.Unlock()
	return clone(v.st)
}

// Receive is a frame from the relay.
func (v *Validator) Receive(from uint64, kind byte, payload []byte) {
	switch kind {
	case wire.Consensus:
		m := &smartbftprotos.Message{}
		if proto.Unmarshal(payload, m) == nil {
			v.c.HandleMessage(from, m)
		}
	case wire.Forward:
		v.c.HandleRequest(from, payload)
	case wire.Submit:
		_ = v.c.SubmitRequest(payload)
	case wire.SyncAsk:
		// always answered, if only with "nothing yet": the asker stops
		// waiting once enough validators have spoken
		v.mu.Lock()
		s := v.saved
		v.mu.Unlock()
		b, _ := json.Marshal(s)
		v.o.Transport.Send(from, wire.SyncState, b)
	case wire.SyncState:
		var s Saved
		if json.Unmarshal(payload, &s) == nil {
			select {
			case v.syncs <- answer{from, s}:
			default:
			}
		}
	}
}

// ---- the state, its hash, and catching up

func hashOf(st *state.State) string {
	b, _ := json.Marshal(st) // map keys sorted: the same bytes everywhere
	h := sha256.Sum256(b)
	return hex.EncodeToString(h[:])
}

func clone(st *state.State) *state.State {
	b, _ := json.Marshal(st)
	n := state.New()
	_ = json.Unmarshal(b, n)
	return n
}

func digest(b []byte) string { h := sha256.Sum256(b); return hex.EncodeToString(h[:]) }

func (v *Validator) quorum() int {
	n := len(v.pubs)
	f := (n - 1) / 3
	return (n + f + 2) / 2 // ceil((n+f+1)/2)
}

// signedByQuorum: enough distinct validators signed this proposal
func (v *Validator) signedByQuorum(p bft.Proposal, sigs []bft.Signature) bool {
	seen := map[uint64]bool{}
	msg := proposalBytes(p)
	for _, s := range sigs {
		pub, ok := v.pubs[s.ID]
		if ok && !seen[s.ID] && ed25519.Verify(pub, msg, s.Value) {
			seen[s.ID] = true
		}
	}
	return len(seen) >= v.quorum()
}

// adopt takes a saved state and its decision, if they hold together
func (v *Validator) adopt(s Saved) error {
	var h header
	if err := json.Unmarshal(s.Proposal.Header, &h); err != nil {
		return err
	}
	if !v.signedByQuorum(s.Proposal, s.Sigs) {
		return errors.New("not signed by a quorum of validators")
	}
	before := state.New()
	if err := json.Unmarshal(s.Before, before); err != nil {
		return err
	}
	if hashOf(before) != h.Before {
		return errors.New("the state does not match the decision")
	}
	v.mu.Lock()
	defer v.mu.Unlock()
	if h.Height <= v.st.Height {
		return errors.New("not newer than what is here")
	}
	v.apply(before, s.Proposal, s.Sigs, h)
	return nil
}

// apply: `before` plus the batch becomes the state; caller holds v.mu
func (v *Validator) apply(before *state.State, p bft.Proposal, sigs []bft.Signature, h header) {
	var raw [][]byte
	_ = json.Unmarshal(p.Payload, &raw)
	reqs := make([]state.Request, 0, len(raw))
	for _, r := range raw {
		var q state.Request
		if json.Unmarshal(r, &q) == nil {
			reqs = append(reqs, q)
		}
	}
	snap, _ := json.Marshal(before)
	after := clone(before)
	after.Block(v.o.Config, state.P256, reqs, h.Time)
	v.st = after
	v.saved = Saved{Before: snap, Proposal: p, Sigs: sigs}
}

// ---- SmartBFT's side: Comm

func (v *Validator) SendConsensus(to uint64, m *smartbftprotos.Message) {
	if b, err := proto.Marshal(m); err == nil {
		v.o.Transport.Send(to, wire.Consensus, b)
	}
}
func (v *Validator) SendTransaction(to uint64, req []byte) {
	v.o.Transport.Send(to, wire.Forward, req)
}
func (v *Validator) Nodes() []uint64 {
	out := make([]uint64, 0, len(v.pubs))
	for _, val := range v.o.Config.Validators {
		out = append(out, val.ID)
	}
	return out
}

// RequestInspector: a request is known by its box and sequence
func (v *Validator) RequestID(req []byte) bft.RequestInfo {
	var q state.Request
	_ = json.Unmarshal(req, &q)
	return bft.RequestInfo{ClientID: q.Box, ID: fmt.Sprint(q.Seq)}
}

// Assembler: the proposer stamps the agreed clock and the state it builds on
func (v *Validator) AssembleProposal(md []byte, reqs [][]byte) bft.Proposal {
	v.mu.Lock()
	st := v.st
	v.mu.Unlock()
	data, _ := json.Marshal(reqs)
	t := v.o.Now().UnixMilli()
	if t <= st.Time {
		t = st.Time + 1
	}
	h, _ := json.Marshal(header{Height: st.Height + 1, Time: t, Before: hashOf(st), Data: digest(data)})
	return bft.Proposal{Header: h, Payload: data, Metadata: md}
}

// Verifier
func (v *Validator) VerifyProposal(p bft.Proposal) ([]bft.RequestInfo, error) {
	var h header
	if err := json.Unmarshal(p.Header, &h); err != nil {
		return nil, err
	}
	if h.Data != digest(p.Payload) {
		return nil, errors.New("the payload does not match its header")
	}
	v.mu.Lock()
	st := v.st
	v.mu.Unlock()
	if h.Height != st.Height+1 || h.Before != hashOf(st) {
		return nil, fmt.Errorf("built on height %d, here is %d", h.Height-1, st.Height)
	}
	if h.Time <= st.Time {
		return nil, errors.New("the clock went back")
	}
	if d := v.o.Now().UnixMilli() - h.Time; d > ClockTolerance || d < -ClockTolerance {
		return nil, fmt.Errorf("the proposer's clock is %d ms off", d)
	}
	return v.RequestsFromProposal(p), nil
}

func (v *Validator) RequestsFromProposal(p bft.Proposal) []bft.RequestInfo {
	var raw [][]byte
	_ = json.Unmarshal(p.Payload, &raw)
	out := make([]bft.RequestInfo, 0, len(raw))
	for _, r := range raw {
		out = append(out, v.RequestID(r))
	}
	return out
}

// VerifyRequest: well formed and signed by its box. Whether it passes the
// rules is decided when its batch is applied, the same way everywhere.
func (v *Validator) VerifyRequest(raw []byte) (bft.RequestInfo, error) {
	var q state.Request
	if err := json.Unmarshal(raw, &q); err != nil {
		return bft.RequestInfo{}, err
	}
	box, ok := v.o.Config.Boxes[q.Box]
	if !ok || !state.P256(box.Key, q.Bytes(), q.Sig) {
		return bft.RequestInfo{}, errors.New("not signed by a box of this fleet")
	}
	return v.RequestID(raw), nil
}

func (v *Validator) VerifyConsenterSig(s bft.Signature, p bft.Proposal) ([]byte, error) {
	pub, ok := v.pubs[s.ID]
	if !ok || !ed25519.Verify(pub, proposalBytes(p), s.Value) {
		return nil, fmt.Errorf("bad signature from %d", s.ID)
	}
	return nil, nil
}

func (v *Validator) VerifySignature(s bft.Signature) error {
	pub, ok := v.pubs[s.ID]
	if !ok || !ed25519.Verify(pub, s.Msg, s.Value) {
		return fmt.Errorf("bad signature from %d", s.ID)
	}
	return nil
}

func (v *Validator) VerificationSequence() uint64  { return 0 }
func (v *Validator) AuxiliaryData(_ []byte) []byte { return nil }

// Signer
func proposalBytes(p bft.Proposal) []byte {
	b := append([]byte("commonty control proposal v1\x00"), p.Header...)
	b = append(b, p.Payload...)
	b = append(b, p.Metadata...)
	var n [8]byte
	binary.BigEndian.PutUint64(n[:], uint64(p.VerificationSequence))
	return append(b, n[:]...)
}

func (v *Validator) Sign(m []byte) []byte { return ed25519.Sign(v.o.Key, m) }
func (v *Validator) SignProposal(p bft.Proposal, _ []byte) *bft.Signature {
	return &bft.Signature{ID: v.o.ID, Value: ed25519.Sign(v.o.Key, proposalBytes(p))}
}

// MembershipNotifier: the validator set changes with a release, for now
func (v *Validator) MembershipChange() bool { return false }

// Application
func (v *Validator) Deliver(p bft.Proposal, sigs []bft.Signature) bft.Reconfig {
	var h header
	_ = json.Unmarshal(p.Header, &h)
	v.mu.Lock()
	v.apply(v.st, p, sigs, h)
	st, saved := clone(v.st), v.saved
	v.mu.Unlock()
	if v.o.Persist != nil {
		v.o.Persist(saved)
	}
	if v.Decided != nil {
		v.Decided(st)
	}
	return bft.Reconfig{InLatestDecision: false}
}

// Synchronizer: ask everyone for their latest, and take the newest one
// that holds together - a quorum signed the decision, and the state it came
// with is the one the decision names. Nobody is taken at their word.
func (v *Validator) Sync() bft.SyncResponse {
	for len(v.syncs) > 0 {
		<-v.syncs
	}
	v.o.Transport.Send(wire.ToValidators, wire.SyncAsk, nil)
	// f+1 answers include at least one honest validator's latest
	need := (len(v.pubs)-1)/3 + 1
	heard := map[uint64]bool{}
	deadline := time.After(time.Second)
	for len(heard) < need {
		select {
		case a := <-v.syncs:
			if _, ok := v.pubs[a.from]; ok && a.from != v.o.ID {
				heard[a.from] = true
				if len(a.saved.Proposal.Header) > 0 {
					_ = v.adopt(a.saved)
				}
			}
		case <-deadline:
			need = 0
		}
	}
	v.mu.Lock()
	s := v.saved
	v.mu.Unlock()
	return bft.SyncResponse{Latest: bft.Decision{Proposal: s.Proposal, Signatures: s.Sigs}}
}
