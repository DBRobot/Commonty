package tpm

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"os"
	"testing"

	"commonty/control/state/attest"
)

func TestTheChainIndexSplitsIntoItsCertificates(t *testing.T) {
	nv, err := os.ReadFile("../state/attest/testdata/node1-ek-chain.bin")
	if err != nil {
		t.Fatal(err)
	}
	if n := len(splitDER(nv)); n != 3 {
		t.Fatalf("%d certificates", n)
	}
	ek, _ := os.ReadFile("../state/attest/testdata/node1-ek-ecc.der")
	padded := append(append([]byte(nil), ek...), make([]byte, 300)...)
	if !bytes.Equal(oneDER(padded), ek) {
		t.Fatal("the NV index's padding kept")
	}
}

func TestAChallengeIsSealedForTheAKAndCommitted(t *testing.T) {
	raw, err := os.ReadFile("../state/attest/testdata/swtpm-vector.json")
	if err != nil {
		t.Fatal(err)
	}
	var v struct {
		Begin attest.Begin `json:"begin"`
	}
	_ = json.Unmarshal(raw, &v)
	a, err := MakeChallenge(v.Begin, "jbox", "node1")
	if err != nil {
		t.Fatal(err)
	}
	b, _ := MakeChallenge(v.Begin, "jbox", "node1")
	if a.Commit == b.Commit || a.Secret == b.Secret {
		t.Fatal("two challenges alike: the secret is not fresh")
	}
	if h, _ := hex.DecodeString(a.Commit); len(h) != 32 || a.Box != "jbox" || a.Validator != "node1" {
		t.Fatalf("%+v", a)
	}
	f, err := credFile(a)
	if err != nil || !bytes.HasPrefix(f, []byte{0xBA, 0xDC, 0xC0, 0xDE, 0, 0, 0, 1}) {
		t.Fatal("not tpm2-tools' credential file")
	}
}
