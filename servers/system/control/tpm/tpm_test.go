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

func TestTheMakerIsReadFromGetcap(t *testing.T) {
	props := []byte("TPM2_PT_FAMILY_INDICATOR:\n  raw: 0x322E3000\n  value: \"2.0\"\nTPM2_PT_MANUFACTURER:\n  raw: 0x414D4400\n  value: \"AMD\"\n")
	if m := manufacturer(props); m != amdVendor {
		t.Fatalf("%q", m)
	}
	if m := manufacturer([]byte("TPM2_PT_MANUFACTURER:\n  raw: 0x1014\n")); m != "00001014" {
		t.Fatalf("%q", m)
	}
	if manufacturer([]byte("nothing")) != "" {
		t.Fatal("a maker from nothing")
	}
}

// David's laptop's EK, and where AMD served its certificate on 2026-10-19
func TestTheAMDLinkIsMadeFromTheEK(t *testing.T) {
	raw, err := os.ReadFile("../state/attest/testdata/laptop-ek.pub")
	if err != nil {
		t.Fatal(err)
	}
	url, err := AMDEKCertURL(raw)
	if err != nil || url != "https://ftpm.amd.com/pki/aia/63b28c74805e8a4ed6d8cea39729f030" {
		t.Fatalf("%s %v", url, err)
	}
}

func TestLoaderEntrySelectedReads(t *testing.T) {
	raw := []byte{6, 0, 0, 0}
	for _, c := range "nixos-generation-3.efi" {
		raw = append(raw, byte(c), 0)
	}
	raw = append(raw, 0, 0)
	if s := utf16le(raw[4:]); s != "nixos-generation-3.efi" {
		t.Fatalf("%q", s)
	}
}

func TestTheESPsNewestGenerationsAreItsBoots(t *testing.T) {
	esp := t.TempDir()
	if err := os.MkdirAll(esp+"/EFI/Linux", 0o755); err != nil {
		t.Fatal(err)
	}
	for _, g := range []string{"2-a", "10-b", "9-c", "1-d"} {
		_ = os.WriteFile(esp+"/EFI/Linux/nixos-generation-"+g+".efi", nil, 0o644)
	}
	boots, err := ESPBoots(esp)
	if err != nil || len(boots) != KeepGood {
		t.Fatalf("%v %v", boots, err)
	}
	for i, want := range []string{"10-b", "9-c", "2-a"} {
		if boots[i][0] != esp+"/EFI/systemd/systemd-bootx64.efi" || boots[i][1] != esp+"/EFI/Linux/nixos-generation-"+want+".efi" {
			t.Fatalf("%d: %v", i, boots[i])
		}
	}
}

func TestPCRReadOutputIsReadByIndex(t *testing.T) {
	out := []byte("  sha256:\n    0 : 0xAB01\n    2 : 0xCD02\n    4 : 0x0000\n    7 : 0xEF03\n")
	if !pcrIn(out, "0", "ab01") || !pcrIn(out, "7", "EF03") || pcrIn(out, "2", "cd03") || pcrIn(out, "1", "ab01") {
		t.Fatal("pcrread's lines misread")
	}
}
