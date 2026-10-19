package tpm

import (
	"bytes"
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/x509"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"

	"commonty/control/state/attest"

	"github.com/google/go-tpm/tpm2"
)

// The join key across updates (docs/dev/attestation.md): the key is used
// only under a policy the box's approver signed for - one boot's PCRs 4
// and 7 at the current epoch. The approver's private key is sealed under
// that same rule, so only a boot already approved approves the next.
// Its files sit beside the join key in Keys:
//
//	approver.pem, .tpm, .name   its public key, as the TPM loads it, and name
//	join.policy                 PolicyAuthorize by it: the join key's policy
//	approver.pub, .priv         its private key, sealed
//	approvals/<policy>.sig      one approval: its signature over the policy
//	approvals.json              what each approval is for
//	good.json                   the boots that came up approved, newest first

// KeepGood is how many boots, newest first, stay approved: the running
// one and those to roll back to.
const KeepGood = 3

var ErrNotApproved = errors.New("this boot is not one the box approved")

type approval struct {
	Policy string   `json:"policy"`
	Epoch  uint64   `json:"epoch"`
	PCR4   string   `json:"pcr4"`
	Apps   []string `json:"apps"`
}

func (t *Box) k(n string) string { return filepath.Join(t.Keys, n) }

// BootKeyed: this box's join key is under its approver (else: PCR 7 alone,
// as node1's and node2's were made).
func (t *Box) BootKeyed() bool {
	_, err := os.Stat(t.k("approver.tpm"))
	return err == nil
}

func epochName() []byte {
	pub := attest.EpochPublic()
	n, _ := tpm2.NVName(&pub)
	return n.Buffer
}

// EpochPublic: the counter's public area, as Begin carries it
func EpochPublic() []byte {
	pub := attest.EpochPublic()
	return tpm2.Marshal(tpm2.New2B(pub))
}

const epochIndex = "0x01500DD0"

func (t *Box) epoch() (uint64, error) {
	out, err := t.run("tpm2_nvread", epochIndex, "-C", "o")
	if err != nil {
		return 0, err
	}
	if len(out) != 8 {
		return 0, fmt.Errorf("an epoch of %d bytes", len(out))
	}
	return binary.BigEndian.Uint64(out), nil
}

// ensureEpoch: the counter, defined and written once, and the one we
// define (another index there would name another policy)
func (t *Box) ensureEpoch() error {
	if _, err := t.run("tpm2_nvreadpublic", epochIndex); err != nil {
		if _, err := t.run("tpm2_nvdefine", "-Q", epochIndex, "-C", "o", "-s", "8", "-a", "nt=counter|ownerwrite|ownerread|no_da"); err != nil {
			return err
		}
	}
	if _, err := t.epoch(); err != nil {
		if _, err := t.run("tpm2_nvincrement", "-Q", epochIndex, "-C", "o"); err != nil {
			return err
		}
	}
	out, err := t.run("tpm2_nvreadpublic", epochIndex)
	if err != nil {
		return err
	}
	if !bytes.Contains(out, []byte("name: "+hex.EncodeToString(epochName()))) {
		return errors.New("the epoch index is not the counter this box defines")
	}
	return nil
}

func (t *Box) pcrs() (pcr4, pcr7 [32]byte, err error) {
	f := t.p("pcrs.bin")
	if _, err = t.run("tpm2_pcrread", "-Q", "sha256:4,7", "-o", f); err != nil {
		return
	}
	raw, err := os.ReadFile(f)
	if err != nil {
		return
	}
	if len(raw) != 64 {
		return pcr4, pcr7, errors.New("tpm2_pcrread gave no two SHA-256 PCRs")
	}
	return [32]byte(raw[:32]), [32]byte(raw[32:]), nil
}

// current: this boot's PCRs, which its log must give
func (t *Box) current() (log []byte, pcr4, pcr7 [32]byte, err error) {
	if err = os.MkdirAll(t.Dir, 0o700); err != nil {
		return
	}
	if log, err = os.ReadFile(t.Log); err != nil {
		return
	}
	if pcr4, pcr7, err = t.pcrs(); err != nil {
		return
	}
	l4, l7, err := attest.Replay47(log)
	if err != nil {
		return
	}
	if l4 != pcr4 || l7 != pcr7 {
		err = errors.New("the event log does not give this boot's PCRs 4 and 7")
	}
	return
}

func (t *Box) writeFile(name string, data []byte) error {
	tmp := name + ".new"
	if err := os.WriteFile(tmp, data, 0o600); err != nil {
		return err
	}
	return os.Rename(tmp, name)
}

func (t *Box) approvals() ([]approval, error) {
	var as []approval
	raw, err := os.ReadFile(t.k("approvals.json"))
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return as, json.Unmarshal(raw, &as)
}

func (t *Box) good() ([][]string, error) {
	var g [][]string
	raw, err := os.ReadFile(t.k("good.json"))
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	return g, json.Unmarshal(raw, &g)
}

func saveJSON(t *Box, name string, v any) error {
	raw, err := json.MarshalIndent(v, "", " ")
	if err != nil {
		return err
	}
	return t.writeFile(t.k(name), append(raw, '\n'))
}

// approve: the approver's signature for the boot measuring pcr4 and pcr7,
// at epoch, kept in Keys
func (t *Box) approve(key *ecdsa.PrivateKey, pcr4, pcr7 [32]byte, epoch uint64, apps [][]byte) error {
	pol := attest.Approved(pcr4, pcr7, epochName(), epoch)
	sig, err := ecdsa.SignASN1(rand.Reader, key, attest.ApprovalDigest(pol))
	if err != nil {
		return err
	}
	if err := os.MkdirAll(t.k("approvals"), 0o700); err != nil {
		return err
	}
	if err := t.writeFile(t.k("approvals/"+hex.EncodeToString(pol)+".sig"), sig); err != nil {
		return err
	}
	as, err := t.approvals()
	if err != nil {
		return err
	}
	a := approval{Policy: hex.EncodeToString(pol), Epoch: epoch, PCR4: hex.EncodeToString(pcr4[:])}
	for _, d := range apps {
		a.Apps = append(a.Apps, hex.EncodeToString(d))
	}
	as = slices.DeleteFunc(as, func(x approval) bool { return x.Policy == a.Policy })
	return saveJSON(t, "approvals.json", append(as, a))
}

// Session leaves the policy session s satisfied for the join key and the
// approver's seal: PCRs 4 and 7 now, the epoch now, and the approval for
// them (as dd-tpm-policy does for the shell's users).
func (t *Box) Session(s string) error {
	if _, err := t.run("tpm2_startauthsession", "-Q", "--policy-session", "-S", s); err != nil {
		return err
	}
	err := t.satisfy(s)
	if err != nil {
		t.flush(s)
	}
	return err
}

func (t *Box) satisfy(s string) error {
	if _, err := t.run("tpm2_policypcr", "-Q", "-S", s, "-l", "sha256:4,7"); err != nil {
		return err
	}
	e, err := t.run("tpm2_nvread", epochIndex, "-C", "o")
	if err != nil {
		return err
	}
	cur := t.p("cur.pol")
	if _, err := t.runIn(e, "tpm2_policynv", "-Q", "-S", s, "-i-", epochIndex, "eq", "-C", "o", "-L", cur); err != nil {
		return err
	}
	pol, err := os.ReadFile(cur)
	if err != nil {
		return err
	}
	sig := t.k("approvals/" + hex.EncodeToString(pol) + ".sig")
	if _, err := os.Stat(sig); err != nil {
		return ErrNotApproved
	}
	msg := t.p("approval.msg")
	if err := os.WriteFile(msg, append(slices.Clone(pol), attest.ApprovalRef...), 0o600); err != nil {
		return err
	}
	ref := t.p("approval.ref")
	if err := os.WriteFile(ref, attest.ApprovalRef, 0o600); err != nil {
		return err
	}
	if _, err := t.run("tpm2_loadexternal", "-Q", "-C", "o", "-G", "ecc", "-u", t.k("approver.pem"), "-c", t.p("approver.ctx")); err != nil {
		return err
	}
	if _, err := t.run("tpm2_verifysignature", "-Q", "-c", t.p("approver.ctx"), "-g", "sha256", "-m", msg, "-s", sig, "-f", "ecdsa", "-t", t.p("approval.tk")); err != nil {
		return err
	}
	_, err = t.run("tpm2_policyauthorize", "-Q", "-S", s, "-i", cur, "-q", ref, "-n", t.k("approver.name"), "-t", t.p("approval.tk"))
	return err
}

// approver: its private key, unsealed under this boot's approval
func (t *Box) approver() (*ecdsa.PrivateKey, error) {
	if _, err := t.run("tpm2_createprimary", "-Q", "-C", "o", "-G", "ecc256", "-c", t.p("srk.ctx")); err != nil {
		return nil, err
	}
	if _, err := t.run("tpm2_load", "-Q", "-C", t.p("srk.ctx"), "-u", t.k("approver.pub"), "-r", t.k("approver.priv"), "-c", t.p("seal.ctx")); err != nil {
		return nil, err
	}
	s := t.p("s.ctx")
	if err := t.Session(s); err != nil {
		return nil, err
	}
	raw, err := t.run("tpm2_unseal", "-c", t.p("seal.ctx"), "-p", "session:"+s)
	t.flush(s)
	if err != nil {
		return nil, err
	}
	return ecdsa.ParseRawPrivateKey(elliptic.P256(), raw)
}

// Enrol makes the join key under the approver, the approver and its
// seal, and approves this boot. Run once, on the box's first boot under
// its own Secure Boot keys.
func (t *Box) Enrol() error {
	if _, err := os.Stat(t.k("sig.pub")); err == nil {
		return errors.New("already enrolled: " + t.k("sig.pub"))
	}
	for _, d := range []string{t.Dir, t.Keys} {
		if err := os.MkdirAll(d, 0o700); err != nil {
			return err
		}
	}
	log, pcr4, pcr7, err := t.current()
	if err != nil {
		return err
	}
	if err := t.ensureEpoch(); err != nil {
		return err
	}
	key, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		return err
	}
	der, err := x509.MarshalPKIXPublicKey(&key.PublicKey)
	if err != nil {
		return err
	}
	if err := t.writeFile(t.k("approver.pem"), pem.EncodeToMemory(&pem.Block{Type: "PUBLIC KEY", Bytes: der})); err != nil {
		return err
	}
	if _, err := t.run("tpm2_loadexternal", "-Q", "-C", "o", "-G", "ecc", "-u", t.k("approver.pem"), "-c", t.p("approver.ctx"), "-n", t.k("approver.name")); err != nil {
		return err
	}
	if _, err := t.run("tpm2_readpublic", "-Q", "-c", t.p("approver.ctx"), "-o", t.k("approver.tpm")); err != nil {
		return err
	}
	name, err := os.ReadFile(t.k("approver.name"))
	if err != nil {
		return err
	}
	pol := t.k("join.policy") // for what else is sealed beside the join key
	if err := t.writeFile(pol, attest.PolicyAuthorize(name)); err != nil {
		return err
	}
	if _, err := t.run("tpm2_createprimary", "-Q", "-C", "o", "-G", "ecc256", "-c", t.p("srk.ctx")); err != nil {
		return err
	}
	priv, err := key.Bytes()
	if err != nil {
		return err
	}
	if _, err := t.runIn(priv, "tpm2_create", "-Q", "-C", t.p("srk.ctx"), "-L", pol, "-a", "fixedtpm|fixedparent|noda",
		"-i-", "-u", t.k("approver.pub"), "-r", t.k("approver.priv")); err != nil {
		return err
	}
	clear(priv)
	if _, err := t.run("tpm2_create", "-Q", "-C", t.p("srk.ctx"), "-G", "ecc256:ecdsa-sha256", "-L", pol,
		"-a", "fixedtpm|fixedparent|sensitivedataorigin|sign|noda", "-u", t.k("sig.pub"), "-r", t.k("sig.priv")); err != nil {
		return err
	}
	e, err := t.epoch()
	if err != nil {
		return err
	}
	apps, err := attest.BootApps(log)
	if err != nil {
		return err
	}
	if err := t.approve(key, pcr4, pcr7, e, apps); err != nil {
		return err
	}
	return saveJSON(t, "good.json", [][]string{hexes(apps)})
}

func hexes(ds [][]byte) []string {
	var out []string
	for _, d := range ds {
		out = append(out, hex.EncodeToString(d))
	}
	return out
}

func unhex(ss []string) ([][]byte, error) {
	var out [][]byte
	for _, s := range ss {
		d, err := hex.DecodeString(s)
		if err != nil {
			return nil, err
		}
		out = append(out, d)
	}
	return out, nil
}

// Approve approves, at the current epoch, the boots of these images
// (each the boot applications in the order the firmware runs them -
// systemd-boot, then the entry's image): what this boot's log gives with
// their digests in place of its own. Run before restarting into them,
// from a boot already approved.
func (t *Box) Approve(boots [][]string) error {
	log, _, pcr7, err := t.current()
	if err != nil {
		return err
	}
	e, err := t.epoch()
	if err != nil {
		return err
	}
	key, err := t.approver()
	if err != nil {
		return err
	}
	for _, files := range boots {
		var apps [][]byte
		for _, f := range files {
			img, err := os.ReadFile(f)
			if err != nil {
				return err
			}
			d, err := attest.Authenticode(img)
			if err != nil {
				return fmt.Errorf("%s: %v", f, err)
			}
			apps = append(apps, d)
		}
		pcr4, err := attest.PredictPCR4(log, apps)
		if err != nil {
			return err
		}
		if err := t.approve(key, pcr4, pcr7, e, apps); err != nil {
			return err
		}
	}
	return nil
}

// Settle, at each boot: a boot that came up approved and is new joins the
// good ones (KeepGood, newest first); they are approved for the next
// epoch - each also with this boot's systemd-boot, which lzbt may have
// replaced - and the counter raised, which retires every approval before.
// A boot not approved changes nothing (ErrNotApproved).
func (t *Box) Settle() error {
	log, pcr4, pcr7, err := t.current()
	if err != nil {
		return err
	}
	e, err := t.epoch()
	if err != nil {
		return err
	}
	if _, err := os.Stat(t.k("approvals/" + hex.EncodeToString(attest.Approved(pcr4, pcr7, epochName(), e)) + ".sig")); err != nil {
		return ErrNotApproved
	}
	apps, err := attest.BootApps(log)
	if err != nil {
		return err
	}
	good, err := t.good()
	if err != nil {
		return err
	}
	now := hexes(apps)
	if len(good) > 0 && slices.Equal(good[0], now) {
		return nil
	}
	good = slices.DeleteFunc(good, func(g []string) bool { return slices.Equal(g, now) })
	good = append([][]string{now}, good...)
	if len(good) > KeepGood {
		good = good[:KeepGood]
	}
	key, err := t.approver()
	if err != nil {
		return err
	}
	for _, g := range good {
		gapps, err := unhex(g)
		if err != nil {
			return err
		}
		variants := [][][]byte{gapps}
		if len(gapps) == len(apps) && len(apps) > 0 && !bytes.Equal(gapps[0], apps[0]) {
			v := slices.Clone(gapps)
			v[0] = apps[0]
			variants = append(variants, v)
		}
		for _, v := range variants {
			p4, err := attest.PredictPCR4(log, v)
			if err != nil {
				return err
			}
			if err := t.approve(key, p4, pcr7, e+1, v); err != nil {
				return err
			}
		}
	}
	if _, err := t.run("tpm2_nvincrement", "-Q", epochIndex, "-C", "o"); err != nil {
		return err
	}
	if err := saveJSON(t, "good.json", good); err != nil {
		return err
	}
	as, err := t.approvals()
	if err != nil {
		return err
	}
	var keep []approval
	for _, a := range as {
		if a.Epoch > e {
			keep = append(keep, a)
		} else {
			_ = os.Remove(t.k("approvals/" + a.Policy + ".sig"))
		}
	}
	return saveJSON(t, "approvals.json", keep)
}

// ESPBoots: what a lanzaboote ESP can start - systemd-boot, then each of
// the newest generations' images (KeepGood of them)
func ESPBoots(esp string) ([][]string, error) {
	loader := filepath.Join(esp, "EFI", "systemd", "systemd-bootx64.efi")
	entries, err := filepath.Glob(filepath.Join(esp, "EFI", "Linux", "nixos-generation-*.efi"))
	if err != nil {
		return nil, err
	}
	gen := func(f string) int {
		s := strings.TrimPrefix(filepath.Base(f), "nixos-generation-")
		end := strings.IndexFunc(s, func(r rune) bool { return r < '0' || r > '9' })
		if end < 0 {
			end = len(s)
		}
		n, _ := strconv.Atoi(s[:end])
		return n
	}
	sort.Slice(entries, func(i, j int) bool { return gen(entries[i]) > gen(entries[j]) })
	var boots [][]string
	for i, f := range entries {
		if i == KeepGood {
			break
		}
		boots = append(boots, []string{loader, f})
	}
	return boots, nil
}

// bootKeyed: what Begin carries of the approver and the epoch counter
func (t *Box) beginBootKey(b *attest.Begin) error {
	raw, err := os.ReadFile(t.k("approver.tpm"))
	if err != nil {
		return err
	}
	b.Approver = base64.StdEncoding.EncodeToString(raw)
	b.Epoch = base64.StdEncoding.EncodeToString(EpochPublic())
	return nil
}

// respondBootKey: this boot's approval, and the epoch certified by the AK
// over nonce (hex)
func (t *Box) respondBootKey(r *attest.Response, nonce string) error {
	_, pcr4, pcr7, err := t.current()
	if err != nil {
		return err
	}
	e, err := t.epoch()
	if err != nil {
		return err
	}
	sig, err := os.ReadFile(t.k("approvals/" + hex.EncodeToString(attest.Approved(pcr4, pcr7, epochName(), e)) + ".sig"))
	if err != nil {
		return ErrNotApproved
	}
	if _, err := t.run("tpm2_nvcertify", "-Q", "-C", t.p("ak.ctx"), "-c", "o", "-g", "sha256", "-s", "rsassa",
		"-q", nonce, "-o", t.p("epoch.sig"), "--attestation", t.p("epoch.msg"), "--size", "8", epochIndex); err != nil {
		return err
	}
	msg, err := os.ReadFile(t.p("epoch.msg"))
	if err != nil {
		return err
	}
	esig, err := os.ReadFile(t.p("epoch.sig"))
	if err != nil {
		return err
	}
	r.Epoch, r.Approval = e, base64.StdEncoding.EncodeToString(sig)
	r.EpochCert, r.EpochCertSig = base64.StdEncoding.EncodeToString(msg), base64.StdEncoding.EncodeToString(esig)
	return nil
}
