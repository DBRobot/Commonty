// Package attest judges a box's proof that it is a real machine with a real
// TPM: the TPM's endorsement key (EK) certificate chains to a TPM maker's
// root the release ships, a key the box made in that TPM (the AK) is shown
// to sit beside the EK by credential activation, the AK certifies the box's
// join key, and the AK quotes PCRs 0, 2, 4 and 7 over a nonce only the
// validators' challenges make. Like the rest of state it is deterministic:
// the agreed time comes in, no clock, no network, no randomness.
// docs/dev/attestation.md
package attest

import (
	"bytes"
	"crypto"
	"crypto/ecdsa"
	"crypto/rsa"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/pem"
	"errors"
	"fmt"
	"math/big"
	"slices"
	"sort"
	"time"

	"github.com/google/go-tpm/tpm2"
)

// Begin is what a box sends to start: everything about its TPM that a
// challenge is made from. It rides in the box's request to enlist.
type Begin struct {
	EK string `json:"ek"` // TPM2B_PUBLIC, base64, as tpm2_createek writes it
	// DER certificates, base64: the EK's first, then the TPM's own chain
	// (NV 0x01C00100) and what their AIA links gave; never the root
	Chain []string `json:"chain"`
	AK    string   `json:"ak"`   // TPM2B_PUBLIC of the key made to attest
	Join  string   `json:"join"` // TPM2B_PUBLIC of the box's join key (sig.pub)
	Log   string   `json:"log"`  // the firmware's event log, base64
	// the box's own Secure Boot certificate, as its installer made and
	// enrolled it in db, DER base64; empty where Secure Boot is not on,
	// refused outside tests
	DB string `json:"db,omitempty"`
	// the join key's approver (TPM2B_PUBLIC as the TPM loads it) and the
	// epoch counter (TPM2B_NV_PUBLIC), base64: the join key's policy is
	// PolicyAuthorize by the approver (policy.go)
	Approver string `json:"approver,omitempty"`
	Epoch    string `json:"epoch,omitempty"`
	// the lanzaboote image this boot ran (the ESP's EFI/Linux entry
	// systemd-boot chose), base64: its Authenticode digest is in PCR 4
	Stub string `json:"stub,omitempty"`
}

// Challenge is one validator's: a secret only the EK's TPM can open, and
// only for the AK named in Begin, with a hash of it everyone can check.
type Challenge struct {
	Box       string `json:"box"`
	Validator string `json:"validator"`
	Cred      string `json:"cred"`   // TPM2B_ID_OBJECT, base64
	Secret    string `json:"secret"` // TPM2B_ENCRYPTED_SECRET, base64
	Commit    string `json:"commit"` // sha256 of the secret, hex
}

// Response is the box's answer: every secret it opened, and a quote and a
// certification over a nonce made of them.
type Response struct {
	Box        string            `json:"box"`
	Secrets    map[string]string `json:"secrets"` // validator -> its secret, base64
	Quote      string            `json:"quote"`   // TPMS_ATTEST, base64
	QuoteSig   string            `json:"quoteSig"`
	Certify    string            `json:"certify"` // TPMS_ATTEST of the join key
	CertifySig string            `json:"certifySig"`
	// the epoch now, certified by the AK over the nonce, and the
	// approver's approval of this boot at it (ASN.1 ECDSA)
	Epoch        uint64 `json:"epoch,omitempty"`
	EpochCert    string `json:"epochCert,omitempty"` // TPMS_ATTEST of the counter
	EpochCertSig string `json:"epochCertSig,omitempty"`
	Approval     string `json:"approval,omitempty"`
}

// Config is the signed part, from the release: each TPM maker's roots by
// its TCG vendor id in hex ("494E5443" is Intel), and test switches a
// release never sets.
type Config struct {
	Roots map[string][]string `json:"roots"` // vendor id -> PEM certificates
	Boot  *Boot               `json:"boot,omitempty"`
	Test  *Test               `json:"test,omitempty"`
}

type Test struct {
	// a software TPM (swtpm) and its own roots, given under its vendor id
	SoftwareTPM bool `json:"softwareTPM,omitempty"`
	// a VM's firmware boots without Secure Boot
	NoSecureBoot bool `json:"noSecureBoot,omitempty"`
	// a box joins on its word alone, no proof of its TPM (state/join.go):
	// for tests of what comes after the join
	UnattestedJoin bool `json:"unattestedJoin,omitempty"`
}

// PCRs is what a quote covers: firmware (0), option ROMs (2), what was
// booted (4) and Secure Boot's state and keys (7).
var PCRs = []int{0, 2, 4, 7}

var (
	ErrSoftware   = errors.New("a software TPM: refused outside tests")
	ErrNoRoot     = errors.New("no root for this TPM's maker")
	ErrSecureBoot = errors.New("not booted under its own Secure Boot keys")
	// PCR 4: not the release's systemd-boot and lanzaboote image
	ErrBoot = errors.New("not the release's boot")
	// the EK certificate does not chain to its maker's roots
	ErrEK = errors.New("not a TPM its maker certified")
)

// software TPMs by vendor id: swtpm's own default, and IBM's simulator
var software = map[string]bool{"00001014": true, "49424D00": true}

type policy struct {
	roots map[string]*x509.CertPool
	boot  *Boot
	test  Test
}

func (c *Config) policy() (*policy, error) {
	p := &policy{roots: map[string]*x509.CertPool{}}
	if c == nil {
		return p, nil
	}
	if c.Test != nil {
		p.test = *c.Test
	}
	p.boot = c.Boot
	for vendor, pems := range c.Roots {
		pool := x509.NewCertPool()
		for _, s := range pems {
			rest := []byte(s)
			for {
				var blk *pem.Block
				blk, rest = pem.Decode(rest)
				if blk == nil {
					break
				}
				cert, err := x509.ParseCertificate(blk.Bytes)
				if err != nil {
					return nil, fmt.Errorf("a root for %s: %v", vendor, err)
				}
				pool.AddCert(cert)
			}
		}
		p.roots[vendor] = pool
	}
	return p, nil
}

// Checked is what VerifyBegin found: enough to make a challenge and to
// judge the answer.
type Checked struct {
	EKHash    string // sha256 of the EK's public key (SPKI DER), hex: one box per TPM
	Vendor    string // TCG vendor id, hex
	Model     string
	EKPub     crypto.PublicKey
	AKName    []byte // nameAlg || H(TPMT_PUBLIC)
	DBHash    string // sha256 of the box's db certificate, hex: one box per key
	approver  *ecdsa.PublicKey
	epochName []byte
	ak        *tpm2.TPMTPublic
	join      *tpm2.TPMTPublic
	log       []event
	db        []byte
	stub      []byte
}

func b64(s string) ([]byte, error) { return base64.StdEncoding.DecodeString(s) }

func public(s string) (*tpm2.TPMTPublic, error) {
	raw, err := b64(s)
	if err != nil {
		return nil, err
	}
	p2b, err := tpm2.Unmarshal[tpm2.TPM2BPublic](raw)
	if err != nil {
		return nil, err
	}
	return p2b.Contents()
}

// VerifyBegin checks Begin at the agreed time at: the EK's certificate and
// chain, the AK and join key's attributes, the join key being the box's
// key (joinSPKI, base64 SPKI as in Config.Boxes) and the event log's form.
func VerifyBegin(cfg *Config, b Begin, joinSPKI string, at time.Time) (*Checked, error) {
	pol, err := cfg.policy()
	if err != nil {
		return nil, err
	}
	return verifyBegin(pol, b, joinSPKI, at)
}

func verifyBegin(pol *policy, b Begin, joinSPKI string, at time.Time) (*Checked, error) {
	certs, err := parseChain(b.Chain)
	if err != nil {
		return nil, err
	}
	ek, err := public(b.EK)
	if err != nil {
		return nil, fmt.Errorf("the EK: %v", err)
	}
	ekPub, err := tpm2.Pub(*ek)
	if err != nil {
		return nil, fmt.Errorf("the EK: %v", err)
	}
	certKey, err := ekCertKey(certs[0])
	if err != nil {
		return nil, err
	}
	if !samePub(certKey, ekPub) {
		return nil, errors.New("the EK certificate is for another key")
	}
	if err := ekShape(ek); err != nil {
		return nil, err
	}
	vendor, model, err := verifyChain(pol, certs, at)
	if err != nil {
		return nil, err
	}
	spki, err := x509.MarshalPKIXPublicKey(ekPub)
	if err != nil {
		return nil, err
	}
	h := sha256.Sum256(spki)

	ak, err := public(b.AK)
	if err != nil {
		return nil, fmt.Errorf("the AK: %v", err)
	}
	if err := akShape(ak); err != nil {
		return nil, err
	}
	akName, err := tpm2.ObjectName(ak)
	if err != nil {
		return nil, err
	}
	join, err := public(b.Join)
	if err != nil {
		return nil, fmt.Errorf("the join key: %v", err)
	}
	if err := joinShape(join, joinSPKI); err != nil {
		return nil, err
	}
	approverRaw, err := b64(b.Approver)
	if err != nil || b.Approver == "" {
		return nil, errors.New("no approver: the join key must be under one (PolicyAuthorize)")
	}
	approver, approverName, err := approverKey(approverRaw)
	if err != nil {
		return nil, fmt.Errorf("the approver: %v", err)
	}
	if !bytes.Equal(join.AuthPolicy.Buffer, PolicyAuthorize(approverName)) {
		return nil, errors.New("the join key's policy is not its approver's PolicyAuthorize")
	}
	epochRaw, err := b64(b.Epoch)
	if err != nil {
		return nil, err
	}
	en, err := epochName(epochRaw)
	if err != nil {
		return nil, err
	}
	logRaw, err := b64(b.Log)
	if err != nil {
		return nil, fmt.Errorf("the event log: %v", err)
	}
	log, err := parseLog(logRaw)
	if err != nil {
		return nil, fmt.Errorf("the event log: %v", err)
	}
	var db []byte
	if b.DB != "" {
		if db, err = b64(b.DB); err != nil {
			return nil, fmt.Errorf("the db certificate: %v", err)
		}
		c, err := x509.ParseCertificate(db)
		if err != nil {
			return nil, fmt.Errorf("the db certificate: %v", err)
		}
		if sharedDB(c) {
			return nil, fmt.Errorf("%w: the db certificate is a shared one, not the box's own", ErrSecureBoot)
		}
	}
	var stub []byte
	if b.Stub != "" {
		if stub, err = b64(b.Stub); err != nil {
			return nil, fmt.Errorf("the stub: %v", err)
		}
	}
	c := &Checked{
		EKHash: hex.EncodeToString(h[:]), Vendor: vendor, Model: model, EKPub: ekPub,
		AKName: akName.Buffer, ak: ak, join: join, log: log, db: db, stub: stub,
		approver: approver, epochName: en,
	}
	if db != nil {
		d := sha256.Sum256(db)
		c.DBHash = hex.EncodeToString(d[:])
	}
	return c, nil
}

func parseChain(chain []string) ([]*x509.Certificate, error) {
	if len(chain) == 0 || len(chain) > 8 {
		return nil, errors.New("an EK certificate and at most 7 more")
	}
	var certs []*x509.Certificate
	for i, s := range chain {
		der, err := b64(s)
		if err != nil {
			return nil, fmt.Errorf("certificate %d: %v", i, err)
		}
		c, err := x509.ParseCertificate(der)
		if err != nil {
			return nil, fmt.Errorf("certificate %d: %v", i, err)
		}
		certs = append(certs, c)
	}
	return certs, nil
}

// verifyChain: the EK certificate (first) chains, through the rest, to a
// root of the maker its alternative name says, at the agreed time
func verifyChain(pol *policy, certs []*x509.Certificate, at time.Time) (vendor, model string, err error) {
	leaf := certs[0]
	if !ekMarked(leaf) {
		return "", "", fmt.Errorf("%w: not an EK certificate: no TCG EK purpose or policy", ErrEK)
	}
	vendor, model = tpmInfo(leaf)
	if (software[vendor] || model == "swtpm") && !pol.test.SoftwareTPM {
		return "", "", ErrSoftware
	}
	inter := x509.NewCertPool()
	for _, c := range certs[1:] {
		tpmExtensionsHandled(c)
		inter.AddCert(c)
	}
	tpmExtensionsHandled(leaf)
	verify := func(roots *x509.CertPool) error {
		_, err := leaf.Verify(x509.VerifyOptions{
			Roots:         roots,
			Intermediates: inter,
			CurrentTime:   at,
			KeyUsages:     []x509.ExtKeyUsage{x509.ExtKeyUsageAny},
		})
		return err
	}
	if vendor == "" {
		// AMD's fTPM certificates name no maker: such a certificate is
		// AMD's, issued in AMD's name and chained to AMD's roots, or none
		if !slices.Contains(leaf.Issuer.Organization, amdOrg) {
			return "", "", fmt.Errorf("%w: the EK certificate names no maker", ErrNoRoot)
		}
		vendor = amdVendor
	}
	roots := pol.roots[vendor]
	if roots == nil {
		return "", "", fmt.Errorf("%w: %s", ErrNoRoot, vendorName(vendor))
	}
	if err := verify(roots); err != nil {
		return "", "", fmt.Errorf("%w: the EK certificate does not chain to %s's roots: %v", ErrEK, vendorName(vendor), err)
	}
	return vendor, model, nil
}

func samePub(a, b crypto.PublicKey) bool {
	type equaler interface{ Equal(crypto.PublicKey) bool }
	e, ok := a.(equaler)
	return ok && e.Equal(b)
}

// ekShape: a key credential activation works with - restricted decrypt,
// fixed to its TPM, SHA-256 names and AES-128 - and the RSA 2048 one of the
// TCG's default template (L-1), which every PC TPM has a certificate for:
// a TPM with an ECC EK too would otherwise be two boxes
func ekShape(p *tpm2.TPMTPublic) error {
	a := p.ObjectAttributes
	if !a.FixedTPM || !a.FixedParent || !a.Restricted || !a.Decrypt || a.SignEncrypt {
		return errors.New("the EK is not a restricted decryption key fixed to its TPM")
	}
	if p.NameAlg != tpm2.TPMAlgSHA256 {
		return errors.New("the EK's name is not SHA-256")
	}
	d, err := p.Parameters.RSADetail()
	if p.Type != tpm2.TPMAlgRSA || err != nil || d.KeyBits != 2048 {
		return errors.New("the EK is not RSA 2048")
	}
	sym := d.Symmetric
	bits, err := sym.KeyBits.AES()
	if sym.Algorithm != tpm2.TPMAlgAES || err != nil || *bits != 128 {
		return errors.New("the EK's symmetric key is not AES-128")
	}
	return nil
}

// akShape: a restricted signing key made in, and fixed to, the TPM - what
// it signs that starts TPM_GENERATED, the TPM made
func akShape(p *tpm2.TPMTPublic) error {
	a := p.ObjectAttributes
	if !a.FixedTPM || !a.FixedParent || !a.SensitiveDataOrigin || !a.Restricted || !a.SignEncrypt || a.Decrypt {
		return errors.New("the AK is not a restricted signing key made in its TPM")
	}
	if p.NameAlg != tpm2.TPMAlgSHA256 {
		return errors.New("the AK's name is not SHA-256")
	}
	if p.Type != tpm2.TPMAlgRSA && p.Type != tpm2.TPMAlgECC {
		return errors.New("the AK is neither RSA nor ECC")
	}
	return nil
}

// joinShape: the box's key, made in its TPM, fixed there, used only under
// a policy (PCR 7, checked against the quote)
func joinShape(p *tpm2.TPMTPublic, spki string) error {
	a := p.ObjectAttributes
	if !a.FixedTPM || !a.FixedParent || !a.SensitiveDataOrigin || !a.SignEncrypt || a.UserWithAuth {
		return errors.New("the join key is not a policy-only signing key made in its TPM")
	}
	if p.NameAlg != tpm2.TPMAlgSHA256 || len(p.AuthPolicy.Buffer) != sha256.Size {
		return errors.New("the join key has no SHA-256 policy")
	}
	pub, err := tpm2.Pub(*p)
	if err != nil {
		return fmt.Errorf("the join key: %v", err)
	}
	der, err := b64(spki)
	if err != nil {
		return err
	}
	want, err := x509.ParsePKIXPublicKey(der)
	if err != nil {
		return err
	}
	if !samePub(want, pub) {
		return errors.New("the join key is not this box's key")
	}
	return nil
}

// CredentialFor gives what a validator's challenge is made from: the EK's
// public key and the AK's name.
func CredentialFor(b Begin) (crypto.PublicKey, []byte, error) {
	ek, err := public(b.EK)
	if err != nil {
		return nil, nil, err
	}
	pub, err := tpm2.Pub(*ek)
	if err != nil {
		return nil, nil, err
	}
	ak, err := public(b.AK)
	if err != nil {
		return nil, nil, err
	}
	name, err := tpm2.ObjectName(ak)
	if err != nil {
		return nil, nil, err
	}
	return pub, name.Buffer, nil
}

// Nonce is what the quote and the certification are made over: every
// secret, in validator order, so none was known before the challenges.
func Nonce(box string, secrets map[string][]byte) []byte {
	var names []string
	for v := range secrets {
		names = append(names, v)
	}
	sort.Strings(names)
	h := sha256.New()
	h.Write([]byte("commonty attest v1\x00" + box + "\x00"))
	for _, v := range names {
		h.Write([]byte(v + "\x00"))
		h.Write(secrets[v])
	}
	return h.Sum(nil)
}

// Record is what the agreed state keeps of a box that proved itself.
type Record struct {
	EK         string            `json:"ek"` // Checked.EKHash
	Vendor     string            `json:"vendor"`
	Model      string            `json:"model,omitempty"`
	PCRs       map[string]string `json:"pcrs"` // "0" -> hex, from the quote
	SecureBoot bool              `json:"secureBoot"`
	DB         string            `json:"db,omitempty"` // Checked.DBHash
	At         int64             `json:"at"`           // agreed ms
}

// VerifyResponse judges the box's answer to the challenges.
func VerifyResponse(cfg *Config, c *Checked, chals []Challenge, r Response) (*Record, error) {
	pol, err := cfg.policy()
	if err != nil {
		return nil, err
	}
	return verifyResponse(pol, c, chals, r)
}

func verifyResponse(pol *policy, c *Checked, chals []Challenge, r Response) (*Record, error) {
	if len(chals) == 0 {
		return nil, errors.New("no challenges")
	}
	secrets := map[string][]byte{}
	for _, ch := range chals {
		s, ok := r.Secrets[ch.Validator]
		if !ok {
			return nil, fmt.Errorf("no answer to %s's challenge", ch.Validator)
		}
		raw, err := b64(s)
		if err != nil {
			return nil, err
		}
		h := sha256.Sum256(raw)
		if hex.EncodeToString(h[:]) != ch.Commit {
			return nil, fmt.Errorf("%s's secret is not the one it committed to", ch.Validator)
		}
		secrets[ch.Validator] = raw
	}
	if len(r.Secrets) != len(chals) {
		return nil, errors.New("answers to challenges no one made")
	}
	nonce := Nonce(r.Box, secrets)

	pcrs, err := replay(c.log)
	if err != nil {
		return nil, err
	}
	q, err := attested(c.ak, r.Quote, r.QuoteSig, nonce, tpm2.TPMSTAttestQuote)
	if err != nil {
		return nil, fmt.Errorf("the quote: %v", err)
	}
	qi, err := q.Attested.Quote()
	if err != nil {
		return nil, err
	}
	if !quotesPCRs(qi.PCRSelect) {
		return nil, errors.New("the quote is not of PCRs 0, 2, 4 and 7 in SHA-256")
	}
	d := sha256.New()
	for _, i := range PCRs {
		v := pcrs[i]
		d.Write(v[:])
	}
	if !bytes.Equal(d.Sum(nil), qi.PCRDigest.Buffer) {
		return nil, errors.New("the event log does not give the PCRs quoted")
	}

	// no nonce here (tpm2_certify takes none): the AK is fresh by the
	// activation, and a key fixed to its TPM that this AK certified at any
	// time lives in the AK's TPM
	ct, err := attested(c.ak, r.Certify, r.CertifySig, nil, tpm2.TPMSTAttestCertify)
	if err != nil {
		return nil, fmt.Errorf("the certification: %v", err)
	}
	ci, err := ct.Attested.Certify()
	if err != nil {
		return nil, err
	}
	joinName, err := tpm2.ObjectName(c.join)
	if err != nil {
		return nil, err
	}
	if !bytes.Equal(ci.Name.Buffer, joinName.Buffer) {
		return nil, errors.New("the AK certified another key than the join key")
	}
	// the join key opens only under a boot its approver approved at the
	// counter's epoch: this one is, at the epoch the TPM certifies now
	ec, err := attested(c.ak, r.EpochCert, r.EpochCertSig, nonce, tpm2.TPMSTAttestNV)
	if err != nil {
		return nil, fmt.Errorf("the epoch: %v", err)
	}
	nv, err := ec.Attested.NV()
	if err != nil {
		return nil, err
	}
	if !bytes.Equal(nv.IndexName.Buffer, c.epochName) || nv.Offset != 0 ||
		!bytes.Equal(nv.NVContents.Buffer, binary.BigEndian.AppendUint64(nil, r.Epoch)) {
		return nil, errors.New("the certified epoch is not the counter's, or not the one given")
	}
	approval, err := b64(r.Approval)
	if err != nil {
		return nil, err
	}
	if !ecdsa.VerifyASN1(c.approver, ApprovalDigest(Approved(pcrs[4], pcrs[7], c.epochName, r.Epoch)), approval) {
		return nil, errors.New("this boot is not approved at the epoch: the join key would not open under it")
	}

	sb, err := secureBoot(c.log, c.db)
	if err == nil {
		err = checkBoot(pol.boot, c.log, c.stub)
		sb = err == nil
	}
	if err != nil && !pol.test.NoSecureBoot {
		return nil, err
	}
	rec := &Record{EK: c.EKHash, DB: c.DBHash, Vendor: c.Vendor, Model: c.Model, PCRs: map[string]string{}, SecureBoot: sb}
	for _, i := range PCRs {
		v := pcrs[i]
		rec.PCRs[fmt.Sprint(i)] = hex.EncodeToString(v[:])
	}
	return rec, nil
}

// attested: a TPMS_ATTEST of kind typ over extra, signed by the AK
func attested(ak *tpm2.TPMTPublic, msg64, sig64 string, extra []byte, typ tpm2.TPMST) (*tpm2.TPMSAttest, error) {
	msg, err := b64(msg64)
	if err != nil {
		return nil, err
	}
	sig, err := b64(sig64)
	if err != nil {
		return nil, err
	}
	if err := verifySig(ak, msg, sig); err != nil {
		return nil, err
	}
	a, err := tpm2.Unmarshal[tpm2.TPMSAttest](msg)
	if err != nil {
		return nil, err
	}
	if a.Magic != tpm2.TPMGeneratedValue || a.Type != typ {
		return nil, errors.New("not the TPM's own statement of that kind")
	}
	if extra != nil && !bytes.Equal(a.ExtraData.Buffer, extra) {
		return nil, errors.New("not over the challenges' nonce")
	}
	return a, nil
}

func verifySig(key *tpm2.TPMTPublic, msg, raw []byte) error {
	sig, err := tpm2.Unmarshal[tpm2.TPMTSignature](raw)
	if err != nil {
		return err
	}
	pub, err := tpm2.Pub(*key)
	if err != nil {
		return err
	}
	h := sha256.Sum256(msg)
	switch sig.SigAlg {
	case tpm2.TPMAlgRSASSA:
		s, err := sig.Signature.RSASSA()
		if err != nil {
			return err
		}
		k, ok := pub.(*rsa.PublicKey)
		if !ok || s.Hash != tpm2.TPMAlgSHA256 {
			return errors.New("not an RSA SHA-256 signature by an RSA key")
		}
		return rsa.VerifyPKCS1v15(k, crypto.SHA256, h[:], s.Sig.Buffer)
	case tpm2.TPMAlgECDSA:
		s, err := sig.Signature.ECDSA()
		if err != nil {
			return err
		}
		k, ok := pub.(*ecdsa.PublicKey)
		if !ok || s.Hash != tpm2.TPMAlgSHA256 {
			return errors.New("not an ECDSA SHA-256 signature by an ECC key")
		}
		r := new(big.Int).SetBytes(s.SignatureR.Buffer)
		ss := new(big.Int).SetBytes(s.SignatureS.Buffer)
		if !ecdsa.Verify(k, h[:], r, ss) {
			return errors.New("bad signature")
		}
		return nil
	}
	return fmt.Errorf("a signature of kind %v", sig.SigAlg)
}

func quotesPCRs(sel tpm2.TPMLPCRSelection) bool {
	if len(sel.PCRSelections) != 1 || sel.PCRSelections[0].Hash != tpm2.TPMAlgSHA256 {
		return false
	}
	return bytes.Equal(trimZeros(sel.PCRSelections[0].PCRSelect), pcrMask())
}

// pcrMask: PCRs 0, 2, 4 and 7 as a TPMS_PCR_SELECTION bitmap
func pcrMask() []byte {
	m := make([]byte, 1)
	for _, i := range PCRs {
		m[i/8] |= 1 << (i % 8)
	}
	return m
}

func trimZeros(b []byte) []byte {
	for len(b) > 0 && b[len(b)-1] == 0 {
		b = b[:len(b)-1]
	}
	return b
}
