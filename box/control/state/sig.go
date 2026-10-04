package state

import (
	"crypto/ecdsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"math/big"
	"strings"
)

// P256 checks a box's signature: its TPM-held key as SPKI, base64 (the
// unlockKey in fleet/boxes.json); the signature base64url, as the TPM gives
// it - r||s (64 bytes) or DER.
func P256(spki string, msg []byte, sig string) bool {
	der, err := base64.StdEncoding.DecodeString(spki)
	if err != nil {
		return false
	}
	k, err := x509.ParsePKIXPublicKey(der)
	if err != nil {
		return false
	}
	pub, ok := k.(*ecdsa.PublicKey)
	if !ok {
		return false
	}
	raw, err := base64.RawURLEncoding.DecodeString(strings.TrimRight(sig, "="))
	if err != nil {
		return false
	}
	h := sha256.Sum256(msg)
	if len(raw) == 64 {
		return ecdsa.Verify(pub, h[:], new(big.Int).SetBytes(raw[:32]), new(big.Int).SetBytes(raw[32:]))
	}
	return ecdsa.VerifyASN1(pub, h[:], raw)
}
