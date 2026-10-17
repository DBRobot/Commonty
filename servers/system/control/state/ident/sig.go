package ident

// The signature checks a person's items are held to: an ed25519 root, a
// passkey root's WebAuthn assertion, the release key's invite, an invite
// code's proof. Each matches client/identity (testdata/vectors.json).

import (
	"bytes"
	"crypto"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/elliptic"
	"crypto/rsa"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"math/big"
	"strings"
)

var (
	ErrSignature = errors.New("bad signature")
	b64          = base64.StdEncoding
	b64url       = base64.RawURLEncoding
)

const WebauthnRoot = "webauthn:"

// DecodePublic is identity::decode_public: base64 of 32 bytes
func DecodePublic(s string) (ed25519.PublicKey, error) {
	b, err := b64.DecodeString(strings.TrimSpace(s))
	if err != nil {
		return nil, errors.New("bad key: " + err.Error())
	}
	if len(b) != ed25519.PublicKeySize {
		return nil, errors.New("bad key: public key is not 32 bytes")
	}
	return ed25519.PublicKey(b), nil
}

// VerifyEd25519 checks a base64 signature over msg by a base64 key
func VerifyEd25519(keyB64 string, msg []byte, sigB64 string) error {
	k, err := DecodePublic(keyB64)
	if err != nil {
		return err
	}
	sig, err := b64.DecodeString(sigB64)
	if err != nil || len(sig) != ed25519.SignatureSize {
		return ErrSignature
	}
	if !ed25519.Verify(k, msg, sig) {
		return ErrSignature
	}
	return nil
}

// MemberID is identity::member_id: sha256 hex of the root string
func MemberID(root string) string {
	h := sha256.Sum256([]byte(root))
	return hexs(h[:])
}

func hexs(b []byte) string {
	const d = "0123456789abcdef"
	out := make([]byte, 2*len(b))
	for i, c := range b {
		out[2*i], out[2*i+1] = d[c>>4], d[c&15]
	}
	return string(out)
}

// MemberIDs is identity::member_ids: the root's, then the roots recovered
// from, newest first
func (e *Entry) MemberIDs() []string {
	ids := []string{MemberID(e.Root)}
	for i := len(e.RecoveredFrom) - 1; i >= 0; i-- {
		ids = append(ids, MemberID(e.RecoveredFrom[i]))
	}
	return ids
}

// RootParts splits a passkey root, `webauthn:<id>:<digest>`
func RootParts(root string) (id, digest string, ok bool) {
	rest, ok := strings.CutPrefix(root, WebauthnRoot)
	if !ok {
		return "", "", false
	}
	return strings.Cut(rest, ":")
}

// GrantSubject is identity::grant_subject: a passkey root without its key
// digest
func GrantSubject(root string) string {
	id, _, ok := RootParts(root)
	if !ok {
		return root
	}
	return WebauthnRoot + id
}

// coseKey is the key inside a credential as webauthn-rs serialises it
func coseKey(cred json.RawMessage) (json.RawMessage, error) {
	var c struct {
		Cred struct {
			Cred json.RawMessage `json:"cred"`
		} `json:"cred"`
	}
	if err := json.Unmarshal(cred, &c); err != nil || len(c.Cred.Cred) == 0 {
		return nil, errors.New("bad key: the passkey has no key")
	}
	return c.Cred.Cred, nil
}

// CoseDigest is identity's cose_digest: base64url sha256 of the key's json,
// as serde_json writes a Value. A missing key hashes as null, as in Rust.
func CoseDigest(cred json.RawMessage) string {
	k, err := coseKey(cred)
	if err != nil {
		k = json.RawMessage("null")
	}
	b, err := CanonicalValue(k)
	if err != nil {
		b = []byte("null")
	}
	h := sha256.Sum256(b)
	return b64url.EncodeToString(h[:])
}

// PasskeyRoot is identity::passkey_root
func PasskeyRoot(id string, cred json.RawMessage) string {
	return WebauthnRoot + id + ":" + CoseDigest(cred)
}

// Assertion is a WebAuthn assertion as a browser hands it over
type Assertion struct {
	AuthenticatorData string `json:"authenticatorData"`
	ClientDataJSON    string `json:"clientDataJSON"`
	Signature         string `json:"signature"`
}

// ReadAssertion: base64 of the assertion's json, as signatures carry one
func ReadAssertion(sigB64 string) (*Assertion, error) {
	raw, err := b64.DecodeString(sigB64)
	if err != nil {
		return nil, ErrSignature
	}
	var a Assertion
	if err := json.Unmarshal(raw, &a); err != nil {
		return nil, ErrSignature
	}
	return &a, nil
}

// AssertionOver is identity's assertion_over: `a` made by `p` over `expected`
// (the challenge), by a person present, for the relying party on record
func AssertionOver(p *Passkey, a *Assertion, expected []byte) error {
	auth, err := b64url.DecodeString(a.AuthenticatorData)
	if err != nil {
		return ErrSignature
	}
	client, err := b64url.DecodeString(a.ClientDataJSON)
	if err != nil {
		return ErrSignature
	}
	sig, err := b64url.DecodeString(a.Signature)
	if err != nil {
		return ErrSignature
	}
	h := sha256.Sum256(client)
	data := append(append([]byte(nil), auth...), h[:]...)
	k, err := coseKey(p.Cred)
	if err != nil {
		return err
	}
	if err := coseVerify(k, sig, data); err != nil {
		return err
	}
	var c struct {
		Type      *string `json:"type"`
		Challenge *string `json:"challenge"`
		Origin    *string `json:"origin"`
	}
	if err := json.Unmarshal(client, &c); err != nil {
		return ErrSignature
	}
	if c.Type == nil || *c.Type != "webauthn.get" {
		return ErrSignature
	}
	if c.Challenge == nil || *c.Challenge != b64url.EncodeToString(expected) {
		return ErrSignature
	}
	origin := ""
	if c.Origin != nil {
		origin = *c.Origin
	}
	var rp *string
	if p.RPID != nil {
		rp = p.RPID
	}
	if !presenceAndPlace(auth, origin, rp) {
		return ErrSignature
	}
	return nil
}

// presenceAndPlace is identity's presence_and_place
func presenceAndPlace(auth []byte, origin string, rpID *string) bool {
	if len(auth) < 37 || auth[32]&0x01 == 0 {
		return false
	}
	host, ok := strings.CutPrefix(origin, "https://")
	if !ok {
		return false
	}
	if i := strings.IndexAny(host, "/:"); i >= 0 {
		host = host[:i]
	}
	within := func(rp string) bool { return host == rp || strings.HasSuffix(host, "."+rp) }
	hashed := func(rp string) bool {
		h := sha256.Sum256([]byte(rp))
		return bytes.Equal(h[:], auth[:32])
	}
	if rpID != nil {
		return within(*rpID) && hashed(*rpID)
	}
	labels := strings.Split(host, ".")
	for i := 0; i < len(labels)-1; i++ {
		if hashed(strings.Join(labels[i:], ".")) {
			return true
		}
	}
	return false
}

// coseVerify checks a signature with a key as webauthn-rs serialises its
// COSEKey: ES256/384/512 over P-256/384/521, EdDSA over Ed25519, RS256
func coseVerify(raw json.RawMessage, sig, data []byte) error {
	var k struct {
		Type string `json:"type_"`
		Key  struct {
			EC2 *struct {
				Curve string `json:"curve"`
				X     string `json:"x"`
				Y     string `json:"y"`
			} `json:"EC_EC2"`
			OKP *struct {
				Curve string `json:"curve"`
				X     string `json:"x"`
			} `json:"EC_OKP"`
			RSA *struct {
				N string `json:"n"`
				E string `json:"e"`
			} `json:"RSA"`
		} `json:"key"`
	}
	if err := json.Unmarshal(raw, &k); err != nil {
		return errors.New("bad key: passkey public key: " + err.Error())
	}
	switch {
	case k.Key.EC2 != nil:
		var curve elliptic.Curve
		var hash crypto.Hash
		switch k.Type + "/" + k.Key.EC2.Curve {
		case "ES256/SECP256R1":
			curve, hash = elliptic.P256(), crypto.SHA256
		case "ES384/SECP384R1":
			curve, hash = elliptic.P384(), crypto.SHA384
		case "ES512/SECP521R1":
			curve, hash = elliptic.P521(), crypto.SHA512
		default:
			return ErrSignature
		}
		x, err1 := b64url.DecodeString(k.Key.EC2.X)
		y, err2 := b64url.DecodeString(k.Key.EC2.Y)
		if err1 != nil || err2 != nil {
			return ErrSignature
		}
		pub := &ecdsa.PublicKey{Curve: curve, X: new(big.Int).SetBytes(x), Y: new(big.Int).SetBytes(y)}
		if !curve.IsOnCurve(pub.X, pub.Y) {
			return ErrSignature
		}
		h := hash.New()
		h.Write(data)
		if !ecdsa.VerifyASN1(pub, h.Sum(nil), sig) {
			return ErrSignature
		}
		return nil
	case k.Key.OKP != nil:
		if k.Type != "EDDSA" || k.Key.OKP.Curve != "ED25519" {
			return ErrSignature
		}
		x, err := b64url.DecodeString(k.Key.OKP.X)
		if err != nil || len(x) != ed25519.PublicKeySize {
			return ErrSignature
		}
		if !ed25519.Verify(ed25519.PublicKey(x), data, sig) {
			return ErrSignature
		}
		return nil
	case k.Key.RSA != nil:
		if k.Type != "RS256" {
			return ErrSignature
		}
		n, err1 := b64url.DecodeString(k.Key.RSA.N)
		e, err2 := b64url.DecodeString(k.Key.RSA.E)
		if err1 != nil || err2 != nil || len(e) == 0 || len(e) > 4 {
			return ErrSignature
		}
		pub := &rsa.PublicKey{N: new(big.Int).SetBytes(n), E: int(new(big.Int).SetBytes(e).Int64())}
		h := sha256.Sum256(data)
		if rsa.VerifyPKCS1v15(pub, crypto.SHA256, h[:], sig) != nil {
			return ErrSignature
		}
		return nil
	}
	return ErrSignature
}
