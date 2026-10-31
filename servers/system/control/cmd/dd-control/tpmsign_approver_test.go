package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/hex"
	"encoding/pem"
	"errors"
	"io/fs"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"

	"commonty/control/state/attest"

	"github.com/google/go-tpm/tpm2"
	"github.com/google/go-tpm/tpm2/transport"
	"github.com/google/go-tpm/tpm2/transport/tcp"
)

// A key under the box's approver, made as dd-boot-key enrol makes it, against
// a software TPM (DD_TEST_SWTPM as TestSignWithSoftwareTPM): the signer opens
// it only under an approval of these PCRs at this epoch, a bad approval is an
// error and no panic, and a key with no approver stays PCR 7's (that test)
func TestSignUnderTheApprover(t *testing.T) {
	addrs := os.Getenv("DD_TEST_SWTPM")
	if addrs == "" {
		t.Skip("DD_TEST_SWTPM not set")
	}
	cmd, plat, _ := strings.Cut(addrs, ",")
	host, port, _ := strings.Cut(cmd, ":")
	keys := t.TempDir()
	w := t.TempDir()
	p := func(n string) string { return filepath.Join(keys, n) }
	tool := func(args ...string) []byte {
		t.Helper()
		c := exec.Command(args[0], args[1:]...)
		c.Env = append(os.Environ(), "TPM2TOOLS_TCTI=swtpm:host="+host+",port="+port)
		out, err := c.Output()
		if err != nil {
			var ee *exec.ExitError
			errors.As(err, &ee)
			t.Fatalf("%v: %v: %s", args, err, ee.Stderr)
		}
		f := exec.Command("tpm2_flushcontext", "-t")
		f.Env = c.Env
		_ = f.Run()
		return out
	}
	const idx = "0x01500DD0"
	tool("tpm2_nvdefine", "-Q", idx, "-C", "o", "-s", "8", "-a", "nt=counter|ownerwrite|ownerread|no_da")
	tool("tpm2_nvincrement", "-Q", idx, "-C", "o")
	approver, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	der, _ := x509.MarshalPKIXPublicKey(&approver.PublicKey)
	_ = os.WriteFile(filepath.Join(w, "a.pem"), pem.EncodeToMemory(&pem.Block{Type: "PUBLIC KEY", Bytes: der}), 0o600)
	tool("tpm2_loadexternal", "-Q", "-C", "o", "-G", "ecc", "-u", filepath.Join(w, "a.pem"), "-c", filepath.Join(w, "a.ctx"), "-n", p("approver.name"))
	tool("tpm2_loadexternal", "-Q", "-C", "o", "-G", "ecc", "-u", filepath.Join(w, "a.pem"), "-c", filepath.Join(w, "a.ctx"))
	c := exec.Command("sh", "-c", "tpm2_loadexternal -Q -C o -G ecc -u a.pem -c a.ctx && tpm2_readpublic -Q -c a.ctx -o "+p("approver.tpm"))
	c.Dir, c.Env = w, append(os.Environ(), "TPM2TOOLS_TCTI=swtpm:host="+host+",port="+port)
	if out, err := c.CombinedOutput(); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	tool("tpm2_flushcontext", "-t")
	name, err := os.ReadFile(p("approver.name"))
	if err != nil {
		t.Fatal(err)
	}
	pol := filepath.Join(w, "join.policy")
	_ = os.WriteFile(pol, attest.PolicyAuthorize(name), 0o600)
	mk := exec.Command("sh", "-c", "tpm2_createprimary -Q -C o -G ecc256 -c srk.ctx && tpm2_create -Q -C srk.ctx -G ecc256:ecdsa-sha256 -L join.policy"+
		" -a 'fixedtpm|fixedparent|sensitivedataorigin|sign|noda' -u "+p("sig.pub")+" -r "+p("sig.priv"))
	mk.Dir, mk.Env = w, c.Env
	if out, err := mk.CombinedOutput(); err != nil {
		t.Fatalf("%v: %s", err, out)
	}
	tool("tpm2_flushcontext", "-t")

	epochPub := attest.EpochPublic()
	en, _ := tpm2.NVName(&epochPub)
	pcrs := func() (a, b [32]byte) {
		f := filepath.Join(w, "pcrs")
		tool("tpm2_pcrread", "-Q", "sha256:4,7", "-o", f)
		raw, err := os.ReadFile(f)
		if err != nil || len(raw) != 64 {
			t.Fatalf("pcrs: %v", err)
		}
		return [32]byte(raw[:32]), [32]byte(raw[32:])
	}
	epoch := func() uint64 {
		raw := tool("tpm2_nvread", idx, "-C", "o")
		var e uint64
		for _, x := range raw {
			e = e<<8 | uint64(x)
		}
		return e
	}
	approve := func() string {
		p4, p7 := pcrs()
		a := attest.Approved(p4, p7, en.Buffer, epoch())
		sig, err := ecdsa.SignASN1(rand.Reader, approver, attest.ApprovalDigest(a))
		if err != nil {
			t.Fatal(err)
		}
		_ = os.MkdirAll(p("approvals"), 0o700)
		f := p("approvals/" + hex.EncodeToString(a) + ".sig")
		_ = os.WriteFile(f, sig, 0o600)
		return f
	}
	b := newTPMBackend(keys)
	b.open = func() (transport.TPMCloser, error) {
		return tcp.Open(tcp.Config{CommandAddress: cmd, PlatformAddress: plat})
	}
	digest := sha256.Sum256([]byte("a heartbeat"))
	sign := func() error {
		// no resource manager here to drop what a failed signature held,
		// as the kernel's does when the signer lets its connection go
		defer func() { b.release(); _, _ = b.flushStale() }()
		sig, err := b.sign(digest[:])
		if err != nil {
			return err
		}
		pub, err := b.publicKey()
		if err != nil {
			return err
		}
		if !ecdsa.VerifyASN1(pub, digest[:], sig) {
			return errors.New("the signature does not check")
		}
		return nil
	}
	if err := sign(); !errors.Is(err, fs.ErrNotExist) || !permanent(err) {
		t.Fatalf("no approval yet: %v", err)
	}
	f := approve()
	if err := sign(); err != nil {
		t.Fatalf("under its approval: %v", err)
	}
	for _, junk := range [][]byte{[]byte("junk"), {0x30, 0x06, 0x02, 0x01, 0x00, 0x02, 0x01, 0x00}, append([]byte{0x30, 0x47, 0x02, 0x41, 0x01}, make([]byte, 66)...)} {
		_ = os.WriteFile(f, junk, 0o600)
		if err := sign(); err == nil {
			t.Fatalf("signed under an approval of %x", junk)
		}
	}
	// another approver's signature: the TPM's own check refuses it
	other, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	p4, p7 := pcrs()
	forged, _ := ecdsa.SignASN1(rand.Reader, other, attest.ApprovalDigest(attest.Approved(p4, p7, en.Buffer, epoch())))
	_ = os.WriteFile(f, forged, 0o600)
	if err := sign(); err == nil {
		t.Fatal("signed under another key's approval")
	}
	// the epoch moves on: the old approval is retired
	f = approve()
	tool("tpm2_nvincrement", "-Q", idx, "-C", "o")
	if err := sign(); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("an older epoch's approval: %v", err)
	}
	approve()
	if err := sign(); err != nil {
		t.Fatalf("at the new epoch: %v", err)
	}
	// another boot: no approval for its PCR 7, none for its PCR 4
	tool("tpm2_pcrextend", "4:sha256="+strings.Repeat("11", 32))
	if err := sign(); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("another PCR 4: %v", err)
	}
	approve()
	if err := sign(); err != nil {
		t.Fatalf("approved again: %v", err)
	}
}
