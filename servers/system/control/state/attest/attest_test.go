package attest

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/asn1"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"errors"
	"math/big"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/google/go-tpm/tpm2"
)

// swtpm-vector.json is one whole attestation by swtpm (make-vector.sh):
// its EK certified by a test CA under swtpm's vendor id, a made-up event
// log booted under Secure Boot with the box's own db certificate, and the
// PCRs extended to match.
type vector struct {
	Config     Config      `json:"config"`
	Box        string      `json:"box"`
	Key        string      `json:"key"`
	At         int64       `json:"at"`
	Voters     []string    `json:"voters"`
	Begin      Begin       `json:"begin"`
	Challenges []Challenge `json:"challenges"`
	Response   Response    `json:"response"`
}

func load(t *testing.T) vector {
	t.Helper()
	raw, err := os.ReadFile("testdata/swtpm-vector.json")
	if err != nil {
		t.Fatal(err)
	}
	var v vector
	if err := json.Unmarshal(raw, &v); err != nil {
		t.Fatal(err)
	}
	return v
}

// opened: v's box under way, every voter's challenge in
func opened(t *testing.T, v vector) *State {
	t.Helper()
	s := New()
	if _, err := s.CheckOpen(&v.Config, v.Box, v.Key, v.Begin, v.At); err != nil {
		t.Fatalf("open: %v", err)
	}
	s.Open(&v.Config, v.Box, v.Key, v.Begin, v.At)
	for _, c := range v.Challenges {
		if err := s.CheckChallenge(v.Voters, c.Validator, c, v.At); err != nil {
			t.Fatalf("challenge: %v", err)
		}
		s.ApplyChallenge(c)
	}
	return s
}

func wantErr(t *testing.T, err error, part string) {
	t.Helper()
	if err == nil || !strings.Contains(err.Error(), part) {
		t.Fatalf("want an error with %q, got %v", part, err)
	}
}

func TestASoftwareTPMAttestsEndToEnd(t *testing.T) {
	v := load(t)
	s := opened(t, v)
	rec, err := s.CheckResponse(&v.Config, v.Voters, v.Box, v.Response, v.At+1000)
	if err != nil {
		t.Fatal(err)
	}
	if !rec.SecureBoot || rec.Vendor != "00001014" || rec.Model != "swtpm" || len(rec.PCRs) != 4 {
		t.Fatalf("record: %+v", rec)
	}
	s.ApplyResponse(&v.Config, v.Voters, v.Box, v.Response, v.At+1000)
	if s.Done[v.Box] == nil || s.EKs[rec.EK] != v.Box || s.Pending[v.Box] != nil {
		t.Fatalf("not recorded: %+v", s)
	}
	// one box per TPM: the same TPM under another name is refused
	_, err = s.CheckOpen(&v.Config, "jother", v.Key, v.Begin, v.At+2000)
	wantErr(t, err, "already "+v.Box)
	// until the first box is gone
	s.Forget(v.Box)
	if _, err := s.CheckOpen(&v.Config, "jother", v.Key, v.Begin, v.At+3000); err != nil {
		t.Fatal(err)
	}
}

func TestASoftwareTPMIsRefusedWithoutTheTestSwitch(t *testing.T) {
	v := load(t)
	v.Config.Test = nil
	_, err := New().CheckOpen(&v.Config, v.Box, v.Key, v.Begin, v.At)
	if !errors.Is(err, ErrSoftware) {
		t.Fatalf("want ErrSoftware, got %v", err)
	}
}

func TestTwoBoxesCannotEnlistOneTPMAtOnce(t *testing.T) {
	v := load(t)
	s := opened(t, v)
	_, err := s.CheckOpen(&v.Config, "jother", v.Key, v.Begin, v.At)
	wantErr(t, err, "already enlisting as "+v.Box)
}

func TestTheJoinKeyMustBeTheBoxs(t *testing.T) {
	v := load(t)
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	der, _ := x509.MarshalPKIXPublicKey(&k.PublicKey)
	_, err := New().CheckOpen(&v.Config, v.Box, base64.StdEncoding.EncodeToString(der), v.Begin, v.At)
	wantErr(t, err, "not this box's key")
}

func TestChallengesAreTheVotersOwnOnceEach(t *testing.T) {
	v := load(t)
	s := New()
	s.Open(&v.Config, v.Box, v.Key, v.Begin, v.At)
	c := v.Challenges[0]
	wantErr(t, s.CheckChallenge(v.Voters, "witness", c, v.At), "not a box voter")
	wantErr(t, s.CheckChallenge(v.Voters, v.Voters[1], c, v.At), "not a box voter")
	s.ApplyChallenge(c)
	wantErr(t, s.CheckChallenge(v.Voters, c.Validator, c, v.At), "already")
	wantErr(t, s.CheckChallenge(v.Voters, c.Validator, c, v.At+TTL), "not attesting")
}

func TestAnAnswerNeedsAMajorityOfVoters(t *testing.T) {
	v := load(t)
	s := New()
	s.Open(&v.Config, v.Box, v.Key, v.Begin, v.At)
	s.ApplyChallenge(v.Challenges[0])
	_, err := s.CheckResponse(&v.Config, v.Voters, v.Box, v.Response, v.At)
	wantErr(t, err, "1 of 2 box voters")
}

func TestAWrongSecretIsRefused(t *testing.T) {
	v := load(t)
	s := opened(t, v)
	r := v.Response
	r.Secrets = map[string]string{}
	for k, x := range v.Response.Secrets {
		r.Secrets[k] = x
	}
	r.Secrets[v.Voters[0]] = base64.StdEncoding.EncodeToString(make([]byte, 32))
	_, err := s.CheckResponse(&v.Config, v.Voters, v.Box, r, v.At)
	wantErr(t, err, "not the one it committed to")
}

// a voter whose secret differs from the one the box answered: the commit
// matches the new secret, but the quote was over the old nonce
func TestAQuoteOverAnotherNonceIsRefused(t *testing.T) {
	v := load(t)
	other := make([]byte, 32)
	other[0] = 1
	h := sha256.Sum256(other)
	v.Challenges[0].Commit = hex.EncodeToString(h[:])
	s := opened(t, v)
	r := v.Response
	r.Secrets = map[string]string{}
	for k, x := range v.Response.Secrets {
		r.Secrets[k] = x
	}
	r.Secrets[v.Challenges[0].Validator] = base64.StdEncoding.EncodeToString(other)
	_, err := s.CheckResponse(&v.Config, v.Voters, v.Box, r, v.At)
	wantErr(t, err, "nonce")
}

func TestASignatureByAnotherKeyIsRefused(t *testing.T) {
	v := load(t)
	s := opened(t, v)
	r := v.Response
	r.QuoteSig = r.CertifySig
	_, err := s.CheckResponse(&v.Config, v.Voters, v.Box, r, v.At)
	wantErr(t, err, "the quote")
	r = v.Response
	r.Certify = r.Quote
	r.CertifySig = r.QuoteSig
	_, err = s.CheckResponse(&v.Config, v.Voters, v.Box, r, v.At)
	wantErr(t, err, "not the TPM's own statement of that kind")
}

func TestABootNotUnderTheBoxsOwnKeyIsRefused(t *testing.T) {
	v := load(t)
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	tmpl := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "someone else"}, NotBefore: time.Unix(0, 0), NotAfter: time.Unix(1<<40, 0)}
	der, _ := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &k.PublicKey, k)
	v.Begin.DB = base64.StdEncoding.EncodeToString(der)
	s := opened(t, v)
	_, err := s.CheckResponse(&v.Config, v.Voters, v.Box, v.Response, v.At)
	if !errors.Is(err, ErrSecureBoot) {
		t.Fatalf("want ErrSecureBoot, got %v", err)
	}
	// a test VM's firmware has no Secure Boot: its switch lets it in, and
	// the record says so
	v.Config.Test.NoSecureBoot = true
	rec, err := s.CheckResponse(&v.Config, v.Voters, v.Box, v.Response, v.At)
	if err != nil || rec.SecureBoot {
		t.Fatalf("want a record without Secure Boot, got %+v %v", rec, err)
	}
}

func TestALogThatDoesNotGiveTheQuotedPCRsIsRefused(t *testing.T) {
	v := load(t)
	raw, _ := base64.StdEncoding.DecodeString(v.Begin.Log)
	raw[len(raw)-1] ^= 1 // the last event's data, not its digest
	log, err := parseLog(raw)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := replay(log); err != nil {
		t.Fatal(err)
	}
	// its digest is unchanged, so the PCRs agree; a changed digest would not
	off := len(raw) - len("the boot loader") - 4 - 32
	raw[off] ^= 1
	v.Begin.Log = base64.StdEncoding.EncodeToString(raw)
	s := opened(t, v)
	_, err = s.CheckResponse(&v.Config, v.Voters, v.Box, v.Response, v.At)
	wantErr(t, err, "does not give the PCRs quoted")
}

func TestAPCR7EventMustBeWhatWasMeasured(t *testing.T) {
	v := load(t)
	raw, _ := base64.StdEncoding.DecodeString(v.Begin.Log)
	log, _ := parseLog(raw)
	for i, e := range log {
		if e.pcr == 7 && e.typ == evEFIVariableConfig {
			log[i].data = append([]byte(nil), e.data...)
			log[i].data[len(e.data)-1] = 1 // SecureBoot claimed on, measured as it was
			log[i].sha256 = make([]byte, 32)
		}
	}
	db, _ := base64.StdEncoding.DecodeString(v.Begin.DB)
	_, err := secureBoot(log, db)
	wantErr(t, err, "not what was measured")
}

func TestAnUnfinishedAttestationExpires(t *testing.T) {
	v := load(t)
	s := opened(t, v)
	s.Expire(v.At + TTL - 1)
	if s.Pending[v.Box] == nil {
		t.Fatal("gone early")
	}
	s.Expire(v.At + TTL)
	if s.Pending[v.Box] != nil {
		t.Fatal("still there")
	}
}

func TestCutLogsAreRefused(t *testing.T) {
	v := load(t)
	raw, _ := base64.StdEncoding.DecodeString(v.Begin.Log)
	for _, n := range []int{0, 10, 40, len(raw) - 1} {
		if _, err := parseLog(raw[:n]); err == nil {
			t.Fatalf("a log cut at %d parsed", n)
		}
	}
	bad := append([]byte(nil), raw...)
	binary.LittleEndian.PutUint32(bad[4:], 1)
	if _, err := parseLog(bad); err == nil {
		t.Fatal("a SHA-1 log parsed")
	}
}

func TestThePolicyIsWhatTPM2PolicyPCRGives(t *testing.T) {
	v := load(t)
	join, err := public(v.Begin.Join)
	if err != nil {
		t.Fatal(err)
	}
	raw, _ := base64.StdEncoding.DecodeString(v.Begin.Log)
	log, _ := parseLog(raw)
	pcrs, _ := replay(log)
	if hex.EncodeToString(policyPCR7(pcrs[7])) != hex.EncodeToString(join.AuthPolicy.Buffer) {
		t.Fatal("policy differs from tpm2_policypcr's")
	}
}

// node1's TPM (Intel PTT, Tiger Lake), read from its NV: the RSA and ECC
// EK certificates and the chain index 0x01C00100 (three certificates),
// with the two above it from Intel's AIA links (testdata/README.md)
func intelChain(t *testing.T, leaf string) []*x509.Certificate {
	t.Helper()
	read := func(f string) []byte {
		b, err := os.ReadFile("testdata/" + f)
		if err != nil {
			t.Fatal(err)
		}
		return b
	}
	var ders [][]byte
	ders = append(ders, read(leaf))
	nv := read("node1-ek-chain.bin")
	for len(nv) > 4 {
		n := int(binary.BigEndian.Uint16(nv[2:4])) + 4
		ders = append(ders, nv[:n])
		nv = nv[n:]
	}
	ders = append(ders, read("intel-tgl-00002004-ca.der"), read("intel-csme-intermediate.der"))
	var certs []*x509.Certificate
	for _, d := range ders {
		c, err := x509.ParseCertificate(d)
		if err != nil {
			t.Fatal(err)
		}
		certs = append(certs, c)
	}
	return certs
}

func intelPolicy(t *testing.T) *policy {
	t.Helper()
	root, err := os.ReadFile("testdata/intel-ondie-root.der")
	if err != nil {
		t.Fatal(err)
	}
	p, err := (&Config{Roots: map[string][]string{
		"494E5443": {string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: root}))},
	}}).policy()
	if err != nil {
		t.Fatal(err)
	}
	return p
}

var oct2026 = time.Date(2026, 10, 17, 0, 0, 0, 0, time.UTC)

func TestARealIntelEKChainsToIntelsRoot(t *testing.T) {
	pol := intelPolicy(t)
	for _, leaf := range []string{"node1-ek-rsa.der", "node1-ek-ecc.der"} {
		vendor, model, err := verifyChain(pol, intelChain(t, leaf), oct2026)
		if err != nil {
			t.Fatalf("%s: %v", leaf, err)
		}
		if vendor != "494E5443" || model != "TGL" {
			t.Fatalf("%s: %s %s", leaf, vendor, model)
		}
	}
}

func TestARealIntelEKWithoutItsChainIsRefused(t *testing.T) {
	pol := intelPolicy(t)
	certs := intelChain(t, "node1-ek-ecc.der")
	// the NV chain alone: Intel's two AIA certificates missing
	_, _, err := verifyChain(pol, certs[:4], oct2026)
	wantErr(t, err, "does not chain")
	// before it was made
	_, _, err = verifyChain(pol, certs, time.Date(2020, 1, 1, 0, 0, 0, 0, time.UTC))
	wantErr(t, err, "does not chain")
	// only another maker's roots
	other := &policy{roots: map[string]*x509.CertPool{"49465800": pol.roots["494E5443"]}}
	_, _, err = verifyChain(other, certs, oct2026)
	if !errors.Is(err, ErrNoRoot) {
		t.Fatalf("want ErrNoRoot, got %v", err)
	}
}

// a TPM's ECC EK would make it a second box beside its RSA one
func TestOnlyTheRSAEKCounts(t *testing.T) {
	ecc := tpm2.TPMTPublic{
		Type:    tpm2.TPMAlgECC,
		NameAlg: tpm2.TPMAlgSHA256,
		ObjectAttributes: tpm2.TPMAObject{
			FixedTPM: true, FixedParent: true, SensitiveDataOrigin: true, AdminWithPolicy: true, Restricted: true, Decrypt: true,
		},
		Parameters: tpm2.NewTPMUPublicParms(tpm2.TPMAlgECC, &tpm2.TPMSECCParms{
			Symmetric: tpm2.TPMTSymDefObject{
				Algorithm: tpm2.TPMAlgAES,
				KeyBits:   tpm2.NewTPMUSymKeyBits(tpm2.TPMAlgAES, tpm2.TPMKeyBits(128)),
				Mode:      tpm2.NewTPMUSymMode(tpm2.TPMAlgAES, tpm2.TPMAlgCFB),
			},
			CurveID: tpm2.TPMECCNistP256,
		}),
	}
	wantErr(t, ekShape(&ecc), "not RSA 2048")
	v := load(t)
	rsa, err := public(v.Begin.EK)
	if err != nil {
		t.Fatal(err)
	}
	if err := ekShape(rsa); err != nil {
		t.Fatal(err)
	}
}

// an EK certificate's key named RSAES-OAEP (AMD's do) is the same RSA key
func TestAnOAEPNamedEKKeyParses(t *testing.T) {
	key, err := rsa.GenerateKey(rand.Reader, 2048)
	if err != nil {
		t.Fatal(err)
	}
	spki, err := asn1.Marshal(struct {
		Algorithm pkix.AlgorithmIdentifier
		Key       asn1.BitString
	}{
		pkix.AlgorithmIdentifier{Algorithm: oidRSAESOAEP, Parameters: asn1.NullRawValue},
		asn1.BitString{Bytes: x509.MarshalPKCS1PublicKey(&key.PublicKey), BitLength: 8 * len(x509.MarshalPKCS1PublicKey(&key.PublicKey))},
	})
	if err != nil {
		t.Fatal(err)
	}
	got, err := ekCertKey(&x509.Certificate{RawSubjectPublicKeyInfo: spki})
	if err != nil || !samePub(got, &key.PublicKey) {
		t.Fatalf("%v", err)
	}
	spki[8] ^= 1 // another algorithm
	if _, err := ekCertKey(&x509.Certificate{RawSubjectPublicKeyInfo: spki}); err == nil {
		t.Fatal("a key of another kind taken")
	}
}

// a certificate naming no maker is taken only as AMD's (whose fTPM
// certificates name none), issued under AMD's name and chained to AMD's roots
func TestAnEKCertificateNamingNoMakerIsAMDsOrNone(t *testing.T) {
	pool, leaf := testEK(t, "Advanced Micro Devices", true)
	intel := intelPolicy(t).roots["494E5443"]
	pol := &policy{roots: map[string]*x509.CertPool{"414D4400": pool, "494E5443": intel}}
	vendor, _, err := verifyChain(pol, []*x509.Certificate{leaf}, oct2026)
	if err != nil || vendor != "414D4400" {
		t.Fatalf("%s %v", vendor, err)
	}
	// chained to another maker's root: refused, never tried across makers
	for _, id := range []string{"494E5443", "00001014"} {
		pol = &policy{roots: map[string]*x509.CertPool{id: pool, "414D4400": intel}, test: Test{SoftwareTPM: true}}
		if _, _, err := verifyChain(pol, []*x509.Certificate{leaf}, oct2026); err == nil {
			t.Fatalf("a nameless certificate taken by %s's root", id)
		}
	}
	// issued by someone not AMD, though under a root filed as AMD's
	pool, other := testEK(t, "Someone Else", true)
	pol = &policy{roots: map[string]*x509.CertPool{"414D4400": pool}}
	if _, _, err := verifyChain(pol, []*x509.Certificate{other}, oct2026); !errors.Is(err, ErrNoRoot) {
		t.Fatalf("want ErrNoRoot, got %v", err)
	}
}

func TestALeafWithoutEKMarkersIsRefused(t *testing.T) {
	pool, leaf := testEK(t, "Advanced Micro Devices", false)
	pol := &policy{roots: map[string]*x509.CertPool{"414D4400": pool}}
	_, _, err := verifyChain(pol, []*x509.Certificate{leaf}, oct2026)
	wantErr(t, err, "not an EK certificate")
}

// testEK: a CA under org's name and an EK certificate it issued naming no
// maker, with the EK certificate purpose or without
func testEK(t *testing.T, org string, marked bool) (*x509.CertPool, *x509.Certificate) {
	t.Helper()
	caKey, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	tmpl := &x509.Certificate{
		SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: "maker root", Organization: []string{org}},
		NotBefore: oct2026.AddDate(-1, 0, 0), NotAfter: oct2026.AddDate(10, 0, 0),
		IsCA: true, BasicConstraintsValid: true, KeyUsage: x509.KeyUsageCertSign,
	}
	caDER, _ := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &caKey.PublicKey, caKey)
	ca, _ := x509.ParseCertificate(caDER)
	ekKey, _ := rsa.GenerateKey(rand.Reader, 2048)
	lt := &x509.Certificate{
		SerialNumber: big.NewInt(2), NotBefore: tmpl.NotBefore, NotAfter: tmpl.NotAfter,
		KeyUsage: x509.KeyUsageKeyEncipherment,
	}
	if marked {
		lt.UnknownExtKeyUsage = []asn1.ObjectIdentifier{oidEKCertificate}
	}
	leafDER, err := x509.CreateCertificate(rand.Reader, lt, ca, &ekKey.PublicKey, caKey)
	if err != nil {
		t.Fatal(err)
	}
	leaf, _ := x509.ParseCertificate(leafDER)
	pool := x509.NewCertPool()
	pool.AddCert(ca)
	return pool, leaf
}

// David's laptop (Lenovo Legion Pro 5 16ADR10, AMD fTPM): its RSA EK's
// certificate from AMD's service, and the CA above it by its AIA link
// (testdata/README.md)
func amdChain(t *testing.T) ([]*x509.Certificate, *policy) {
	t.Helper()
	var certs []*x509.Certificate
	for _, f := range []string{"laptop-ek-rsa.der", "amd-prg-rpl-ica.der"} {
		der, err := os.ReadFile("testdata/" + f)
		if err != nil {
			t.Fatal(err)
		}
		c, err := x509.ParseCertificate(der)
		if err != nil {
			t.Fatalf("%s: %v", f, err)
		}
		certs = append(certs, c)
	}
	root, err := os.ReadFile("testdata/amd-root.der")
	if err != nil {
		t.Fatal(err)
	}
	pol, err := (&Config{Roots: map[string][]string{
		"414D4400": {string(pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: root}))},
	}}).policy()
	if err != nil {
		t.Fatal(err)
	}
	return certs, pol
}

func TestARealAMDEKChainsToAMDsRoot(t *testing.T) {
	certs, pol := amdChain(t)
	vendor, model, err := verifyChain(pol, certs, oct2026)
	if err != nil || vendor != "414D4400" || model != "AMD" {
		t.Fatalf("%s %s %v", vendor, model, err)
	}
	raw, err := os.ReadFile("testdata/laptop-ek.pub")
	if err != nil {
		t.Fatal(err)
	}
	p2b, err := tpm2.Unmarshal[tpm2.TPM2BPublic](raw)
	if err != nil {
		t.Fatal(err)
	}
	ek, err := p2b.Contents()
	if err != nil {
		t.Fatal(err)
	}
	if err := ekShape(ek); err != nil {
		t.Fatal(err)
	}
	pub, _ := tpm2.Pub(*ek)
	key, err := ekCertKey(certs[0])
	if err != nil || !samePub(key, pub) {
		t.Fatalf("the certificate is not for the laptop's EK: %v", err)
	}
	// without AMD's CA between them
	_, _, err = verifyChain(pol, certs[:1], oct2026)
	wantErr(t, err, "does not chain")
}

func dbHash(t *testing.T, b64der string) string {
	t.Helper()
	der, err := base64.StdEncoding.DecodeString(b64der)
	if err != nil {
		t.Fatal(err)
	}
	h := sha256.Sum256(der)
	return hex.EncodeToString(h[:])
}

// a box's db certificate is its own: Microsoft's and a distro's shim keys
// are everyone's, and no two boxes claim one
func TestASharedSecureBootCertificateIsRefused(t *testing.T) {
	v := load(t)
	for _, f := range []string{"ms-uefi-ca-2011.der", "canonical-master-ca.der"} {
		der, err := os.ReadFile("testdata/" + f)
		if err != nil {
			t.Fatal(err)
		}
		b := v.Begin
		b.DB = base64.StdEncoding.EncodeToString(der)
		_, err = New().CheckOpen(&v.Config, v.Box, v.Key, b, v.At)
		wantErr(t, err, "shared")
	}
	h := dbHash(t, v.Begin.DB)
	s := New()
	s.DBs[h] = "jother"
	_, err := s.CheckOpen(&v.Config, v.Box, v.Key, v.Begin, v.At)
	wantErr(t, err, "already jother's")
	s = New()
	s.Pending["jother"] = &Pending{DB: h, Opened: v.At}
	_, err = s.CheckOpen(&v.Config, v.Box, v.Key, v.Begin, v.At)
	wantErr(t, err, "jother")
}

func TestTheDBCertificateIsRecordedAndForgotten(t *testing.T) {
	v := load(t)
	s := opened(t, v)
	s.ApplyResponse(&v.Config, v.Voters, v.Box, v.Response, v.At+1000)
	h := dbHash(t, v.Begin.DB)
	if s.DBs[h] != v.Box || s.Done[v.Box].DB != h {
		t.Fatalf("not recorded: %+v", s.DBs)
	}
	s.Forget(v.Box)
	if len(s.DBs) != 0 {
		t.Fatal("kept after the box left")
	}
}
