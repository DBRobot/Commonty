package validator

import (
	"encoding/json"
	"strings"
	"testing"
	"time"

	"commonty/control/state"
	"commonty/control/wire"

	bft "github.com/hyperledger/SmartBFT/pkg/types"
	"github.com/hyperledger/SmartBFT/smartbftprotos"
	"google.golang.org/protobuf/proto"
)

// a proposal on v's state at agreed time t, carrying reqs
func proposalAt(v *Validator, t int64, reqs ...[]byte) bft.Proposal {
	if reqs == nil {
		reqs = [][]byte{}
	}
	data, _ := json.Marshal(reqs)
	st := v.State()
	h, _ := json.Marshal(header{Height: st.Height + 1, Time: t, Before: hashOf(st), Data: digest(data)})
	return proposalOf(h, data)
}

func verifyAt(v *Validator, p bft.Proposal) error {
	_, err := v.VerifyProposal(p)
	return err
}

// 2026-10-24: node1's proposal reached node2 2881 ms after its agreed
// time (node1 busy on disk between stamping it and sending it), node2
// refused it as "the proposer's clock is 2881 ms off", and the two
// validators left with the witness off never agreed again. However late a
// proposal arrives, or is checked again, it is taken if its requests were
// not seen here long before its agreed time
func TestALateProposalIsStillTaken(t *testing.T) {
	f := newStandFleet(t)
	v := f.validator(f.cfg)
	sent := f.now
	req := f.box.req("heartbeat", state.Heartbeat{Facts: map[string]float64{"n": 1}})
	v.Receive(101, wire.Submit, req)
	p := proposalAt(v, sent+100, req)
	for _, late := range []int64{0, 2881, 4500, 60_000, 10 * 60_000} {
		f.now = sent + 100 + late
		if err := verifyAt(v, p); err != nil {
			t.Fatalf("a proposal checked %d ms after its agreed time: %v", late, err)
		}
	}
	// one carrying nothing to apply is held only to the clock's direction
	f.now = sent + 10*60_000
	if err := verifyAt(v, proposalAt(v, sent)); err != nil {
		t.Fatalf("an empty proposal checked late: %v", err)
	}
}

// A request never seen here counts as seen as the proposal is checked: a
// proposal carrying one is taken up to ClockBehind late, as the night's was
func TestARequestNeverSeenCountsAsSeenNow(t *testing.T) {
	f := newStandFleet(t)
	v := f.validator(f.cfg)
	req := f.box.req("heartbeat", state.Heartbeat{Facts: map[string]float64{"n": 1}})
	p := proposalAt(v, f.now, req)
	at := f.now
	f.now = at + 2881
	if err := verifyAt(v, p); err != nil {
		t.Fatalf("2881 ms late with a request not seen before: %v", err)
	}
	f.now = at + ClockBehind + 1
	refused(t, verifyAt(v, p), "first seen here")
}

// The agreed time stays sane: never ahead of this validator's clock by more
// than ClockAhead, never more than ClockBehind before a request it carries
// was first seen here, never back
func TestTheAgreedTimeIsBounded(t *testing.T) {
	f := newStandFleet(t)
	v := f.validator(f.cfg)
	refused(t, verifyAt(v, proposalAt(v, f.now+ClockAhead+1)), "ahead")
	if err := verifyAt(v, proposalAt(v, f.now+ClockAhead)); err != nil {
		t.Fatalf("ClockAhead ahead: %v", err)
	}
	if err := verifyAt(v, proposalAt(v, f.now+5*60_000)); err == nil {
		t.Fatal("a proposal five minutes ahead was taken")
	}
	req := f.box.req("heartbeat", state.Heartbeat{Facts: map[string]float64{"n": 1}})
	v.Receive(101, wire.Submit, req)
	seen := f.now
	f.now += 30_000
	refused(t, verifyAt(v, proposalAt(v, seen-ClockBehind-1, req)), "first seen here")
	if err := verifyAt(v, proposalAt(v, seen-ClockBehind, req)); err != nil {
		t.Fatalf("ClockBehind before the request was seen: %v", err)
	}
	// seen again later, it keeps the first time
	v.Receive(101, wire.Submit, req)
	if err := verifyAt(v, proposalAt(v, seen-ClockBehind, req)); err != nil {
		t.Fatalf("a request seen twice: %v", err)
	}
	if err := verifyAt(v, proposalAt(v, 0)); err == nil || !strings.Contains(err.Error(), "went back") {
		t.Fatalf("a proposal at the agreed time so far: %v", err)
	}
}

func TestSeenForgetsOnlyAfterItsTime(t *testing.T) {
	var s seenAt
	id := (&Validator{}).RequestID([]byte(`{"box":"a","seq":1}`))
	s.saw(id, 1000)
	s.saw(id, 1000+seenFor+1)
	if got := s.first(id, 1000+seenFor+2); got != 1000 {
		t.Fatalf("first seen %d, want 1000", got)
	}
	s.saw((&Validator{}).RequestID([]byte(`{"box":"b","seq":1}`)), 1000+3*seenFor)
	if got := s.first(id, 1000+3*seenFor); got != 1000 {
		t.Fatalf("an entry carried into the next generation was lost: %d", got)
	}
	s.saw((&Validator{}).RequestID([]byte(`{"box":"b","seq":2}`)), 1000+5*seenFor)
	if got := s.first(id, 1000+5*seenFor); got != 1000+5*seenFor {
		t.Fatalf("an entry outlived two generations: %d", got)
	}
}

// End to end: every proposal reaches the others 2.5 s after it was
// stamped, past the 2 s release 361 allowed, and the validators go on
// deciding
func TestValidatorsDecideWhileProposalsArriveLate(t *testing.T) {
	c := newCluster(t)
	c.until("first decisions", func() bool { return c.height(1) >= 2 && c.height(2) >= 2 })
	c.r.mu.Lock()
	c.r.slow = func(kind byte, p []byte) time.Duration {
		m := &smartbftprotos.Message{}
		if kind == wire.Consensus && proto.Unmarshal(p, m) == nil && m.GetPrePrepare() != nil {
			return 2500 * time.Millisecond
		}
		return 0
	}
	c.r.mu.Unlock()
	h := max(c.height(1), c.height(2), c.height(3), c.height(4))
	// a view changes now and then while proposals are this late (a box's
	// request waits 4 s before it complains), so the wait is long; release
	// 361 decided nothing at all here
	end := time.Now().Add(90 * time.Second)
	for n := 0; ; n++ {
		ahead := 0
		for id := uint64(1); id <= 4; id++ {
			if c.height(id) >= h+3 {
				ahead++
			}
		}
		if ahead >= 3 {
			return
		}
		if time.Now().After(end) {
			t.Fatalf("decided nothing with late proposals (heights %d %d %d %d, from %d)", c.height(1), c.height(2), c.height(3), c.height(4), h)
		}
		c.heartbeat(float64(2000 + n))
		time.Sleep(300 * time.Millisecond)
	}
}
