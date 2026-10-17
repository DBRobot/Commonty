package ident

// The old format (v1), accepted during the move to envelopes
// (docs/dev/identity-state.md): its signatures are over serde_json's output
// of the Rust structs (client/identity), and the log keeps items with their
// keys sorted, so the signed bytes are not in the log. They are rebuilt here,
// exactly: struct field order, `skip_serializing_if` fields left out when
// empty, nested JSON values with sorted keys, serde_json's string escaping.
// testdata/vectors.json holds bytes the Rust code made; v1_test.go holds this
// to them. Deleted once every entry is v2.

import (
	"bytes"
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"sort"
	"strconv"
	"strings"
)

// Entry is identity::Entry
type Entry struct {
	Name          string    `json:"name"`
	Root          string    `json:"root"`
	Recovery      string    `json:"recovery"`
	RecoveredFrom []string  `json:"recovered_from,omitempty"`
	Devices       []Device  `json:"devices"`
	Passkeys      []Passkey `json:"passkeys,omitempty"`
	Grant         *Grant    `json:"grant,omitempty"`
	Libraries     []Library `json:"libraries,omitempty"`
	Terms         *string   `json:"terms,omitempty"`
	Version       uint64    `json:"version"`
	Updated       uint64    `json:"updated"`
}

type Device struct {
	Fingerprint string `json:"fingerprint"`
	PublicKey   string `json:"public_key"`
	Added       uint64 `json:"added"`
}

type Passkey struct {
	ID         string          `json:"id"`
	Cred       json.RawMessage `json:"cred"`
	Added      uint64          `json:"added"`
	RPID       *string         `json:"rp_id,omitempty"`
	LibraryKey *string         `json:"library_key,omitempty"`
}

type Library struct {
	ID      string      `json:"id"`
	Keys    []SealedKey `json:"keys"`
	Readers []Reader    `json:"readers,omitempty"`
	Created uint64      `json:"created"`
}

type SealedKey struct {
	To     string `json:"to"`
	Sealed string `json:"sealed"`
}

type Reader struct {
	Name string      `json:"name"`
	Keys []SealedKey `json:"keys"`
}

type Invite struct {
	PublicKey string `json:"public_key"`
	Issued    uint64 `json:"issued"`
	Expires   uint64 `json:"expires"`
}

type SignedInvite struct {
	Invite    Invite `json:"invite"`
	Signer    string `json:"signer"`
	Signature string `json:"signature"`
}

type Grant struct {
	Invite   SignedInvite `json:"invite"`
	Redeemed uint64       `json:"redeemed"`
	Proof    string       `json:"proof"`
}

type SignedEntry struct {
	Entry             Entry   `json:"entry"`
	Signature         string  `json:"signature"`
	RecoverySignature *string `json:"recovery_signature,omitempty"`
}

type Leave struct {
	Name string `json:"name"`
	Root string `json:"root"`
	At   uint64 `json:"at"`
	Back bool   `json:"back,omitempty"`
}

type SignedLeave struct {
	Leave     Leave  `json:"leave"`
	Signature string `json:"signature"`
}

// w writes serde_json's compact form
type w struct{ bytes.Buffer }

// str: serde_json's escaping - the quote, the backslash and the control
// characters, the short forms where it has them; nothing else (not '/', not
// '<', not U+2028, not anything past ASCII)
func (b *w) str(s string) {
	b.WriteByte('"')
	for i := 0; i < len(s); i++ {
		c := s[i]
		switch c {
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		case '\b':
			b.WriteString(`\b`)
		case '\f':
			b.WriteString(`\f`)
		case '\n':
			b.WriteString(`\n`)
		case '\r':
			b.WriteString(`\r`)
		case '\t':
			b.WriteString(`\t`)
		default:
			if c < 0x20 {
				fmt.Fprintf(b, `\u%04x`, c)
			} else {
				b.WriteByte(c)
			}
		}
	}
	b.WriteByte('"')
}

func (b *w) key(k string) {
	b.str(k)
	b.WriteByte(':')
}

func (b *w) u(n uint64) { b.WriteString(strconv.FormatUint(n, 10)) }

func (b *w) strs(xs []string) {
	b.WriteByte('[')
	for i, x := range xs {
		if i > 0 {
			b.WriteByte(',')
		}
		b.str(x)
	}
	b.WriteByte(']')
}

// value writes a JSON value the way serde_json writes a serde_json::Value:
// objects with their keys sorted
func (b *w) value(raw json.RawMessage) error {
	d := json.NewDecoder(bytes.NewReader(raw))
	d.UseNumber()
	var v any
	if err := d.Decode(&v); err != nil {
		return err
	}
	return b.any(v)
}

func (b *w) any(v any) error {
	switch x := v.(type) {
	case nil:
		b.WriteString("null")
	case bool:
		if x {
			b.WriteString("true")
		} else {
			b.WriteString("false")
		}
	case string:
		b.str(x)
	case json.Number:
		s, err := number(x)
		if err != nil {
			return err
		}
		b.WriteString(s)
	case []any:
		b.WriteByte('[')
		for i, y := range x {
			if i > 0 {
				b.WriteByte(',')
			}
			if err := b.any(y); err != nil {
				return err
			}
		}
		b.WriteByte(']')
	case map[string]any:
		keys := make([]string, 0, len(x))
		for k := range x {
			keys = append(keys, k)
		}
		sort.Strings(keys)
		b.WriteByte('{')
		for i, k := range keys {
			if i > 0 {
				b.WriteByte(',')
			}
			b.key(k)
			if err := b.any(x[k]); err != nil {
				return err
			}
		}
		b.WriteByte('}')
	default:
		return fmt.Errorf("not a json value: %T", v)
	}
	return nil
}

// number as serde_json reads and writes it: an integer that fits stays as
// written; anything else becomes an f64 and is written the shortest way that
// reads back, with a ".0" when it is whole
func number(n json.Number) (string, error) {
	s := string(n)
	if !strings.ContainsAny(s, ".eE") {
		if _, err := strconv.ParseInt(s, 10, 64); err == nil {
			return s, nil
		}
		if _, err := strconv.ParseUint(s, 10, 64); err == nil {
			return s, nil
		}
	}
	f, err := strconv.ParseFloat(s, 64)
	if err != nil || math.IsInf(f, 0) || math.IsNaN(f) {
		return "", errors.New("a number serde_json would not read")
	}
	out := strconv.FormatFloat(f, 'g', -1, 64)
	if !strings.ContainsAny(out, ".e") {
		out += ".0"
	}
	return out, nil
}

// Canonical is identity::canonical: the bytes an entry's v1 signature is over
func (e *Entry) Canonical() ([]byte, error) {
	var b w
	b.WriteByte('{')
	b.key("name")
	b.str(e.Name)
	b.WriteByte(',')
	b.key("root")
	b.str(e.Root)
	b.WriteByte(',')
	b.key("recovery")
	b.str(e.Recovery)
	if len(e.RecoveredFrom) > 0 {
		b.WriteByte(',')
		b.key("recovered_from")
		b.strs(e.RecoveredFrom)
	}
	b.WriteByte(',')
	b.key("devices")
	b.WriteByte('[')
	for i, d := range e.Devices {
		if i > 0 {
			b.WriteByte(',')
		}
		b.WriteByte('{')
		b.key("fingerprint")
		b.str(d.Fingerprint)
		b.WriteByte(',')
		b.key("public_key")
		b.str(d.PublicKey)
		b.WriteByte(',')
		b.key("added")
		b.u(d.Added)
		b.WriteByte('}')
	}
	b.WriteByte(']')
	if len(e.Passkeys) > 0 {
		b.WriteByte(',')
		b.key("passkeys")
		b.WriteByte('[')
		for i, p := range e.Passkeys {
			if i > 0 {
				b.WriteByte(',')
			}
			b.WriteByte('{')
			b.key("id")
			b.str(p.ID)
			b.WriteByte(',')
			b.key("cred")
			if err := b.value(p.Cred); err != nil {
				return nil, fmt.Errorf("passkey %s: %w", p.ID, err)
			}
			b.WriteByte(',')
			b.key("added")
			b.u(p.Added)
			if p.RPID != nil {
				b.WriteByte(',')
				b.key("rp_id")
				b.str(*p.RPID)
			}
			if p.LibraryKey != nil {
				b.WriteByte(',')
				b.key("library_key")
				b.str(*p.LibraryKey)
			}
			b.WriteByte('}')
		}
		b.WriteByte(']')
	}
	if e.Grant != nil {
		b.WriteByte(',')
		b.key("grant")
		g := e.Grant
		b.WriteByte('{')
		b.key("invite")
		b.signedInvite(&g.Invite)
		b.WriteByte(',')
		b.key("redeemed")
		b.u(g.Redeemed)
		b.WriteByte(',')
		b.key("proof")
		b.str(g.Proof)
		b.WriteByte('}')
	}
	if len(e.Libraries) > 0 {
		b.WriteByte(',')
		b.key("libraries")
		b.WriteByte('[')
		for i, l := range e.Libraries {
			if i > 0 {
				b.WriteByte(',')
			}
			b.WriteByte('{')
			b.key("id")
			b.str(l.ID)
			b.WriteByte(',')
			b.key("keys")
			b.sealed(l.Keys)
			if len(l.Readers) > 0 {
				b.WriteByte(',')
				b.key("readers")
				b.WriteByte('[')
				for j, r := range l.Readers {
					if j > 0 {
						b.WriteByte(',')
					}
					b.WriteByte('{')
					b.key("name")
					b.str(r.Name)
					b.WriteByte(',')
					b.key("keys")
					b.sealed(r.Keys)
					b.WriteByte('}')
				}
				b.WriteByte(']')
			}
			b.WriteByte(',')
			b.key("created")
			b.u(l.Created)
			b.WriteByte('}')
		}
		b.WriteByte(']')
	}
	if e.Terms != nil {
		b.WriteByte(',')
		b.key("terms")
		b.str(*e.Terms)
	}
	b.WriteByte(',')
	b.key("version")
	b.u(e.Version)
	b.WriteByte(',')
	b.key("updated")
	b.u(e.Updated)
	b.WriteByte('}')
	return b.Bytes(), nil
}

func (b *w) sealed(ks []SealedKey) {
	b.WriteByte('[')
	for i, k := range ks {
		if i > 0 {
			b.WriteByte(',')
		}
		b.WriteByte('{')
		b.key("to")
		b.str(k.To)
		b.WriteByte(',')
		b.key("sealed")
		b.str(k.Sealed)
		b.WriteByte('}')
	}
	b.WriteByte(']')
}

func (b *w) invite(i *Invite) {
	b.WriteByte('{')
	b.key("public_key")
	b.str(i.PublicKey)
	b.WriteByte(',')
	b.key("issued")
	b.u(i.Issued)
	b.WriteByte(',')
	b.key("expires")
	b.u(i.Expires)
	b.WriteByte('}')
}

func (b *w) signedInvite(s *SignedInvite) {
	b.WriteByte('{')
	b.key("invite")
	b.invite(&s.Invite)
	b.WriteByte(',')
	b.key("signer")
	b.str(s.Signer)
	b.WriteByte(',')
	b.key("signature")
	b.str(s.Signature)
	b.WriteByte('}')
}

// InviteBytes is what the release key signs for an invite
func (i *Invite) Bytes() []byte {
	var b w
	b.invite(i)
	return b.Bytes()
}

// Bytes is what a leave's root signs (under leave_challenge's label)
func (l *Leave) Bytes() []byte {
	var b w
	b.WriteByte('{')
	b.key("name")
	b.str(l.Name)
	b.WriteByte(',')
	b.key("root")
	b.str(l.Root)
	b.WriteByte(',')
	b.key("at")
	b.u(l.At)
	if l.Back {
		b.WriteByte(',')
		b.key("back")
		b.WriteString("true")
	}
	b.WriteByte('}')
	return b.Bytes()
}

// ProofBytes is identity::proof_bytes: what an invite's code key signs
func ProofBytes(invitePublicKey, root string, redeemed uint64) []byte {
	var b w
	b.WriteString(`{"invite":`)
	b.str(invitePublicKey)
	b.WriteString(`,"root":`)
	b.str(GrantSubject(root))
	b.WriteString(`,"redeemed":`)
	b.u(redeemed)
	b.WriteByte('}')
	return b.Bytes()
}

// CanonicalValue writes any JSON as serde_json writes a serde_json::Value
// (keys sorted): the gate's item hash is over these bytes
func CanonicalValue(raw json.RawMessage) ([]byte, error) {
	var b w
	if err := b.value(raw); err != nil {
		return nil, err
	}
	return b.Bytes(), nil
}

// strict reads JSON into v as serde would: unknown fields are ignored, but
// a field of the wrong type is an error
func strict(raw []byte, v any) error {
	return json.Unmarshal(raw, v)
}

// ItemHash is the gate's item_hash: sha256 hex of the item as serde_json
// writes a Value - the same whichever way the log escaped it
func ItemHash(raw json.RawMessage) string {
	b, err := CanonicalValue(raw)
	if err != nil {
		b = raw
	}
	h := sha256.Sum256(b)
	return hexs(h[:])
}
