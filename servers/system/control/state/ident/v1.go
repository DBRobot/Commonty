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
	RemovedBoxes  []string  `json:"removed_boxes,omitempty"` // carried as signed; ownership is enlistment's
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
	// may write too (a calendar two people keep); serde leaves it out when false
	Writes bool `json:"writes,omitempty"`
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

// number as serde_json writes an integer it read: as written. Anything
// else (a fraction, an exponent, past 64 bits) is refused: rebuilding
// serde_json's float formatting exactly is not worth the risk, and nothing a
// person signs carries one. Stricter than the gate, never looser.
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
	return "", errors.New("a number that is not a 64-bit integer")
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
					if r.Writes {
						b.WriteByte(',')
						b.key("writes")
						b.WriteString("true")
					}
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
func LogName(raw json.RawMessage) string {
	b, err := CanonicalValue(raw)
	if err != nil {
		b = raw
	}
	h := sha256.Sum256(b)
	return hexs(h[:])
}

// ItemHash is an item's hash: sha256 of what was signed and the signatures
// over it, so one signed item has one hash however its outer JSON is written
// (docs/dev/identity-state.md, "Item hash"; identity::item_hash in Rust is
// the same rule, held to it by the vectors). Outcomes are keyed on it, and a
// gate refuses an item already judged by it. The signatures are in it so a
// forgery over the same body never shares the real item's hash.
//   - a v2 envelope: Message(kind, body), NUL sig[, NUL recovery_sig]
//   - a v1 entry: its canonical entry, NUL signature[, NUL
//     recovery_signature]; an invite: the invite, NUL signature; a leave:
//     the leave, NUL signature - each as serde writes it
//   - anything else (an ack, or none of these): LogName's bytes
func ItemHash(raw json.RawMessage) string {
	if b := signedBytes(raw); b != nil {
		h := sha256.Sum256(b)
		return hexs(h[:])
	}
	return LogName(raw)
}

func signedBytes(raw json.RawMessage) []byte {
	var top map[string]json.RawMessage
	if json.Unmarshal(raw, &top) != nil {
		return nil
	}
	if v, ok := top["v"]; ok {
		var n int
		if json.Unmarshal(v, &n) != nil || n != 2 {
			return nil
		}
		var e Envelope
		if json.Unmarshal(raw, &e) != nil {
			return nil
		}
		body, err := e.BodyBytes()
		if err != nil {
			return nil
		}
		sigs := []string{e.Sig}
		if e.RecoverySig != nil {
			sigs = append(sigs, *e.RecoverySig)
		}
		return withSigs(Message(e.Kind, body), sigs...)
	}
	if len(top) != 1 {
		return nil
	}
	for k, x := range top {
		switch k {
		case "entry":
			sg, err := FromV1(x)
			if err != nil {
				return nil
			}
			c, err := sg.Entry.Canonical()
			if err != nil {
				return nil
			}
			sigs := []string{sg.Sig}
			if sg.RecoverySig != nil {
				sigs = append(sigs, *sg.RecoverySig)
			}
			return withSigs(c, sigs...)
		case "invite":
			var si SignedInvite
			if strict(x, &si) != nil {
				return nil
			}
			return withSigs(si.Invite.Bytes(), si.Signature)
		case "leave":
			var sl SignedLeave
			if strict(x, &sl) != nil {
				return nil
			}
			return withSigs(sl.Leave.Bytes(), sl.Signature)
		}
	}
	return nil
}

func withSigs(b []byte, sigs ...string) []byte {
	out := append([]byte{}, b...)
	for _, s := range sigs {
		out = append(out, 0)
		out = append(out, s...)
	}
	return out
}

// noDuplicates refuses JSON with a key twice in one object, at any depth:
// serde refuses a struct's field twice, and Go's decoder would quietly take
// the last, so the two would read one item two ways
func noDuplicates(raw []byte) error {
	d := json.NewDecoder(bytes.NewReader(raw))
	d.UseNumber()
	type frame struct {
		obj  bool
		keys map[string]bool
		key  bool // next token in an object is a key
	}
	var st []*frame
	for {
		t, err := d.Token()
		if err != nil {
			if err.Error() == "EOF" {
				return nil
			}
			return err
		}
		var top *frame
		if len(st) > 0 {
			top = st[len(st)-1]
		}
		if top != nil && top.obj && top.key {
			k, ok := t.(string)
			if !ok {
				if dl, ok := t.(json.Delim); ok && dl == '}' {
					st = st[:len(st)-1]
					if len(st) > 0 {
						st[len(st)-1].key = st[len(st)-1].obj
					}
					continue
				}
				return errors.New("a key that is not a string")
			}
			if top.keys[k] {
				return fmt.Errorf("the key %q twice", k)
			}
			top.keys[k] = true
			top.key = false
			continue
		}
		switch dl := t.(type) {
		case json.Delim:
			switch dl {
			case '{':
				st = append(st, &frame{obj: true, keys: map[string]bool{}, key: true})
				continue
			case '[':
				st = append(st, &frame{})
				continue
			case '}', ']':
				st = st[:len(st)-1]
			}
		}
		// a value finished: an object's next token is a key again
		if len(st) > 0 && st[len(st)-1].obj {
			st[len(st)-1].key = true
		}
	}
}

// sameAs holds a parsed item to the JSON it was read from: what the rules
// judge (rebuilt) must be exactly what the item says (raw), as values -
// nothing the struct does not know, no key in another case, no null where
// the signer leaves a field out. Otherwise a signature could pass over
// bytes that say one thing while the item read says another.
// Strict and SameAs read a signed body the way every kind here is read:
// for the judges beside this one (state/computers)
func Strict(raw []byte, v any) error   { return strict(raw, v) }
func SameAs(raw, rebuilt []byte) error { return sameAs(raw, rebuilt) }
func NoDuplicates(raw []byte) error    { return noDuplicates(raw) }

func sameAs(raw, rebuilt []byte) error {
	if err := noDuplicates(raw); err != nil {
		return fmt.Errorf("not an item: %w", err)
	}
	a, err := CanonicalValue(raw)
	if err != nil {
		return fmt.Errorf("not an item: %w", err)
	}
	b, err := CanonicalValue(rebuilt)
	if err != nil {
		return fmt.Errorf("not an item: %w", err)
	}
	if !bytes.Equal(a, b) {
		return errors.New("not an item: it says more, or other, than what is signed in it")
	}
	return nil
}

// signedEntryBytes: a SignedEntry as serde writes it
func (s *Signed) v1Bytes() ([]byte, error) {
	c, err := s.Entry.Canonical()
	if err != nil {
		return nil, err
	}
	var b w
	b.WriteString(`{"entry":`)
	b.Write(c)
	b.WriteString(`,"signature":`)
	b.str(s.Sig)
	if s.RecoverySig != nil {
		b.WriteString(`,"recovery_signature":`)
		b.str(*s.RecoverySig)
	}
	b.WriteByte('}')
	return b.Bytes(), nil
}

// leaveItemBytes: a SignedLeave as serde writes it
func (s *SignedLeave) bytes() []byte {
	var b w
	b.WriteString(`{"leave":`)
	b.Write(s.Leave.Bytes())
	b.WriteString(`,"signature":`)
	b.str(s.Signature)
	b.WriteByte('}')
	return b.Bytes()
}

// inviteItemBytes: a SignedInvite as serde writes it
func (s *SignedInvite) bytes() []byte {
	var b w
	b.signedInvite(s)
	return b.Bytes()
}
