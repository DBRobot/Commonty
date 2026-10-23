package ident

// A person's signed items as the judge reads them, in either format, and the
// rules an entry is held to (identity::verify, identity::accept,
// identity::verify_grant, identity::verify_leave), moved here.

import (
	"crypto/sha256"
	"encoding/json"
	"errors"
	"fmt"
	"strings"
)

// Envelope is a v2 item (docs/dev/identity-state.md)
type Envelope struct {
	V           int     `json:"v"`
	Kind        string  `json:"kind"`
	Body        string  `json:"body"`
	Sig         string  `json:"sig"`
	RecoverySig *string `json:"recovery_sig,omitempty"`
}

// Message is what a v2 item's signer signed
func Message(kind string, body []byte) []byte {
	m := []byte("commonty " + kind + " v2\x00")
	return append(m, body...)
}

// BodyBytes: the body as signed. Says nothing of the signature.
func (e *Envelope) BodyBytes() ([]byte, error) {
	b, err := b64.DecodeString(e.Body)
	if err != nil {
		return nil, errors.New("the body is not base64")
	}
	return b, nil
}

// Signed is one entry as signed, whichever format it came in: what its
// signatures are over, and the entry read from it
type Signed struct {
	Entry Entry
	// v1: the signature over the entry's serde bytes (or the passkey
	// assertion over its challenge); v2: over Message("entry", body)
	Sig         string
	RecoverySig *string
	// v2's body as signed; nil for v1
	body []byte
	// the item as agreed, verbatim
	Raw json.RawMessage
}

func (s *Signed) V2() bool { return s.body != nil }

// message: what an ed25519 root or recovery key signed for this entry
func (s *Signed) message() ([]byte, error) {
	if s.V2() {
		return Message("entry", s.body), nil
	}
	return s.Entry.Canonical()
}

// challenge: what a passkey root's assertion is over for this entry
func (s *Signed) challenge() ([]byte, error) {
	if s.V2() {
		h := sha256.Sum256(Message("entry", s.body))
		return h[:], nil
	}
	c, err := s.Entry.Canonical()
	if err != nil {
		return nil, err
	}
	h := sha256.New()
	h.Write([]byte("commonty entry v1\x00"))
	h.Write(c)
	return h.Sum(nil), nil
}

// FromV1 reads a v1 SignedEntry, as the log keeps it
func FromV1(raw json.RawMessage) (*Signed, error) {
	var se SignedEntry
	if err := strict(raw, &se); err != nil {
		return nil, err
	}
	sg := &Signed{Entry: se.Entry, Sig: se.Signature, RecoverySig: se.RecoverySignature, Raw: raw}
	rebuilt, err := sg.v1Bytes()
	if err != nil {
		return nil, err
	}
	if err := sameAs(raw, rebuilt); err != nil {
		return nil, err
	}
	return sg, nil
}

// FromEnvelope reads a v2 entry envelope. The body is read only to know the
// entry; nothing counts until Verify passes.
func FromEnvelope(raw json.RawMessage) (*Signed, error) {
	env, err := ReadEnvelope(raw)
	if err != nil {
		return nil, err
	}
	if env.Kind != "entry" {
		return nil, errors.New("not a v2 entry")
	}
	body, err := env.BodyBytes()
	if err != nil {
		return nil, err
	}
	s := &Signed{Sig: env.Sig, RecoverySig: env.RecoverySig, body: body, Raw: raw}
	if err := strict(body, &s.Entry); err != nil {
		return nil, err
	}
	// the body says exactly the entry judged, nothing more or other
	c, err := s.Entry.Canonical()
	if err != nil {
		return nil, err
	}
	if err := sameAs(body, c); err != nil {
		return nil, err
	}
	return s, nil
}

func checkWithKey(sig string, s *Signed, key string) error {
	m, err := s.message()
	if err != nil {
		return err
	}
	return VerifyEd25519(key, m, sig)
}

// assertionWith is check_assertion_with: the passkey the root names, taken
// from `trusted` (what is on file), answering this entry's challenge
func assertionWith(s *Signed, trusted []Passkey) error {
	id, digest, ok := RootParts(s.Entry.Root)
	if !ok {
		return errors.New("bad key: a passkey root must commit to its key")
	}
	var p *Passkey
	for i := range trusted {
		if trusted[i].ID == id {
			p = &trusted[i]
			break
		}
	}
	if p == nil {
		return errors.New("bad key: the root names a passkey the entry does not carry")
	}
	if CoseDigest(p.Cred) != digest {
		return errors.New("bad key: the root does not name this passkey's key")
	}
	a, err := ReadAssertion(s.Sig)
	if err != nil {
		return err
	}
	c, err := s.challenge()
	if err != nil {
		return err
	}
	return AssertionOver(p, a, c)
}

// Verify is identity::verify: signed by the root it names
func (s *Signed) Verify() error {
	if strings.HasPrefix(s.Entry.Root, WebauthnRoot) {
		return assertionWith(s, s.Entry.Passkeys)
	}
	return checkWithKey(s.Sig, s, s.Entry.Root)
}

// ReservedNames is identity::RESERVED_NAMES: the boxes' own identities on
// the forge, which no person is
var ReservedNames = []string{"commonty-box", "commonty-ci"}

// ReservedName is identity::reserved_name
func ReservedName(n string) bool {
	for _, r := range ReservedNames {
		if n == r {
			return true
		}
	}
	return false
}

// ValidName is identity::valid_name
func ValidName(n string) bool {
	if n == "" || len(n) > 64 || strings.HasPrefix(n, ".") {
		return false
	}
	for _, c := range n {
		ok := (c >= 'a' && c <= 'z') || (c >= '0' && c <= '9') || c == '-' || c == '_' || c == '.'
		if !ok {
			return false
		}
	}
	return true
}

// ValidLibraryID is identity::valid_library_id
func ValidLibraryID(s string) bool {
	if len(s) != 32 {
		return false
	}
	for _, c := range []byte(s) {
		if !((c >= '0' && c <= '9') || (c >= 'a' && c <= 'f') || (c >= 'A' && c <= 'F')) {
			return false
		}
	}
	return true
}

// Rejected is an entry the rules refuse, with the rules' reason
type Rejected string

func (r Rejected) Error() string { return string(r) }

// Accept is identity::accept: may `next` stand in place of `cur` (nil: a
// first sight)?
func Accept(cur, next *Signed) error {
	n := &next.Entry
	if !ValidName(n.Name) {
		return Rejected("bad name")
	}
	if ReservedName(n.Name) {
		return Rejected("that name is the boxes'")
	}
	if len(n.Devices) == 0 && !strings.HasPrefix(n.Root, WebauthnRoot) {
		return Rejected("an entry needs at least one device")
	}
	for _, d := range n.Devices {
		if _, err := DecodePublic(d.PublicKey); err != nil {
			return err
		}
	}
	for _, l := range n.Libraries {
		if !ValidLibraryID(l.ID) {
			return Rejected(fmt.Sprintf("%q is not a library id", l.ID))
		}
	}
	if cur == nil {
		if len(n.RecoveredFrom) > 0 {
			return Rejected("a first entry has no earlier roots")
		}
		return next.Verify()
	}
	o := &cur.Entry
	if strings.HasPrefix(o.Root, WebauthnRoot) && n.Root == o.Root {
		id, _, ok := RootParts(o.Root)
		if !ok {
			return errors.New("bad key: a passkey root must commit to its key")
		}
		var was, now *Passkey
		for i := range o.Passkeys {
			if o.Passkeys[i].ID == id {
				was = &o.Passkeys[i]
				break
			}
		}
		for i := range n.Passkeys {
			if n.Passkeys[i].ID == id {
				now = &n.Passkeys[i]
				break
			}
		}
		switch {
		case was == nil:
			return errors.New("bad key: the entry on file has no root passkey")
		case now == nil || !sameJSON(was.Cred, now.Cred):
			return Rejected("the root's passkey may not be changed or dropped")
		}
		if err := assertionWith(next, o.Passkeys); err != nil {
			return err
		}
	} else if err := next.Verify(); err != nil {
		return err
	}
	if n.Name != o.Name {
		return Rejected("name mismatch")
	}
	if n.Version <= o.Version {
		return Rejected(fmt.Sprintf("version %d is not newer than %d", n.Version, o.Version))
	}
	earlier := append([]string(nil), o.RecoveredFrom...)
	if n.Root != o.Root {
		earlier = append(earlier, o.Root)
	}
	if !sameStrings(n.RecoveredFrom, earlier) {
		return Rejected("the earlier roots are not the ones on file")
	}
	recoveryChanged := n.Recovery != o.Recovery && o.Recovery != ""
	if n.Root == o.Root && !recoveryChanged {
		return nil
	}
	if next.RecoverySig == nil {
		if n.Root == o.Root {
			return Rejected("only the recovery key may change the recovery key")
		}
		return Rejected("only the recovery key may change the root")
	}
	if o.Recovery == "" {
		return Rejected("a passkey root has no recovery key")
	}
	return checkWithKey(*next.RecoverySig, next, o.Recovery)
}

// sameJSON: two JSON values equal as serde_json::Value compares them
func sameJSON(a, b json.RawMessage) bool {
	ca, err1 := CanonicalValue(a)
	cb, err2 := CanonicalValue(b)
	return err1 == nil && err2 == nil && string(ca) == string(cb)
}

func sameStrings(a, b []string) bool {
	if len(a) != len(b) {
		return false
	}
	for i := range a {
		if a[i] != b[i] {
			return false
		}
	}
	return true
}

// VerifyInvite is identity::verify_invite: signed by the release key named,
// and no other. v2 invites sign Message("invite", body).
func VerifyInvite(s *SignedInvite, release string) error {
	if s.Signer != release {
		return Rejected("invite is not signed by the release key")
	}
	return VerifyEd25519(release, s.Invite.Bytes(), s.Signature)
}

// VerifyGrant is identity::verify_grant
func VerifyGrant(e *Entry, release string) error {
	g := e.Grant
	if g == nil {
		return Rejected("no grant")
	}
	if err := VerifyInvite(&g.Invite, release); err != nil {
		return err
	}
	inv := &g.Invite.Invite
	if g.Redeemed < inv.Issued || g.Redeemed > inv.Expires {
		return Rejected("redeemed outside the invite's window")
	}
	for _, root := range append([]string{e.Root}, e.RecoveredFrom...) {
		if VerifyEd25519(inv.PublicKey, ProofBytes(inv.PublicKey, root, g.Redeemed), g.Proof) == nil {
			return nil
		}
	}
	if _, err := DecodePublic(inv.PublicKey); err != nil {
		return err
	}
	return ErrSignature
}

// LeaveChallenge is identity::leave_challenge
func LeaveChallenge(l *Leave) []byte {
	h := sha256.New()
	h.Write([]byte("commonty leave v1\x00"))
	h.Write(l.Bytes())
	return h.Sum(nil)
}

// VerifyLeave is identity::verify_leave: signed by the root of the entry on
// file; a passkey root's key from that entry, never from the statement
func VerifyLeave(s *SignedLeave, onFile *Entry) error {
	if s.Leave.Name != onFile.Name {
		return Rejected("that statement is someone else's")
	}
	if s.Leave.Root != onFile.Root {
		return Rejected("not signed by the root on file")
	}
	expected := LeaveChallenge(&s.Leave)
	if strings.HasPrefix(onFile.Root, WebauthnRoot) {
		id, digest, ok := RootParts(onFile.Root)
		if !ok {
			return errors.New("bad key: a passkey root must commit to its key")
		}
		for i := range onFile.Passkeys {
			p := &onFile.Passkeys[i]
			if p.ID == id && CoseDigest(p.Cred) == digest {
				a, err := ReadAssertion(s.Signature)
				if err != nil {
					return err
				}
				return AssertionOver(p, a, expected)
			}
		}
		return errors.New("bad key: the entry on file has no root passkey")
	}
	return VerifyEd25519(onFile.Root, expected, s.Signature)
}

// VerifyRoot checks a v2 statement (leave, enlist, unenlist) against the
// entry on file's root: ed25519 over the message, or the root passkey's
// assertion over its sha256
func VerifyRoot(env *Envelope, onFile *Entry) error {
	body, err := env.BodyBytes()
	if err != nil {
		return err
	}
	m := Message(env.Kind, body)
	if strings.HasPrefix(onFile.Root, WebauthnRoot) {
		id, digest, ok := RootParts(onFile.Root)
		if !ok {
			return errors.New("bad key: a passkey root must commit to its key")
		}
		for i := range onFile.Passkeys {
			p := &onFile.Passkeys[i]
			if p.ID == id && CoseDigest(p.Cred) == digest {
				a, err := ReadAssertion(env.Sig)
				if err != nil {
					return err
				}
				h := sha256.Sum256(m)
				return AssertionOver(p, a, h[:])
			}
		}
		return errors.New("bad key: the entry on file has no root passkey")
	}
	return VerifyEd25519(onFile.Root, m, env.Sig)
}

// ReadEnvelope reads a v2 envelope, holding it to its JSON
func ReadEnvelope(raw json.RawMessage) (*Envelope, error) {
	var env Envelope
	if err := strict(raw, &env); err != nil {
		return nil, err
	}
	if env.V != 2 {
		return nil, errors.New("not a v2 item")
	}
	again, _ := json.Marshal(&env)
	if err := sameAs(raw, again); err != nil {
		return nil, err
	}
	return &env, nil
}
