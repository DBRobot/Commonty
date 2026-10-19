// Package tpm is attestation's two live ends (state/attest judges): a
// validator's challenge, made with a fresh secret, and the box's side, its
// TPM driven through tpm2-tools as dd-control already does to sign.
// docs/dev/attestation.md
package tpm

import (
	"bytes"
	"context"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unicode/utf16"

	"commonty/control/state/attest"

	legacy "github.com/google/go-tpm/legacy/tpm2"
	"github.com/google/go-tpm/legacy/tpm2/credactivation"
	"github.com/google/go-tpm/tpm2"
)

// MakeChallenge: validator's challenge of box, from its Begin. The secret
// goes nowhere but sealed in the challenge; the agreed state holds its hash.
func MakeChallenge(b attest.Begin, box, validator string) (attest.Challenge, error) {
	ek, name, err := attest.CredentialFor(b)
	if err != nil {
		return attest.Challenge{}, err
	}
	secret := make([]byte, 32)
	if _, err := rand.Read(secret); err != nil {
		return attest.Challenge{}, err
	}
	// the AK's name is nameAlg || digest; the credential binds the digest
	cred, enc, err := credactivation.Generate(&legacy.HashValue{Alg: legacy.AlgSHA256, Value: name[2:]}, ek, 16, secret)
	if err != nil {
		return attest.Challenge{}, err
	}
	h := sha256.Sum256(secret)
	return attest.Challenge{
		Box: box, Validator: validator,
		Cred:   base64.StdEncoding.EncodeToString(cred),
		Secret: base64.StdEncoding.EncodeToString(enc),
		Commit: hex.EncodeToString(h[:]),
	}, nil
}

// Box is this box's TPM.
type Box struct {
	Dir  string // where the AK is kept between Begin and Respond
	Keys string // the join key's files (sig.pub, sig.priv), /boot/dd/unlock
	TCTI string // device:/dev/tpmrm0
	// the firmware's event log
	Log string
	// the box's own Secure Boot certificate (PEM or DER); none: Secure Boot off
	DB string
	// the lanzaboote image this boot ran; empty: the ESP's entry that
	// systemd-boot says it chose (LoaderEntrySelected)
	Stub string
	ESP  string
	// fetches an issuer's certificate (an AIA link); nil: none fetched
	Fetch func(url string) ([]byte, error)
	// no resource manager in front of the TPM (a bare swtpm): objects stay
	// loaded after each command, so each is flushed after it
	NoRM bool
}

func Default() *Box {
	return &Box{
		Dir:   "/var/lib/dd-control/attest",
		Keys:  "/boot/dd/unlock",
		TCTI:  "device:/dev/tpmrm0",
		Log:   "/sys/kernel/security/tpm0/binary_bios_measurements",
		DB:    "/var/lib/sbctl/keys/db/db.pem",
		ESP:   "/boot",
		Fetch: Fetch,
	}
}

func (t *Box) run(args ...string) ([]byte, error) { return t.runIn(nil, args...) }

// runIn: run with stdin
func (t *Box) runIn(stdin []byte, args ...string) ([]byte, error) {
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	cmd := exec.CommandContext(ctx, args[0], args[1:]...)
	cmd.Env = append(os.Environ(), "TPM2TOOLS_TCTI="+t.TCTI)
	if stdin != nil {
		cmd.Stdin = bytes.NewReader(stdin)
	}
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	out, err := cmd.Output()
	if t.NoRM {
		flush := exec.Command("tpm2_flushcontext", "-t")
		flush.Env = cmd.Env
		_ = flush.Run()
	}
	if err != nil {
		return nil, fmt.Errorf("%s: %v: %s", args[0], err, strings.TrimSpace(stderr.String()))
	}
	return out, nil
}

// flush: an object's context file is reloaded by each command that names
// it, and dropped at its end by the resource manager (or NoRM's flush);
// a session's is flushed here
func (t *Box) flush(session string) { _, _ = t.run("tpm2_flushcontext", session) }

func (t *Box) p(n string) string { return filepath.Join(t.Dir, n) }

// EK certificates in NV (TCG EK Credential Profile): RSA 2048, and the
// TPM's own chain above it
const (
	nvRSA   = "0x1C00002"
	nvChain = "0x1C00100"
)

func (t *Box) nv(index string) ([]byte, error) {
	return t.run("tpm2_nvread", index)
}

// Begin: what the box's request to enlist carries. It makes the AK, once.
func (t *Box) Begin() (attest.Begin, error) {
	var b attest.Begin
	if err := os.MkdirAll(t.Dir, 0o700); err != nil {
		return b, err
	}
	// the RSA EK alone (attest's ekShape): one key per TPM
	if _, err := t.run("tpm2_createek", "-Q", "-c", t.p("ek.ctx"), "-G", "rsa", "-u", t.p("ek.pub")); err != nil {
		return b, err
	}
	cert, err := t.ekCert()
	if err != nil {
		return b, err
	}
	if _, err := os.Stat(t.p("ak.priv")); err != nil {
		if _, err := t.run("tpm2_createak", "-Q", "-C", t.p("ek.ctx"), "-c", t.p("ak.ctx"), "-G", "rsa", "-g", "sha256", "-s", "rsassa",
			"-u", t.p("ak.pub"), "-r", t.p("ak.priv"), "-n", t.p("ak.name")); err != nil {
			return b, err
		}
	}
	chain := [][]byte{cert}
	if raw, err := t.nv(nvChain); err == nil {
		chain = append(chain, splitDER(raw)...)
	}
	chain = t.issuers(chain)
	for _, c := range chain {
		b.Chain = append(b.Chain, base64.StdEncoding.EncodeToString(c))
	}
	files := map[string]*string{t.p("ek.pub"): &b.EK, t.p("ak.pub"): &b.AK, filepath.Join(t.Keys, "sig.pub"): &b.Join, t.Log: &b.Log}
	for f, dst := range files {
		raw, err := os.ReadFile(f)
		if err != nil {
			return b, err
		}
		*dst = base64.StdEncoding.EncodeToString(raw)
	}
	if t.BootKeyed() {
		if err := t.beginBootKey(&b); err != nil {
			return b, err
		}
	}
	if stub := t.stub(); stub != "" {
		raw, err := os.ReadFile(stub)
		if err != nil {
			return b, fmt.Errorf("the booted stub: %v", err)
		}
		b.Stub = base64.StdEncoding.EncodeToString(raw)
	}
	if t.DB != "" {
		if raw, err := os.ReadFile(t.DB); err == nil {
			if blk, _ := pem.Decode(raw); blk != nil {
				raw = blk.Bytes
			}
			b.DB = base64.StdEncoding.EncodeToString(raw)
		}
	}
	return b, nil
}

// systemd-boot's vendor GUID, its variables' suffix
const loaderVars = "4a67b082-0a4c-41cf-b6c7-440b29bb8c4f"

// stub: the lanzaboote image this boot ran - Stub, or the ESP's EFI/Linux
// entry systemd-boot chose; empty when neither is there
func (t *Box) stub() string {
	if t.Stub != "" || t.ESP == "" {
		return t.Stub
	}
	raw, err := os.ReadFile("/sys/firmware/efi/efivars/LoaderEntrySelected-" + loaderVars)
	if err != nil || len(raw) < 6 {
		return ""
	}
	name := utf16le(raw[4:])
	if name == "" || strings.ContainsAny(name, "/\\") || strings.HasPrefix(name, ".") {
		return ""
	}
	return filepath.Join(t.ESP, "EFI", "Linux", name)
}

// utf16le: an EFI variable's UTF-16 string, up to its NUL
func utf16le(b []byte) string {
	var u []uint16
	for i := 0; i+1 < len(b); i += 2 {
		c := binary.LittleEndian.Uint16(b[i:])
		if c == 0 {
			break
		}
		u = append(u, c)
	}
	return string(utf16.Decode(u))
}

// ekCert: the RSA EK's certificate, from NV, or for an AMD fTPM (none in
// NV) from AMD's service
func (t *Box) ekCert() ([]byte, error) {
	if cert, err := t.nv(nvRSA); err == nil {
		return oneDER(cert), nil
	}
	props, err := t.run("tpm2_getcap", "properties-fixed")
	if err != nil {
		return nil, err
	}
	if manufacturer(props) != amdVendor || t.Fetch == nil {
		return nil, errors.New("no RSA EK certificate in the TPM")
	}
	raw, err := os.ReadFile(t.p("ek.pub"))
	if err != nil {
		return nil, err
	}
	url, err := AMDEKCertURL(raw)
	if err != nil {
		return nil, err
	}
	cert, err := t.Fetch(url)
	if err != nil {
		return nil, fmt.Errorf("the EK certificate from AMD: %v", err)
	}
	return cert, nil
}

const amdVendor = "414D4400"

// manufacturer: TPM2_PT_MANUFACTURER's raw value, hex, from tpm2_getcap
// properties-fixed
func manufacturer(props []byte) string {
	_, rest, ok := bytes.Cut(props, []byte("TPM2_PT_MANUFACTURER:"))
	if !ok {
		return ""
	}
	_, rest, ok = bytes.Cut(rest, []byte("raw:"))
	if !ok {
		return ""
	}
	v, _, _ := bytes.Cut(rest, []byte("\n"))
	n, err := strconv.ParseUint(string(bytes.TrimSpace(v)), 0, 32)
	if err != nil {
		return ""
	}
	return fmt.Sprintf("%08X", n)
}

// AMDEKCertURL: where AMD serves an fTPM's EK certificate, from the EK's
// TPM2B_PUBLIC - the first 16 bytes of sha256(00 00 22 22 || exponent ||
// modulus), hex, as tpm2-tools' tpm2_getekcertificate makes it
func AMDEKCertURL(ekPub []byte) (string, error) {
	p2b, err := tpm2.Unmarshal[tpm2.TPM2BPublic](ekPub)
	if err != nil {
		return "", err
	}
	pub, err := p2b.Contents()
	if err != nil {
		return "", err
	}
	d, err := pub.Parameters.RSADetail()
	if pub.Type != tpm2.TPMAlgRSA || err != nil {
		return "", errors.New("not an RSA EK")
	}
	n, err := pub.Unique.RSA()
	if err != nil {
		return "", err
	}
	exp := d.Exponent
	if exp == 0 {
		exp = 65537
	}
	h := sha256.New()
	h.Write([]byte{0, 0, 0x22, 0x22})
	_ = binary.Write(h, binary.BigEndian, exp)
	h.Write(n.Buffer)
	return "https://ftpm.amd.com/pki/aia/" + hex.EncodeToString(h.Sum(nil)[:16]), nil
}

// issuers: up the AIA links from the top of chain until a self-signed
// certificate (a root, which the release has and the chain leaves out)
func (t *Box) issuers(chain [][]byte) [][]byte {
	if t.Fetch == nil {
		return chain
	}
	for range 4 {
		top, err := x509.ParseCertificate(chain[len(chain)-1])
		if err != nil || len(top.IssuingCertificateURL) == 0 || bytes.Equal(top.RawIssuer, top.RawSubject) {
			break
		}
		raw, err := t.Fetch(top.IssuingCertificateURL[0])
		if err != nil {
			break
		}
		up, err := x509.ParseCertificate(raw)
		if err != nil || bytes.Equal(up.RawIssuer, up.RawSubject) {
			break
		}
		chain = append(chain, raw)
	}
	return chain
}

// Fetch gets one certificate, DER or PEM, from a maker's AIA link.
func Fetch(url string) ([]byte, error) {
	if !strings.HasPrefix(url, "http://") && !strings.HasPrefix(url, "https://") {
		return nil, fmt.Errorf("not a web link: %s", url)
	}
	c := http.Client{Timeout: 20 * time.Second}
	r, err := c.Get(url)
	if err != nil {
		return nil, err
	}
	defer r.Body.Close()
	if r.StatusCode != 200 {
		return nil, fmt.Errorf("%s: %s", url, r.Status)
	}
	raw, err := io.ReadAll(io.LimitReader(r.Body, 64<<10))
	if err != nil {
		return nil, err
	}
	if blk, _ := pem.Decode(raw); blk != nil {
		raw = blk.Bytes
	}
	return raw, nil
}

// oneDER: an NV index is often bigger than its certificate; cut the rest
func oneDER(b []byte) []byte {
	if all := splitDER(b); len(all) > 0 {
		return all[0]
	}
	return b
}

// splitDER: certificates back to back, as the chain index holds them
func splitDER(b []byte) [][]byte {
	var out [][]byte
	for len(b) > 4 && b[0] == 0x30 {
		n, h := 0, 2
		switch l := b[1]; {
		case l < 0x80:
			n = int(l)
		case l == 0x81:
			n, h = int(b[2]), 3
		case l == 0x82:
			n, h = int(binary.BigEndian.Uint16(b[2:4])), 4
		default:
			return out
		}
		if h+n > len(b) {
			return out
		}
		out = append(out, b[:h+n])
		b = b[h+n:]
	}
	return out
}

// Respond: the box's answer to the challenges made of it.
func (t *Box) Respond(box string, chals []attest.Challenge) (attest.Response, error) {
	r := attest.Response{Box: box, Secrets: map[string]string{}}
	if _, err := t.run("tpm2_createek", "-Q", "-c", t.p("ek.ctx"), "-G", "rsa", "-u", t.p("ek.pub")); err != nil {
		return r, err
	}
	// the EK's own policy: the endorsement hierarchy's secret, per use
	ekSession := func() (string, func(), error) {
		s := t.p("s.ctx")
		if _, err := t.run("tpm2_startauthsession", "-Q", "--policy-session", "-S", s); err != nil {
			return "", nil, err
		}
		if _, err := t.run("tpm2_policysecret", "-Q", "-S", s, "-c", "e"); err != nil {
			t.flush(s)
			return "", nil, err
		}
		return "session:" + s, func() { t.flush(s) }, nil
	}
	sess, done, err := ekSession()
	if err != nil {
		return r, err
	}
	_, err = t.run("tpm2_load", "-Q", "-C", t.p("ek.ctx"), "-P", sess, "-u", t.p("ak.pub"), "-r", t.p("ak.priv"), "-c", t.p("ak.ctx"))
	done()
	if err != nil {
		return r, err
	}

	secrets := map[string][]byte{}
	for _, c := range chals {
		blob, err := credFile(c)
		if err != nil {
			return r, err
		}
		if err := os.WriteFile(t.p("cred"), blob, 0o600); err != nil {
			return r, err
		}
		sess, done, err := ekSession()
		if err != nil {
			return r, err
		}
		_, err = t.run("tpm2_activatecredential", "-Q", "-c", t.p("ak.ctx"), "-C", t.p("ek.ctx"), "-P", sess, "-i", t.p("cred"), "-o", t.p("secret"))
		done()
		if err != nil {
			return r, fmt.Errorf("%s's challenge: %v", c.Validator, err)
		}
		s, err := os.ReadFile(t.p("secret"))
		os.Remove(t.p("secret"))
		if err != nil {
			return r, err
		}
		secrets[c.Validator] = s
		r.Secrets[c.Validator] = base64.StdEncoding.EncodeToString(s)
	}
	nonce := hex.EncodeToString(attest.Nonce(box, secrets))
	if _, err := t.run("tpm2_quote", "-Q", "-c", t.p("ak.ctx"), "-l", "sha256:0,2,4,7", "-q", nonce, "-g", "sha256",
		"-m", t.p("quote.msg"), "-s", t.p("quote.sig")); err != nil {
		return r, err
	}
	if _, err := t.run("tpm2_createprimary", "-Q", "-C", "o", "-G", "ecc256", "-c", t.p("srk.ctx")); err != nil {
		return r, err
	}
	_, err = t.run("tpm2_load", "-Q", "-C", t.p("srk.ctx"), "-u", filepath.Join(t.Keys, "sig.pub"), "-r", filepath.Join(t.Keys, "sig.priv"), "-c", t.p("sig.ctx"))
	if err != nil {
		return r, err
	}
	// certifying is the key's admin role: no policy, its empty password
	if _, err := t.run("tpm2_certify", "-Q", "-c", t.p("sig.ctx"), "-C", t.p("ak.ctx"), "-g", "sha256",
		"-o", t.p("certify.msg"), "-s", t.p("certify.sig")); err != nil {
		return r, err
	}
	if t.BootKeyed() {
		if err := t.respondBootKey(&r, nonce); err != nil {
			return r, err
		}
	}
	files := map[string]*string{"quote.msg": &r.Quote, "quote.sig": &r.QuoteSig, "certify.msg": &r.Certify, "certify.sig": &r.CertifySig}
	for f, dst := range files {
		raw, err := os.ReadFile(t.p(f))
		if err != nil {
			return r, err
		}
		*dst = base64.StdEncoding.EncodeToString(raw)
	}
	return r, nil
}

// credFile: a challenge as tpm2_activatecredential reads it
func credFile(c attest.Challenge) ([]byte, error) {
	cred, err := base64.StdEncoding.DecodeString(c.Cred)
	if err != nil {
		return nil, err
	}
	secret, err := base64.StdEncoding.DecodeString(c.Secret)
	if err != nil {
		return nil, err
	}
	out := []byte{0xBA, 0xDC, 0xC0, 0xDE, 0, 0, 0, 1}
	return append(append(out, cred...), secret...), nil
}
