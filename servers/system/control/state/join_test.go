package state

import (
	"commonty/control/state/ident"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"testing"
)

// a key for the fake verifier: any base64 will do
func strangerKey(i int) string {
	return base64.StdEncoding.EncodeToString([]byte(fmt.Sprintf("stranger-%d", i)))
}

func joinReq(key string, seq uint64) Request {
	name, _, _ := JoinName(key)
	b, _ := json.Marshal(JoinReq{Key: key, Recipient: "age1x", House: "h"})
	return Request{Kind: "join", Box: name, Seq: seq, Body: b, Sig: "ok:" + key}
}

func TestAStrangerJoinsOnItsOwnKeyAndNameAndHoldsNoLease(t *testing.T) {
	f := newFleet(t)
	k := strangerKey(1)
	name, id, _ := JoinName(k)
	if f.block(1000, joinReq(k, 1)) != 1 || f.s.Joined[name] == nil || f.s.Joined[name].ID != id {
		t.Fatalf("not joined: %+v", f.s.Joined)
	}
	// its heartbeats now count, signed by its own key
	b, _ := json.Marshal(Heartbeat{Facts: map[string]float64{"cores": 2}})
	if f.block(2000, Request{Kind: "heartbeat", Box: name, Seq: 2, Body: b, Sig: "ok:" + k}) != 1 {
		t.Fatal("a joined box's heartbeat refused")
	}
	// and it holds no lease, not even one any live box may hold
	op, _ := json.Marshal(lease("anyone", "acquire", 0))
	if f.block(3000, Request{Kind: "lease", Box: name, Seq: 3, Body: op, Sig: "ok:" + k}) != 0 {
		t.Fatal("a stranger was given a lease")
	}
}

func TestAJoinCannotTakeANameOrAKeyThatIsNotItsOwn(t *testing.T) {
	f := newFleet(t)
	k := strangerKey(1)
	r := joinReq(k, 1)
	r.Box = "node1"
	if f.block(1000, r) != 0 {
		t.Fatal("joined under a release box's name")
	}
	// a release box's own key does not join a second time
	if f.block(1000, joinReq("k1", 1)) != 0 {
		t.Fatal("a release box joined again under another name")
	}
	// signed by another key
	r = joinReq(k, 1)
	r.Sig = "ok:" + strangerKey(2)
	if f.block(1000, r) != 0 {
		t.Fatal("joined without its key's signature")
	}
	if f.block(1000, joinReq(k, 1)) != 1 || f.block(2000, joinReq(k, 2)) != 0 {
		t.Fatal("the same key joined twice")
	}
}

func TestJoinsAreBoundedAndTheGoneAreForgotten(t *testing.T) {
	f := newFleet(t)
	now := int64(1000)
	for i := 0; i < JoinsPerHour; i++ {
		if f.block(now, joinReq(strangerKey(i), 1)) != 1 {
			t.Fatalf("join %d refused", i)
		}
	}
	if f.block(now, joinReq(strangerKey(99), 1)) != 0 {
		t.Fatal("more joins than an hour takes")
	}
	// an hour on, more
	now += 3_600_000
	if f.block(now, joinReq(strangerKey(99), 1)) != 1 {
		t.Fatal("a join an hour later refused")
	}
	// none heard from for thirty days: all forgotten
	f.block(now + JoinedGoneAfter + 1)
	if len(f.s.Joined) != 0 {
		t.Fatalf("strangers kept: %d", len(f.s.Joined))
	}
}

func TestNoMoreThanMaxJoined(t *testing.T) {
	f := newFleet(t)
	now := int64(1000)
	for i := 0; i < MaxJoined; i++ {
		if i > 0 && i%JoinsPerHour == 0 {
			now += 3_600_000
		}
		if f.block(now, joinReq(strangerKey(i), 1)) != 1 {
			t.Fatalf("join %d refused", i)
		}
		// each keeps beating, so none is forgotten
	}
	now += 3_600_000
	if f.block(now, joinReq(strangerKey(MaxJoined), 1)) != 0 {
		t.Fatal("past the most that may join")
	}
}

func TestOnlyABoxOfTheReleaseSaysWhereAJoinedBoxIs(t *testing.T) {
	f := newFleet(t)
	k := strangerKey(1)
	name, _, _ := JoinName(k)
	f.block(1000, joinReq(k, 1))
	ip := uint32(100)<<24 | 64<<16 | 9
	b, _ := json.Marshal(NetAddr{Name: name, IPv4: ip})
	// the stranger itself may not
	if f.block(2000, Request{Kind: "netaddr", Box: name, Seq: 2, Body: b, Sig: "ok:" + k}) != 0 {
		t.Fatal("a joined box said where it is")
	}
	if f.block(3000, f.req("node1", "netaddr", NetAddr{Name: name, IPv4: ip})) != 1 || f.s.Joined[name].IPv4 != ip {
		t.Fatalf("the release box's word refused: %+v", f.s.Joined[name])
	}
	// and never off the fleet's network
	if f.block(4000, f.req("node1", "netaddr", NetAddr{Name: name, IPv4: 8 << 24})) != 0 {
		t.Fatal("an address off the fleet's network")
	}
}

func TestAJoinedBoxOnlySendsHeartbeats(t *testing.T) {
	f := newFleet(t)
	k := strangerKey(1)
	name, _, _ := JoinName(k)
	f.block(1000, joinReq(k, 1))
	for i, kind := range []string{"directory", "built"} {
		r := Request{Kind: kind, Box: name, Seq: uint64(2 + i), Body: []byte(`{}`), Sig: "ok:" + k}
		if f.block(2000, r) != 0 {
			t.Fatalf("a joined box sent a %s", kind)
		}
	}
}

func ticketJoin(key, ticket string, seq uint64) Request {
	name, _, _ := JoinName(key)
	b, _ := json.Marshal(JoinReq{Key: key, Recipient: "age1x", House: "h", Ticket: ticket})
	return Request{Kind: "join", Box: name, Seq: seq, Body: b, Sig: "ok:" + key}
}

func TestATicketGoesWithOneBoxAndIsCarriedForTheGates(t *testing.T) {
	f := newFleet(t)
	a, b := strangerKey(1), strangerKey(2)
	an, _, _ := JoinName(a)
	if f.block(1000, ticketJoin(a, "En0KEwoEdXNlcg", 1)) != 1 || f.s.Joined[an].Ticket != "En0KEwoEdXNlcg" {
		t.Fatalf("the ticket is not carried: %+v", f.s.Joined[an])
	}
	// the same ticket with another box is someone's copy of it
	if f.block(2000, ticketJoin(b, "En0KEwoEdXNlcg", 1)) != 0 {
		t.Fatal("one ticket came with two boxes")
	}
	// one too long is refused, and a box without one is a stranger as before
	big := make([]byte, MaxTicket+1)
	for i := range big {
		big[i] = 'A'
	}
	if f.block(3000, ticketJoin(b, string(big), 1)) != 0 {
		t.Fatal("an oversized ticket was taken")
	}
	bn, _, _ := JoinName(b)
	if f.block(4000, joinReq(b, 1)) != 1 || f.s.Joined[bn].Ticket != "" {
		t.Fatal("a stranger's join changed")
	}
}

func TestAJoinWithATicketIsInTheDirectoryLogUnderItsName(t *testing.T) {
	f := newFleet(t)
	k := strangerKey(1)
	name, _, _ := JoinName(k)
	r := joinReq(k, 1)
	r.Body, _ = json.Marshal(JoinReq{Key: k, Recipient: "age1x", House: "h", Ticket: "T"})
	if f.block(1000, r) != 1 || len(f.s.Directory) != 1 {
		t.Fatalf("%+v", f.s.Directory)
	}
	d := f.s.Directory[0]
	if d.Box != name || string(d.Item) != `{"joined":{"key":"`+k+`","ticket":"T"}}` {
		t.Fatalf("%s %s", d.Box, d.Item)
	}
	// a stranger's join, with none, puts nothing there
	if f.block(2000, joinReq(strangerKey(2), 1)) != 1 || len(f.s.Directory) != 1 {
		t.Fatal("a join with no ticket was logged")
	}
	// and no box's gate can say one
	for i, body := range []string{
		`{"joined":{"key":"x","ticket":"T"}}`,
		`{"entry":{},"joined":{}}`,
		`{"joined":{}}`,
	} {
		req := Request{Kind: "directory", Box: "node1", Seq: uint64(10 + i), Body: json.RawMessage(body), Sig: "ok:k1"}
		if f.block(3000+int64(i), req) != 0 {
			t.Fatalf("a gate put %s", body)
		}
	}
	ok := Request{Kind: "directory", Box: "node1", Seq: 20, Body: json.RawMessage(`{"ack":"x"}`), Sig: "ok:k1"}
	if f.block(4000, ok) != 1 {
		t.Fatal("an ordinary item refused")
	}
}

func TestABoxDownByOurFaultIsNotForgotten(t *testing.T) {
	f := newFleet(t)
	k := strangerKey(1)
	name, _, _ := JoinName(k)
	f.block(1000, joinReq(k, 1))
	// release 6 is out: the release's own box runs it
	f.block(1500, f.req("node1", "heartbeat", Heartbeat{Facts: map[string]float64{"release": 6}}))
	b, _ := json.Marshal(Heartbeat{Facts: map[string]float64{"agent_ok": 0, "release": 5}})
	f.block(2000, Request{Kind: "heartbeat", Box: name, Seq: 2, Body: b, Sig: "ok:" + k})
	f.block(2000 + JoinedGoneAfter + 1)
	if f.s.Joined[name] == nil || f.s.Boxes[name] == nil {
		t.Fatal("a box whose release failed on it was forgotten")
	}
	got := f.s.IdentBoxes()
	if info, ok := got(name); !ok || info.LastSeen != 2000 || info.Fleet != 6 {
		t.Fatal(info, ok)
	}
	// not for ever: past ident.OurFaultFor it goes like any other
	f.block(2000 + ident.OurFaultFor*1000 + 1)
	if f.s.Joined[name] != nil {
		t.Fatal("a box down by our fault, by its own word, kept its place for good")
	}
	if _, ok := got("nobody"); ok {
		t.Fatal("a box that is not there")
	}
}

func TestAStrangersWordAloneIsNotOurFault(t *testing.T) {
	f := newFleet(t)
	f.block(500, f.req("node1", "heartbeat", Heartbeat{Facts: map[string]float64{"release": 6}}))
	for i, facts := range []map[string]float64{
		{"agent_ok": 0},
		{"agent_ok": 0, "release": 6},
		// a joined box's word of a newer release is no release of ours
		{"agent_ok": 0, "release": 9},
	} {
		k := strangerKey(10 + i)
		name, _, _ := JoinName(k)
		at := 1000 + int64(i)*2*JoinedGoneAfter
		f.block(at, joinReq(k, 1))
		b, _ := json.Marshal(Heartbeat{Facts: facts})
		f.block(at, Request{Kind: "heartbeat", Box: name, Seq: 2, Body: b, Sig: "ok:" + k})
		if f.s.Joined[name] == nil {
			t.Fatalf("%v: not joined", facts)
		}
		f.block(at + JoinedGoneAfter + 1)
		if f.s.Joined[name] != nil {
			t.Fatalf("%v: a stranger that said a release failed, with none newer out, kept its place", facts)
		}
	}
}
