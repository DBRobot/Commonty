package state

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"testing"
)

func TestP256TakesBothSignatureShapesAndNothingElse(t *testing.T) {
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	der, _ := x509.MarshalPKIXPublicKey(&k.PublicKey)
	spki := base64.StdEncoding.EncodeToString(der)
	msg := []byte("commonty control v1\x00{}")
	h := sha256.Sum256(msg)
	asn1, _ := ecdsa.SignASN1(rand.Reader, k, h[:])
	r, s, _ := ecdsa.Sign(rand.Reader, k, h[:])
	plain := make([]byte, 64)
	r.FillBytes(plain[:32])
	s.FillBytes(plain[32:])
	enc := base64.RawURLEncoding.EncodeToString
	if !P256(spki, msg, enc(asn1)) || !P256(spki, msg, enc(plain)) {
		t.Fatal("a good signature was refused")
	}
	if P256(spki, []byte("something else"), enc(plain)) {
		t.Fatal("a signature over another message passed")
	}
	other, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	od, _ := x509.MarshalPKIXPublicKey(&other.PublicKey)
	if P256(base64.StdEncoding.EncodeToString(od), msg, enc(plain)) {
		t.Fatal("another key's check passed")
	}
}
