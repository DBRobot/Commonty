package ident

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/binary"
	"testing"
)

// biscuit-cli 0.6's, from a device key of seed 3 (key(3)): user("ann"),
// device("f"), ticket("0011…eeff"), purpose("join"), until 1791800000; and
// the same with a block a holder appended naming another ticket
const (
	cliTicket   = "EqMCCrgBCgNhbm4KBmRldmljZQoBZgoGdGlja2V0CiAwMDExMjIzMzQ0NTU2Njc3ODg5OWFhYmJjY2RkZWVmZgoHcHVycG9zZQoEam9pbgoBdBgDIgkKBwgKEgMYgAgiCgoICIEIEgMYgggiCgoICIMIEgMYhAgiCgoICIUIEgMYhggyKAomCgIIGxIHCAUSAwiHCBoXCgUKAwiHCAoICgYQwOWy1gYKBBoCCAAyDwoNCgIIGxIHCAMSAxiGCBIkCAASIKbGi1Im0RWOiZK_7j48dq3cqzLyqaZjCtsO0C0zlBicGkCt620JFzrYN2cTNjm0sU3-FPj5sBM1s7q5nyEqZTVhqC2MxgP2nvit0QI7J5XDkTjFK5SWGgVvTQIedukXI3kHIiIKIPiUraeqCqTMMTgjg1f-AJ13kTHwuDXDeVacBd7SkdXX"
	cliAppended = "EqMCCrgBCgNhbm4KBmRldmljZQoBZgoGdGlja2V0CiAwMDExMjIzMzQ0NTU2Njc3ODg5OWFhYmJjY2RkZWVmZgoHcHVycG9zZQoEam9pbgoBdBgDIgkKBwgKEgMYgAgiCgoICIEIEgMYgggiCgoICIMIEgMYhAgiCgoICIUIEgMYhggyKAomCgIIGxIHCAUSAwiHCBoXCgUKAwiHCAoICgYQwOWy1gYKBBoCCAAyDwoNCgIIGxIHCAMSAxiGCBIkCAASIKbGi1Im0RWOiZK_7j48dq3cqzLyqaZjCtsO0C0zlBicGkCt620JFzrYN2cTNjm0sU3-FPj5sBM1s7q5nyEqZTVhqC2MxgP2nvit0QI7J5XDkTjFK5SWGgVvTQIedukXI3kHGpoBCjAKIGZmZWVkZGNjYmJhYTk5ODg3NzY2NTU0NDMzMjIxMTAwGAMiCgoICIMIEgMYiAgSJAgAEiA3BMa99CtdxKrbo50t_ZaEegnhlOSlfSJOZb86fdpURhpA0D4J2dK1nrqCpYfIpgZkOPGa4It4_zv9efqYPLn0XOa9lj89hZUjCoJ2FBa3yHIIMc_rm9UER0b_dZOb7SQFDiIiCiBqoXNBppBi8qbA4zADqN0WzuGnPn1aOCBnc5G4VuIwoA=="
	ticketA     = "00112233445566778899aabbccddeeff"
)

func TestATicketBiscuitMade(t *testing.T) {
	for _, tok := range []string{cliTicket, cliAppended} {
		got, err := ReadTicket(tok, []string{pub(key(9)), pub(key(3))})
		if err != nil {
			t.Fatal(err)
		}
		if got.User != "ann" || got.ID != ticketA || got.Expires != 1791800000 || got.Device != pub(key(3)) {
			t.Fatalf("%+v", got)
		}
	}
	if _, err := ReadTicket(cliTicket, []string{pub(key(4))}); err == nil {
		t.Fatal("read with another device's key")
	}
	if id, exp := TicketID(cliTicket); id != ticketA || exp != 1791800000 {
		t.Fatal(id, exp)
	}
}

// the tests' own tickets, written as biscuit-auth writes them
func pbVarint(b []byte, num, v uint64) []byte {
	b = binary.AppendUvarint(b, num<<3)
	return binary.AppendUvarint(b, v)
}

func pbBytes(b []byte, num uint64, v []byte) []byte {
	b = binary.AppendUvarint(b, num<<3|2)
	b = binary.AppendUvarint(b, uint64(len(v)))
	return append(b, v...)
}

type mint struct {
	facts  [][2]string
	expiry uint64
	rule   bool
}

func (m mint) token(dev ed25519.PrivateKey) string {
	var symbols []string
	sym := func(s string) uint64 {
		for i, d := range biscuitSymbols {
			if d == s {
				return uint64(i)
			}
		}
		for i, d := range symbols {
			if d == s {
				return uint64(biscuitOffset + i)
			}
		}
		symbols = append(symbols, s)
		return uint64(biscuitOffset + len(symbols) - 1)
	}
	strTerm := func(s string) []byte { return pbVarint(nil, 3, sym(s)) }
	pred := func(name uint64, terms ...[]byte) []byte {
		p := pbVarint(nil, 1, name)
		for _, t := range terms {
			p = pbBytes(p, 2, t)
		}
		return p
	}
	var body []byte
	for _, f := range m.facts {
		body = pbBytes(body, 4, pbBytes(nil, 1, pred(sym(f[0]), strTerm(f[1]))))
	}
	if m.rule {
		body = pbBytes(body, 5, pbBytes(nil, 1, pred(sym("ticket"), strTerm("x"))))
	}
	if m.expiry > 0 {
		v := pbVarint(nil, 1, sym("t"))
		ops := pbBytes(nil, 1, pbBytes(nil, 1, v))
		ops = pbBytes(ops, 1, pbBytes(nil, 1, pbVarint(nil, 2, m.expiry)))
		ops = pbBytes(ops, 1, pbBytes(nil, 3, pbVarint(nil, 1, 0)))
		rule := pbBytes(nil, 1, pred(sym("query")))
		rule = pbBytes(rule, 2, pred(sym("time"), v))
		rule = pbBytes(rule, 3, ops)
		body = pbBytes(body, 6, pbBytes(nil, 1, rule))
	}
	var block []byte
	for _, s := range symbols {
		block = pbBytes(block, 1, []byte(s))
	}
	block = pbVarint(block, 3, 3)
	block = append(block, body...)
	next := key(200).Public().(ed25519.PublicKey)
	msg := append(append(append([]byte(nil), block...), 0, 0, 0, 0), next...)
	sb := pbBytes(nil, 1, block)
	sb = pbBytes(sb, 2, pbBytes(pbVarint(nil, 1, 0), 2, next))
	sb = pbBytes(sb, 3, ed25519.Sign(dev, msg))
	tok := pbBytes(nil, 2, sb)
	tok = pbBytes(tok, 4, pbBytes(nil, 1, key(200).Seed()))
	return base64.URLEncoding.EncodeToString(tok)
}

func joinTicket(user, id string, exp uint64) mint {
	return mint{facts: [][2]string{{"user", user}, {"device", "f"}, {"ticket", id}, {"purpose", "join"}}, expiry: exp}
}

func TestOnlyAJoinTicketIsOne(t *testing.T) {
	dev := key(3)
	good := joinTicket("ann", ticketA, 1791800000)
	if got, err := ReadTicket(good.token(dev), []string{pub(dev)}); err != nil || got.ID != ticketA || got.Expires != 1791800000 {
		t.Fatal(got, err)
	}
	bad := map[string]mint{
		"no expiry":       joinTicket("ann", ticketA, 0),
		"another use":     {facts: [][2]string{{"user", "ann"}, {"ticket", ticketA}, {"purpose", "access"}}, expiry: 1791800000},
		"no user":         {facts: [][2]string{{"ticket", ticketA}, {"purpose", "join"}}, expiry: 1791800000},
		"two tickets":     {facts: [][2]string{{"user", "ann"}, {"ticket", ticketA}, {"ticket", "ffeeddccbbaa99887766554433221100"}, {"purpose", "join"}}, expiry: 1791800000},
		"a short id":      joinTicket("ann", "0011", 1791800000),
		"an uppercase id": joinTicket("ann", "00112233445566778899AABBCCDDEEFF", 1791800000),
		"a rule":          {facts: good.facts, expiry: 1791800000, rule: true},
	}
	for what, m := range bad {
		if _, err := ReadTicket(m.token(dev), []string{pub(dev)}); err == nil {
			t.Errorf("%s: read as a ticket", what)
		}
	}
	for _, junk := range []string{"", "!!", "AAAA", base64.URLEncoding.EncodeToString([]byte{0x12, 0xff})} {
		if _, err := ReadTicket(junk, []string{pub(dev)}); err == nil {
			t.Errorf("%q: read as a ticket", junk)
		}
	}
}
