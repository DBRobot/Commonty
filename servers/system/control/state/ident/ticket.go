package ident

// A join ticket, as the judge reads it: the biscuit a member's device mints
// (client/auth device.rs mint_join_ticket) and their box joins with. Only
// its authority block counts - what the device signed - and only the facts
// and the one check a ticket has: user, ticket, purpose, device, and
// `check if time($t), $t < <expires>`. Anything the format allows beyond
// that (rules, a third party's block, a key other than ed25519) is no
// ticket. biscuit-auth 6's format (schema.proto, crypto/mod.rs), read with
// no library: the judge takes nothing it cannot read itself.

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/binary"
	"errors"
	"strings"
)

// Ticket is what a join ticket's device said
type Ticket struct {
	User    string
	ID      string
	Device  string // the public key that signed it, base64
	Expires uint64 // seconds: good while the time is below it
}

var biscuitSymbols = []string{
	"read", "write", "resource", "operation", "right", "time", "role", "owner", "tenant", "namespace",
	"user", "team", "service", "admin", "email", "group", "member", "ip_address", "client", "client_ip",
	"domain", "path", "version", "cluster", "node", "hostname", "nonce", "query",
}

const biscuitOffset = 1024

var errNoTicket = errors.New("not a join ticket")

// pbField is one field of a protobuf message: its number, its wire type,
// and its value (a varint, or the bytes of a length-delimited field)
type pbField struct {
	num  uint64
	wire uint64
	v    uint64
	b    []byte
}

// pbRead splits a message into its fields; anything but varints and
// length-delimited fields is no message of a ticket's
func pbRead(b []byte) ([]pbField, error) {
	var out []pbField
	for len(b) > 0 {
		tag, n := binary.Uvarint(b)
		if n <= 0 {
			return nil, errNoTicket
		}
		b = b[n:]
		f := pbField{num: tag >> 3, wire: tag & 7}
		switch f.wire {
		case 0:
			f.v, n = binary.Uvarint(b)
			if n <= 0 {
				return nil, errNoTicket
			}
			b = b[n:]
		case 2:
			l, n := binary.Uvarint(b)
			if n <= 0 || l > uint64(len(b)-n) {
				return nil, errNoTicket
			}
			f.b = b[n : n+int(l)]
			b = b[n+int(l):]
		default:
			return nil, errNoTicket
		}
		out = append(out, f)
	}
	return out, nil
}

// pbOne: the one field numbered num, or nil when there are none or several
func pbOne(fs []pbField, num uint64) *pbField {
	var got *pbField
	for i := range fs {
		if fs[i].num == num {
			if got != nil {
				return nil
			}
			got = &fs[i]
		}
	}
	return got
}

func pbAll(fs []pbField, num uint64) []pbField {
	var out []pbField
	for _, f := range fs {
		if f.num == num {
			out = append(out, f)
		}
	}
	return out
}

// term: a string's symbol (kind 's'), an integer ('i') or a variable ('v')
type term struct {
	kind byte
	v    uint64
}

func readTerm(b []byte) (term, error) {
	fs, err := pbRead(b)
	if err != nil || len(fs) != 1 || fs[0].wire != 0 {
		return term{}, errNoTicket
	}
	switch fs[0].num {
	case 1:
		return term{'v', fs[0].v}, nil
	case 2:
		return term{'i', fs[0].v}, nil
	case 3:
		return term{'s', fs[0].v}, nil
	}
	return term{}, errNoTicket
}

type predicate struct {
	name  uint64
	terms []term
}

func readPredicate(b []byte) (predicate, error) {
	fs, err := pbRead(b)
	if err != nil {
		return predicate{}, err
	}
	n := pbOne(fs, 1)
	if n == nil || n.wire != 0 {
		return predicate{}, errNoTicket
	}
	p := predicate{name: n.v}
	for _, f := range fs {
		switch {
		case f.num == 1:
		case f.num == 2 && f.wire == 2:
			t, err := readTerm(f.b)
			if err != nil {
				return predicate{}, err
			}
			p.terms = append(p.terms, t)
		default:
			return predicate{}, errNoTicket
		}
	}
	return p, nil
}

// expiry: the one shape of check a ticket's time is in,
// `check if time($t), $t < N`: one query, its body time($t), its one
// expression [$t, N, <]. Ok false for any other check.
func expiry(b []byte) (uint64, bool) {
	fs, err := pbRead(b)
	if err != nil {
		return 0, false
	}
	for _, f := range fs {
		if f.num == 2 && (f.wire != 0 || f.v != 0) || f.num != 1 && f.num != 2 {
			return 0, false
		}
	}
	q := pbOne(fs, 1)
	if q == nil || q.wire != 2 {
		return 0, false
	}
	rule, err := pbRead(q.b)
	if err != nil || len(pbAll(rule, 2)) != 1 || len(pbAll(rule, 3)) != 1 || len(pbAll(rule, 4)) != 0 {
		return 0, false
	}
	body, err := readPredicate(pbOne(rule, 2).b)
	if err != nil || body.name != 5 || len(body.terms) != 1 || body.terms[0].kind != 'v' {
		return 0, false
	}
	expr, err := pbRead(pbOne(rule, 3).b)
	if err != nil || len(expr) != 3 {
		return 0, false
	}
	var ops []pbField
	for _, e := range expr {
		if e.num != 1 || e.wire != 2 {
			return 0, false
		}
		op, err := pbRead(e.b)
		if err != nil || len(op) != 1 || op[0].wire != 2 {
			return 0, false
		}
		ops = append(ops, op[0])
	}
	if ops[0].num != 1 || ops[1].num != 1 || ops[2].num != 3 {
		return 0, false
	}
	v, err1 := readTerm(ops[0].b)
	n, err2 := readTerm(ops[1].b)
	lt, err3 := pbRead(ops[2].b)
	if err1 != nil || err2 != nil || err3 != nil || v != body.terms[0] || n.kind != 'i' || int64(n.v) <= 0 {
		return 0, false
	}
	// LessThan is kind 0, written or left out
	if len(lt) > 1 || len(lt) == 1 && (lt[0].num != 1 || lt[0].wire != 0 || lt[0].v != 0) {
		return 0, false
	}
	return n.v, true
}

func decodeToken(s string) ([]byte, error) {
	s = strings.TrimSpace(s)
	for _, enc := range []*base64.Encoding{base64.URLEncoding, base64.RawURLEncoding, base64.StdEncoding, base64.RawStdEncoding} {
		if b, err := enc.DecodeString(s); err == nil {
			return b, nil
		}
	}
	return nil, errNoTicket
}

// authority: the authority block's bytes and that it was signed by one of
// keys (ed25519, base64): the key that signed it
func authority(token []byte, keys []string) ([]byte, string, error) {
	top, err := pbRead(token)
	if err != nil {
		return nil, "", err
	}
	a := pbOne(top, 2)
	if a == nil || a.wire != 2 {
		return nil, "", errNoTicket
	}
	sb, err := pbRead(a.b)
	if err != nil {
		return nil, "", err
	}
	block, next, sig := pbOne(sb, 1), pbOne(sb, 2), pbOne(sb, 3)
	if block == nil || next == nil || sig == nil || block.wire != 2 || next.wire != 2 || sig.wire != 2 {
		return nil, "", errNoTicket
	}
	if len(pbAll(sb, 4)) > 0 {
		return nil, "", errors.New("a ticket signed by a third party")
	}
	version := uint64(0)
	if v := pbAll(sb, 5); len(v) == 1 && v[0].wire == 0 {
		version = v[0].v
	} else if len(v) > 0 {
		return nil, "", errNoTicket
	}
	nk, err := pbRead(next.b)
	if err != nil {
		return nil, "", err
	}
	alg, nkey := pbOne(nk, 1), pbOne(nk, 2)
	if alg == nil || nkey == nil || alg.wire != 0 || nkey.wire != 2 {
		return nil, "", errNoTicket
	}
	var algLE [4]byte
	binary.LittleEndian.PutUint32(algLE[:], uint32(alg.v))
	var msg []byte
	switch version {
	case 0:
		msg = append(append(append(msg, block.b...), algLE[:]...), nkey.b...)
	case 1:
		var ver [4]byte
		binary.LittleEndian.PutUint32(ver[:], 1)
		msg = append(msg, "\x00BLOCK\x00\x00VERSION\x00"...)
		msg = append(msg, ver[:]...)
		msg = append(msg, "\x00PAYLOAD\x00"...)
		msg = append(msg, block.b...)
		msg = append(msg, "\x00ALGORITHM\x00"...)
		msg = append(msg, algLE[:]...)
		msg = append(msg, "\x00NEXTKEY\x00"...)
		msg = append(msg, nkey.b...)
	default:
		return nil, "", errNoTicket
	}
	for _, k := range keys {
		pk, err := DecodePublic(k)
		if err == nil && len(sig.b) == ed25519.SignatureSize && ed25519.Verify(pk, msg, sig.b) {
			return block.b, k, nil
		}
	}
	return nil, "", errors.New("not signed by a device of theirs")
}

// ReadTicket: a join ticket signed by one of keys (a member's devices),
// what it says. Nothing about the time: the caller holds Expires to it.
func ReadTicket(token string, keys []string) (*Ticket, error) {
	raw, err := decodeToken(token)
	if err != nil {
		return nil, err
	}
	block, key, err := authority(raw, keys)
	if err != nil {
		return nil, err
	}
	return readBlock(block, key)
}

// TicketID: the id a ticket names, its signature unchecked - for keeping
// track of a ticket the judge has not been asked to trust yet
func TicketID(token string) (string, uint64) {
	raw, err := decodeToken(token)
	if err != nil {
		return "", 0
	}
	top, err := pbRead(raw)
	if err != nil {
		return "", 0
	}
	a := pbOne(top, 2)
	if a == nil {
		return "", 0
	}
	sb, err := pbRead(a.b)
	if err != nil || pbOne(sb, 1) == nil {
		return "", 0
	}
	t, err := readBlock(pbOne(sb, 1).b, "")
	if err != nil {
		return "", 0
	}
	return t.ID, t.Expires
}

func readBlock(block []byte, key string) (*Ticket, error) {
	fs, err := pbRead(block)
	if err != nil {
		return nil, err
	}
	var symbols []string
	for _, f := range fs {
		switch f.num {
		case 1:
			if f.wire != 2 {
				return nil, errNoTicket
			}
			symbols = append(symbols, string(f.b))
		case 2, 3, 4, 6:
		default:
			// rules, scopes, keys: a ticket has none
			return nil, errNoTicket
		}
	}
	sym := func(i uint64) (string, bool) {
		if i < biscuitOffset {
			if i < uint64(len(biscuitSymbols)) {
				return biscuitSymbols[i], true
			}
			return "", false
		}
		if i-biscuitOffset < uint64(len(symbols)) {
			return symbols[i-biscuitOffset], true
		}
		return "", false
	}
	said := map[string][]string{}
	for _, f := range pbAll(fs, 4) {
		if f.wire != 2 {
			return nil, errNoTicket
		}
		ff, err := pbRead(f.b)
		if err != nil {
			return nil, err
		}
		p := pbOne(ff, 1)
		if p == nil || len(ff) != 1 || p.wire != 2 {
			return nil, errNoTicket
		}
		pr, err := readPredicate(p.b)
		if err != nil {
			return nil, err
		}
		name, ok := sym(pr.name)
		if !ok {
			return nil, errNoTicket
		}
		if len(pr.terms) != 1 || pr.terms[0].kind != 's' {
			continue
		}
		v, ok := sym(pr.terms[0].v)
		if !ok {
			return nil, errNoTicket
		}
		said[name] = append(said[name], v)
	}
	one := func(k string) string {
		if len(said[k]) != 1 {
			return ""
		}
		return said[k][0]
	}
	t := &Ticket{User: one("user"), ID: one("ticket"), Device: key}
	if t.User == "" || one("purpose") != "join" || !ticketID(t.ID) {
		return nil, errNoTicket
	}
	for _, f := range pbAll(fs, 6) {
		if f.wire != 2 {
			return nil, errNoTicket
		}
		if exp, ok := expiry(f.b); ok && (t.Expires == 0 || exp < t.Expires) {
			t.Expires = exp
		}
	}
	if t.Expires == 0 {
		return nil, errors.New("a ticket good forever is no ticket")
	}
	return t, nil
}

// ticketID: 32 lowercase hex, as mint_join_ticket makes one
func ticketID(s string) bool {
	if len(s) != 32 {
		return false
	}
	for _, c := range s {
		if !(c >= '0' && c <= '9' || c >= 'a' && c <= 'f') {
			return false
		}
	}
	return true
}
