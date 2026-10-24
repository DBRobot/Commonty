// Package validator is one voter: SmartBFT orders the boxes' signed
// requests, and every agreed batch is applied to the state machine
// (package state) the same way on every validator. The same code runs as a
// process on a box and as wasm in the relay's Durable Object; what differs
// is the transport, the write-ahead log and where the state is kept.
package validator

import (
	"cmp"
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"slices"
	"sync"
	"sync/atomic"
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
	// what the log held from a previous run, replayed at start
	WALEntries [][]byte
	// a saved state and the decision that led to it, from a previous run
	Saved *Saved
	// called after every decision with the new state, for a box to keep
	Persist func(Saved)
	Now     func() time.Time
	// tests only: run as this state machine version instead of
	// state.Machine; 1 sends no stand, as release 359
	Machine int
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
	// a version and config every validator stood for, agreed from this
	// block on (stand.go); never in a block a validator of 359 votes on
	Activate *activation `json:"a,omitempty"`
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
	// frames for consensus, handed on by goroutines of their own: a
	// consensus that hangs never stops the relay's reader, nor the answers
	// to peers catching up (two wedges on 2026-10-22). The boxes' requests
	// wait apart from the votes, and each validator's votes in a queue of
	// their own, taken in turn: one sender's flood drops only its own
	// oldest frames, never another validator's votes
	peers   map[uint64]chan frame
	order   []uint64
	turn    int
	voted   chan struct{}
	clients chan frame
	// decided requests other validators passed on, for the next block
	// this one proposes (replays below)
	replayMu sync.Mutex
	replay   map[bft.RequestInfo][]byte
	// tests only: as release 359's validator, which takes in decided
	// requests and keeps them in its pool after it catches up
	keepDecided bool
	limit       limiter
	boxIDs      map[uint64]bool
	asks        chan uint64
	done        chan struct{}
	stop        sync.Once
	running     atomic.Bool
	// each validator's newest stand (stand.go), under its own lock: the
	// relay's reader takes it, and never waits on a decision being applied
	standsMu sync.Mutex
	stands   map[uint64]Stand
	// this validator's newest stand, what it proposes with
	mine Stand
	pulse
	// what every decision told the outside, for whoever runs this
	Decided func(*state.State)
}

type frame struct {
	from    uint64
	kind    byte
	payload []byte
}

// how many frames wait for consensus before the oldest is dropped, of the
// boxes' requests and of each validator's votes; the relay drops frames
// too, and consensus asks again for what it misses
const (
	inbox     = 4096
	peerInbox = 1024
)

func New(o Options) (*Validator, error) {
	if o.Now == nil {
		o.Now = time.Now
	}
	v := &Validator{o: o, pubs: map[uint64]ed25519.PublicKey{}, st: state.New(), syncs: make(chan answer, 64),
		peers: map[uint64]chan frame{}, voted: make(chan struct{}, 1), clients: make(chan frame, inbox), asks: make(chan uint64, 64), done: make(chan struct{}), stands: map[uint64]Stand{}}
	v.pulse.init(o.Now())
	var nodes []uint64
	for _, val := range o.Config.Validators {
		k, err := base64.StdEncoding.DecodeString(val.Key)
		if err != nil || len(k) != ed25519.PublicKeySize {
			return nil, fmt.Errorf("validator %d: bad key", val.ID)
		}
		v.pubs[val.ID] = ed25519.PublicKey(k)
		nodes = append(nodes, val.ID)
		if val.ID != o.ID && v.peers[val.ID] == nil {
			v.peers[val.ID] = make(chan frame, peerInbox)
			v.order = append(v.order, val.ID)
		}
	}
	if _, ok := v.pubs[o.ID]; !ok {
		return nil, fmt.Errorf("%d is not a validator", o.ID)
	}
	v.boxIDs = map[uint64]bool{}
	for _, b := range o.Config.Boxes {
		v.boxIDs[b.ID] = true
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
		WAL: o.WAL, WALInitialContent: o.WALEntries, Metadata: &smartbftprotos.ViewMetadata{},
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
	// answering a peer that catches up needs nothing of consensus, and
	// starts before it
	go v.answer()
	return v, nil
}

func (v *Validator) Start() error {
	if err := v.c.Start(); err != nil {
		return err
	}
	v.pulse.started(v.o.Now())
	v.running.Store(true)
	go v.intakeVotes()
	go v.intake(v.clients)
	go v.say()
	return nil
}

func (v *Validator) Stop() {
	v.stop.Do(func() { close(v.done) })
	if v.running.Swap(false) {
		v.c.Stop()
	}
}

// State is a copy of the agreed state.
func (v *Validator) State() *state.State {
	v.mu.Lock()
	defer v.mu.Unlock()
	return clone(v.st)
}

// Receive is a frame from the relay. It never waits on consensus: frames
// for it are queued, and a peer's "send me your latest" is answered from
// what was last decided, however consensus is doing.
func (v *Validator) Receive(from uint64, kind byte, payload []byte) {
	switch kind {
	case wire.Consensus:
		// votes only from the validators
		if q := v.peers[from]; q != nil {
			v.queue(q, frame{from, kind, payload}, true)
			select {
			case v.voted <- struct{}{}:
			default:
			}
		}
	case wire.Forward, wire.Submit:
		// a request passed on comes only from a validator; any sender but
		// this box itself is held to a rate; and only one some box signed
		// goes on to consensus
		if _, ok := v.pubs[from]; kind == wire.Forward && (!ok || from == v.o.ID) {
			return
		}
		if from != v.o.ID && !v.limit.allow(from, v.sender(from), v.o.Now()) {
			return
		}
		// a decided request never applies: it is dropped, and one a
		// validator passed on goes in this one's next block (replayLater)
		done, signed, fresh := v.check(payload)
		if v.keepDecided && done {
			done, signed = false, v.signed(payload)
		}
		if done && kind == wire.Forward {
			v.replayLater(payload)
		}
		if done || !signed {
			return
		}
		// it counts as waiting only if it is newer than what was agreed of
		// its box and not a join: replays never make a stall
		if fresh && v.pulse.idle() {
			v.pulse.request(v.o.Now())
		}
		v.queue(v.clients, frame{from, kind, payload}, false)
	case wire.SyncAsk:
		// always answered, if only with "nothing yet": the asker stops
		// waiting once enough validators have spoken
		select {
		case v.asks <- from:
		default:
		}
	case wire.Stand:
		var s Stand
		if json.Unmarshal(payload, &s) == nil && s.ID == from {
			v.heardStand(s)
		}
	case wire.SyncState:
		var s Saved
		if json.Unmarshal(payload, &s) == nil {
			v.heard(from, s)
			select {
			case v.syncs <- answer{from, s}:
			default:
			}
		}
	}
}

// sender: who from is - a validator, a box of the release or one that
// joined, a box joining on its own key (an address in the joined range), or
// nobody the fleet knows
func (v *Validator) sender(from uint64) sender {
	if _, ok := v.pubs[from]; ok {
		return aValidator
	}
	if v.boxIDs[from] {
		return aBox
	}
	v.mu.Lock()
	defer v.mu.Unlock()
	for _, j := range v.st.Joined {
		if j.ID == from {
			return aBox
		}
	}
	if from >= state.JoinedIDBase && from < 2*state.JoinedIDBase {
		return aStranger
	}
	return unknown
}

// check: whether a request is decided (decided below), which is asked
// first as it costs no signature check; whether it is signed as
// VerifyRequest wants (a join by the key it brings); and whether it is
// fresh: signed by a box of the fleet, not a join, and not decided
func (v *Validator) check(raw []byte) (done, signed, fresh bool) {
	var q state.Request
	if json.Unmarshal(raw, &q) != nil {
		return false, false, false
	}
	v.mu.Lock()
	done = decided(v.st.Boxes[q.Box], q)
	v.mu.Unlock()
	if done {
		return true, false, false
	}
	if _, err := v.VerifyRequest(raw); err != nil {
		return false, false, false
	}
	return false, true, q.Kind != "join"
}

// signed: VerifyRequest passes it
func (v *Validator) signed(raw []byte) bool {
	_, err := v.VerifyRequest(raw)
	return err == nil
}

// decided: no block can apply q any more, as its box's agreed sequence
// has reached it (state.ErrStale). A join is never taken for decided: it
// is checked otherwise
func decided(b *state.BoxState, q state.Request) bool {
	return q.Kind != "join" && b != nil && q.Seq <= b.Seq
}

// how many decided requests one block carries again, at most
const maxReplays = 64

// replayLater keeps a decided request another validator passed on, for
// the next block this one proposes. It was waiting in that validator's
// pool: one of 359 never prunes (below), and every leader drops it as
// already processed, so it would complain about each leader until it led
// itself. In a block it changes nothing (state.ErrStale), and once decided
// it leaves that validator's pool
func (v *Validator) replayLater(raw []byte) {
	info, err := v.VerifyRequest(raw)
	if err != nil {
		return
	}
	v.replayMu.Lock()
	defer v.replayMu.Unlock()
	if v.replay == nil {
		v.replay = map[bft.RequestInfo][]byte{}
	}
	if len(v.replay) < maxReplays {
		v.replay[info] = raw
	}
}

// replays: reqs and the decided requests kept since the last proposal
func (v *Validator) replays(reqs [][]byte) [][]byte {
	v.replayMu.Lock()
	kept := v.replay
	v.replay = nil
	v.replayMu.Unlock()
	if len(kept) == 0 {
		return reqs
	}
	for _, r := range reqs {
		delete(kept, v.RequestID(r))
	}
	ids := slices.SortedFunc(maps.Keys(kept), func(a, b bft.RequestInfo) int {
		return cmp.Or(cmp.Compare(a.ClientID, b.ClientID), cmp.Compare(a.ID, b.ID))
	})
	out := append([][]byte(nil), reqs...)
	for _, id := range ids {
		out = append(out, kept[id])
	}
	return out
}

// prune drops every decided request from the pool. After catching up
// from a peer's state (Sync) nothing else does: the blocks that decided
// them were never delivered here, a leader drops them as already
// processed, and each one would end in a complaint about the leader, which
// with three validators changes the view for all (docs/dev/control-plane.md,
// "A pool holds only what is still to be decided")
func (v *Validator) prune() {
	if v.c.Pool == nil || v.keepDecided {
		return
	}
	v.mu.Lock()
	boxes := make(map[string]*state.BoxState, len(v.st.Boxes))
	maps.Copy(boxes, v.st.Boxes)
	v.mu.Unlock()
	v.c.Pool.Prune(func(raw []byte) error {
		var q state.Request
		if json.Unmarshal(raw, &q) == nil && decided(boxes[q.Box], q) {
			return state.ErrStale
		}
		return nil
	})
}

// queue: full, the oldest frame goes, and that is said once in a while
func (v *Validator) queue(q chan frame, f frame, peer bool) {
	for {
		select {
		case q <- f:
			return
		default:
		}
		select {
		case <-q:
			if n := v.pulse.drop(v.o.Now(), peer); n > 0 {
				v.o.Logger.Warnf("consensus is not taking frames: %d dropped, oldest first", n)
			}
		default:
		}
	}
}

// intakeVotes hands the validators' votes to consensus, one at a time and
// one validator after another. How long a vote in hand has taken is what
// the watchdog reads
func (v *Validator) intakeVotes() {
	for {
		f, ok := v.nextVote()
		if !ok {
			select {
			case <-v.done:
				return
			case <-v.voted:
			}
			continue
		}
		v.pulse.enter(&v.pulse.calling, v.o.Now())
		v.hand(f)
		v.pulse.leave(&v.pulse.calling)
		select {
		case <-v.done:
			return
		default:
		}
	}
}

// nextVote: the oldest vote of the next validator in turn that has one, so
// a validator that sends without end gets its turn and no more
func (v *Validator) nextVote() (frame, bool) {
	for i := range v.order {
		n := (v.turn + i) % len(v.order)
		select {
		case f := <-v.peers[v.order[n]]:
			v.turn = n + 1
			return f, true
		default:
		}
	}
	return frame{}, false
}

// intake hands the boxes' requests to consensus, one at a time. A request
// may wait on a full pool, which a box can cause, and is not timed
func (v *Validator) intake(q chan frame) {
	for {
		select {
		case <-v.done:
			return
		case f := <-q:
			v.hand(f)
		}
	}
}

func (v *Validator) hand(f frame) {
	switch f.kind {
	case wire.Consensus:
		m := &smartbftprotos.Message{}
		if proto.Unmarshal(f.payload, m) == nil {
			v.c.HandleMessage(f.from, m)
		}
	case wire.Forward:
		v.c.HandleRequest(f.from, f.payload)
	case wire.Submit:
		_ = v.c.SubmitRequest(f.payload)
	}
}

// answer sends the askers what was last decided, off the relay's reader
func (v *Validator) answer() {
	for {
		select {
		case <-v.done:
			return
		case to := <-v.asks:
			v.mu.Lock()
			s := v.saved
			v.mu.Unlock()
			b, _ := json.Marshal(s)
			v.o.Transport.Send(to, wire.SyncState, b)
		}
	}
}

// heard: a peer's latest, kept only if a quorum signed it
func (v *Validator) heard(from uint64, s Saved) {
	if _, ok := v.pubs[from]; !ok || from == v.o.ID {
		return
	}
	var h header
	if len(s.Proposal.Header) > 0 && (json.Unmarshal(s.Proposal.Header, &h) != nil || !v.signedByQuorum(s.Proposal, s.Sigs)) {
		return
	}
	v.pulse.peer(from, h.Height, v.o.Now())
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

// activated: the version a decision makes agreed, 0 for none
func activated(h header) int {
	if h.Activate == nil {
		return 0
	}
	return h.Activate.Machine
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
	if m := max(before.Version(), activated(h)); m > v.machine() {
		return fmt.Errorf("agreed on state machine %d; this code runs %d", m, v.machine())
	}
	v.mu.Lock()
	defer v.mu.Unlock()
	if h.Height <= v.st.Height {
		return errors.New("not newer than what is here")
	}
	v.apply(before, s.Proposal, s.Sigs, h)
	if v.o.Now != nil {
		v.pulse.moved(h.Height, v.o.Now())
	}
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
	if a := h.Activate; a != nil {
		prev := before.In(v.o.Config)
		after.Machine, after.Config, after.ConfigTime = a.Machine, a.Config, h.Time
		// across the change, a request counts only if both configs know its
		// box by the same key
		next := after.In(v.o.Config)
		kept := reqs[:0]
		for _, q := range reqs {
			if before.SameKey(prev, next, q) {
				kept = append(kept, q)
			}
		}
		reqs = kept
	}
	after.Block(after.In(v.o.Config), state.P256, reqs, h.Time)
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
	reqs = v.replays(reqs)
	v.mu.Lock()
	st := v.st
	t := v.o.Now().UnixMilli()
	if t <= st.Time {
		t = st.Time + 1
	}
	a := v.activate(st, t)
	v.mu.Unlock()
	data, _ := json.Marshal(reqs)
	h, _ := json.Marshal(header{Height: st.Height + 1, Time: t, Before: hashOf(st), Data: digest(data), Activate: a})
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
	if h.Activate != nil {
		if err := h.Activate.check(v, st, h.Time); err != nil {
			return nil, fmt.Errorf("activation: %w", err)
		}
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
	// a join is signed by the key it brings; anything else by a box of the
	// release or one that joined
	key, ok := "", false
	if q.Kind == "join" {
		var j state.JoinReq
		if json.Unmarshal(q.Body, &j) == nil {
			key, ok = j.Key, true
		}
	} else {
		v.mu.Lock()
		key, ok = v.st.BoxKey(v.st.In(v.o.Config), q.Box)
		v.mu.Unlock()
	}
	if !ok || !state.P256(key, q.Bytes(), q.Sig) {
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
	v.pulse.enter(&v.pulse.delivering, v.o.Now())
	defer v.pulse.leave(&v.pulse.delivering)
	v.mu.Lock()
	v.apply(v.st, p, sigs, h)
	st, saved := clone(v.st), v.saved
	v.mu.Unlock()
	v.pulse.moved(h.Height, v.o.Now())
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
	v.mu.Lock()
	from := v.st.Height
	v.mu.Unlock()
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
	s, moved := v.saved, v.st.Height > from
	v.mu.Unlock()
	if moved {
		v.prune()
	}
	return bft.SyncResponse{Latest: bft.Decision{Proposal: s.Proposal, Signatures: s.Sigs}}
}
