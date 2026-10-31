//go:build testswitches

package attest

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"os"
	"testing"

	"github.com/google/go-tpm/tpm2"
)

// policy-vector.json: the pieces as swtpm computed them
// (make-policy-vector.sh)
type policyVector struct {
	Approver, ApproverName, Authorize, EpochName, Epoch string
	PCR4, PCR7, PCRPolicy, Approved                     string
}

func loadPolicy(t *testing.T) policyVector {
	t.Helper()
	raw, err := os.ReadFile("testdata/policy-vector.json")
	if err != nil {
		t.Fatal(err)
	}
	var v policyVector
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	return v
}

func h32(t *testing.T, s string) [32]byte {
	t.Helper()
	b, err := hex.DecodeString(s)
	if err != nil || len(b) != 32 {
		t.Fatalf("%q", s)
	}
	return [32]byte(b)
}

func TestThePoliciesAreTheTPMs(t *testing.T) {
	v := loadPolicy(t)
	raw, _ := base64.StdEncoding.DecodeString(v.Approver)
	_, name, err := approverKey(raw)
	if err != nil || hex.EncodeToString(name) != v.ApproverName {
		t.Fatalf("approver name %x %v", name, err)
	}
	if got := hex.EncodeToString(PolicyAuthorize(name)); got != v.Authorize {
		t.Fatalf("PolicyAuthorize %s", got)
	}
	pub := EpochPublic()
	en, err := tpm2.NVName(&pub)
	if err != nil || hex.EncodeToString(en.Buffer) != v.EpochName {
		t.Fatalf("epoch name %x %v", en.Buffer, err)
	}
	p4, p7 := h32(t, v.PCR4), h32(t, v.PCR7)
	if got := hex.EncodeToString(PolicyPCR47(p4, p7)); got != v.PCRPolicy {
		t.Fatalf("PolicyPCR %s", got)
	}
	if got := hex.EncodeToString(Approved(p4, p7, en.Buffer, 2)); got != v.Approved {
		t.Fatalf("approved %s", got)
	}
	// the epoch counter, as a Begin carries it
	b := tpm2.Marshal(tpm2.New2B(pub))
	if n, err := epochName(b); err != nil || hex.EncodeToString(n) != v.EpochName {
		t.Fatalf("%x %v", n, err)
	}
}

// an approval is for one boot: another PCR 4 - a Microsoft-signed loader
// started in place of ours - or another epoch is another policy
func TestAnApprovalIsForOneBootAndEpoch(t *testing.T) {
	v := loadPolicy(t)
	p4, p7 := h32(t, v.PCR4), h32(t, v.PCR7)
	en, _ := hex.DecodeString(v.EpochName)
	ours := Approved(p4, p7, en, 2)
	other := p4
	other[0] ^= 1
	if hex.EncodeToString(Approved(other, p7, en, 2)) == hex.EncodeToString(ours) ||
		hex.EncodeToString(Approved(p4, p7, en, 3)) == hex.EncodeToString(ours) {
		t.Fatal("an approval covers more than its boot and epoch")
	}
	// an index that is not a counter (one whose value could be set back)
	pub := EpochPublic()
	pub.Attributes.NT = tpm2.TPMNTOrdinary
	if _, err := epochName(tpm2.Marshal(tpm2.New2B(pub))); err == nil {
		t.Fatal("an ordinary index taken for the epoch")
	}
}

// the judge: the join key opens only under the approver's approvals, and
// this boot's is one, at the epoch the TPM certifies
func TestTheJoinKeyMustBeUnderAnApprovedBoot(t *testing.T) {
	v := load(t)
	try := func(v vector, part string) {
		t.Helper()
		s := New()
		if _, err := s.CheckOpen(&v.Config, v.Box, v.Key, v.Begin, v.At); err != nil {
			wantErr(t, err, part)
			return
		}
		s.Open(&v.Config, v.Box, v.Key, v.Begin, v.At)
		for _, c := range v.Challenges {
			s.ApplyChallenge(c)
		}
		_, err := s.CheckResponse(&v.Config, v.Voters, v.Box, v.Response, v.At)
		wantErr(t, err, part)
	}
	// an approval for another boot (a Microsoft-signed loader in PCR 4):
	// another key's signature stands in, as any approval but this boot's
	// fails the same check
	w := v
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	sig, _ := ecdsa.SignASN1(rand.Reader, k, ApprovalDigest(make([]byte, 32)))
	w.Response.Approval = base64.StdEncoding.EncodeToString(sig)
	try(w, "not approved")
	// the approval at another epoch than the certified one
	w = v
	w.Response.Epoch++
	try(w, "certified epoch")
	// the epoch's certification over another nonce
	w = v
	w.Response.EpochCert, w.Response.EpochCertSig = v.Response.Certify, v.Response.CertifySig
	try(w, "the epoch")
	// no approver: the join key's policy then names nothing it checks
	w = v
	w.Begin.Approver = ""
	try(w, "no approver")
	// another approver than the join key's policy names
	w = v
	other, _ := base64.StdEncoding.DecodeString(loadPolicy(t).Approver)
	w.Begin.Approver = base64.StdEncoding.EncodeToString(other)
	try(w, "not its approver's")
}
