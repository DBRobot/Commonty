package main

// Signing with the box's TPM-held key, under its signed boot: the key on the
// boot partition (sig.pub, sig.priv). One dd-unlock-enrol made (node1's and
// node2's) is PCR 7's; one made under the box's approver (approver.tpm beside
// it, dd-boot-key) opens under PCRs 4 and 7 as the approver approved
// (docs/dev/attestation.md).
//
// One connection to /dev/tpmrm0 for the life of the process, one signature
// at a time. The key is loaded once and kept; a policy session is made for
// each signature and flushed after it. Nothing is ever saved to a file, so
// a dd-control killed mid-signature leaves nothing behind in the TPM: the
// kernel flushes what a connection held when it closes.
//
// It used to run tpm2-tools, five processes a signature, each saving the
// policy session to a file for the next. A dd-control stopped between two
// of them left that session saved in the TPM for good, and once the TPM had
// saved 65535 contexts since the oldest one it refused to save any more
// (TPM_RC_CONTEXT_GAP): every signature failed, heartbeats and lease renewals
// with them, and node1's writers moved to node2 (2026-10-24 23:52).
//
// So on an error it starts afresh (a new connection, the key loaded again)
// and tries again for a while; and when the TPM looks out of room for
// contexts, it first flushes the sessions saved or loaded in it, whoever's
// they are: by then they are what keeps every TPM user here failing.

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/sha256"
	"crypto/x509"
	"encoding/asn1"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/fs"
	"log"
	"math/big"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"syscall"
	"time"

	"commonty/control/state"
	"commonty/control/state/attest"
	"commonty/control/tpm"

	"github.com/google/go-tpm/tpm2"
	"github.com/google/go-tpm/tpm2/transport"
	"github.com/google/go-tpm/tpm2/transport/linuxtpm"
)

// tpmLock is the file every user of the TPM on a box takes (flock) around
// its use: dd-control, control/tpm's users, and the scripts that sign with
// tpm2-tools (gate/box-sign.sh, gate/acme-ask.sh, mail-send.sh), all root,
// as the device is root's alone. In a directory only root may enter, the
// file root's and 0600: a user who could open it could hold it, and every
// signature here would wait on them (box/control.nix makes both)
const tpmLock = tpm.LockPath

// tpmDevice is what the signer needs of a TPM: the real one (tpmBackend)
// or a test's
type tpmDevice interface {
	// sign: an ECDSA signature (DER) over a SHA-256 digest
	sign(digest []byte) ([]byte, error)
	// reset drops the connection and what it holds; the next sign opens
	// a new one and loads the key again
	reset()
	// flushStale flushes every session saved or loaded in the TPM: how
	// many it flushed
	flushStale() (int, error)
}

// errSigning: the TPM did not sign. Its burst is logged once, here; the
// callers need not log it again
type errSigning struct{ err error }

func (e errSigning) Error() string { return "the TPM did not sign: " + e.err.Error() }
func (e errSigning) Unwrap() error { return e.err }

type signer struct {
	mu  sync.Mutex
	dev tpmDevice
	// the box-wide lock file; "" for none
	lock string
	// how long one signature is tried for: well within a lease (45 s),
	// which a renewal every 5 s keeps
	budget time.Duration
	// the first wait between tries, doubled each time up to maxWait
	wait, maxWait time.Duration
	sleep         func(time.Duration)
	now           func() time.Time
	logf          func(format string, a ...any)
	// tests only: an error in place of the TPM's while it says one
	fault func() error
	// the burst under way: failures since the last signature, from when
	failures int
	since    time.Time
	flushed  int
	// the lock file could not be opened: said once
	noLock bool
}

func newSigner(dev tpmDevice) *signer {
	return &signer{
		dev: dev, lock: tpmLock, budget: 8 * time.Second,
		wait: 100 * time.Millisecond, maxWait: 2 * time.Second,
		sleep: time.Sleep, now: time.Now, logf: log.Printf,
	}
}

// Sign signs msg, retrying what may pass, and returns the signature as
// base64url
func (s *signer) Sign(msg []byte) (string, error) {
	digest := sha256.Sum256(msg)
	s.mu.Lock()
	defer s.mu.Unlock()
	start := s.now()
	wait := s.wait
	// a burst older than one signature's tries: one try each, so a round
	// of renewals is not held up while the TPM stays down. The callers'
	// own rounds are the retries then
	budget := s.budget
	if s.failures > 0 && start.Sub(s.since) > s.budget {
		budget = 0
	}
	for {
		sig, err := s.once(digest[:])
		if err == nil {
			if s.failures > 0 {
				s.logf("tpm: signing again, after %d failed tries over %s", s.failures, s.now().Sub(s.since).Round(time.Millisecond))
				s.failures, s.flushed = 0, 0
			}
			return base64.RawURLEncoding.EncodeToString(sig), nil
		}
		s.dev.reset()
		gap := contextGap(err)
		flushed := 0
		if gap {
			flushed = s.flush()
		}
		if s.failures == 0 {
			s.since = s.now()
			why := "retrying"
			if gap {
				why = fmt.Sprintf("out of room for contexts: flushed %d stale sessions, retrying", flushed)
			}
			s.logf("tpm: a signature failed (%v); %s, and saying nothing more until one succeeds", err, why)
		} else if gap && flushed > 0 && s.flushed == 0 {
			s.logf("tpm: out of room for contexts: flushed %d stale sessions", flushed)
		}
		s.failures++
		s.flushed += flushed
		if permanent(err) || s.now().Sub(start)+wait > budget {
			return "", errSigning{err}
		}
		s.sleep(wait)
		wait = min(wait*2, s.maxWait)
	}
}

// once: one try, under the box-wide lock
func (s *signer) once(digest []byte) ([]byte, error) {
	unlock := s.take()
	defer unlock()
	if s.fault != nil {
		if err := s.fault(); err != nil {
			return nil, err
		}
	}
	return s.dev.sign(digest)
}

func (s *signer) flush() int {
	unlock := s.take()
	defer unlock()
	n, err := s.dev.flushStale()
	if err != nil {
		s.logf("tpm: flushing stale sessions: %v", err)
	}
	return n
}

// take holds the box-wide lock, if there is one: a lock held by another
// for longer than a signature may take is waited out only so long, and
// then the TPM is used anyway
func (s *signer) take() func() {
	if s.lock == "" {
		return func() {}
	}
	f, err := openLock(s.lock)
	if err != nil {
		if !s.noLock {
			s.logf("tpm: %v; signing without the lock", err)
		}
		s.noLock = true
		return func() {}
	}
	deadline := s.now().Add(5 * time.Second)
	for {
		err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
		if err == nil {
			return func() { _ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN); _ = f.Close() }
		}
		if !errors.Is(err, syscall.EWOULDBLOCK) || s.now().After(deadline) {
			_ = f.Close()
			return func() {}
		}
		time.Sleep(20 * time.Millisecond)
	}
}

func openLock(path string) (*os.File, error) { return tpm.OpenLock(path) }

// contextGap: the TPM refused to save a context. Through the kernel's
// resource manager that comes back as EFAULT from the device (the kernel
// logs "tpm2_save_context: failed with a TPM error 0x0901"); a TPM spoken
// to directly says TPM_RC_CONTEXT_GAP. Out of room for sessions or objects
// is cleared the same way
func contextGap(err error) bool {
	return errors.Is(err, tpm2.TPMRCContextGap) || errors.Is(err, tpm2.TPMRCSessionMemory) ||
		errors.Is(err, tpm2.TPMRCObjectMemory) || errors.Is(err, tpm2.TPMRCSessionHandles) ||
		errors.Is(err, syscall.EFAULT) || strings.Contains(err.Error(), "bad address")
}

// permanent: what no retry within a lease mends. PCR 7 not as the key's
// policy wants (disk-check spoils it on a pool not this box's), a key that
// does not load under this TPM's primary, or no key files at all
func permanent(err error) bool {
	return errors.Is(err, tpm2.TPMRCPolicyFail) || errors.Is(err, tpm2.TPMRCIntegrity) ||
		errors.Is(err, fs.ErrNotExist)
}

// tpmBackend: the TPM itself, through the kernel's resource manager
type tpmBackend struct {
	keys string
	open func() (transport.TPMCloser, error)
	t    transport.TPMCloser
	key  tpm2.NamedHandle
}

func newTPMBackend(keys string) *tpmBackend {
	return &tpmBackend{keys: keys, open: func() (transport.TPMCloser, error) { return linuxtpm.Open("/dev/tpmrm0") }}
}

// srkTemplate is what `tpm2_createprimary -C o -G ecc256` makes (tpm2-tools
// 5.7), the parent dd-unlock-enrol made the key under: the same template
// gives the same key from the owner's seed
var srkTemplate = tpm2.TPMTPublic{
	Type:    tpm2.TPMAlgECC,
	NameAlg: tpm2.TPMAlgSHA256,
	ObjectAttributes: tpm2.TPMAObject{
		FixedTPM: true, FixedParent: true, SensitiveDataOrigin: true,
		UserWithAuth: true, Restricted: true, Decrypt: true,
	},
	Parameters: tpm2.NewTPMUPublicParms(tpm2.TPMAlgECC, &tpm2.TPMSECCParms{
		Symmetric: tpm2.TPMTSymDefObject{
			Algorithm: tpm2.TPMAlgAES,
			KeyBits:   tpm2.NewTPMUSymKeyBits(tpm2.TPMAlgAES, tpm2.TPMKeyBits(128)),
			Mode:      tpm2.NewTPMUSymMode(tpm2.TPMAlgAES, tpm2.TPMAlgCFB),
		},
		Scheme:  tpm2.TPMTECCScheme{Scheme: tpm2.TPMAlgNull},
		CurveID: tpm2.TPMECCNistP256,
		KDF:     tpm2.TPMTKDFScheme{Scheme: tpm2.TPMAlgNull},
	}),
	Unique: tpm2.NewTPMUPublicID(tpm2.TPMAlgECC, &tpm2.TPMSECCPoint{}),
}

// ready opens the TPM and loads the key, if not done already
func (b *tpmBackend) ready() error {
	if b.t != nil {
		return nil
	}
	pubRaw, err := os.ReadFile(filepath.Join(b.keys, "sig.pub"))
	if err != nil {
		return err
	}
	privRaw, err := os.ReadFile(filepath.Join(b.keys, "sig.priv"))
	if err != nil {
		return err
	}
	pub, err := tpm2.Unmarshal[tpm2.TPM2BPublic](pubRaw)
	if err != nil {
		return fmt.Errorf("sig.pub: %w", err)
	}
	priv, err := tpm2.Unmarshal[tpm2.TPM2BPrivate](privRaw)
	if err != nil {
		return fmt.Errorf("sig.priv: %w", err)
	}
	t, err := b.open()
	if err != nil {
		return err
	}
	srk, err := tpm2.CreatePrimary{
		PrimaryHandle: tpm2.TPMRHOwner,
		InPublic:      tpm2.New2B(srkTemplate),
	}.Execute(t)
	if err != nil {
		_ = t.Close()
		return fmt.Errorf("creating the primary: %w", err)
	}
	// the parent is needed only to load the key
	defer func() { _, _ = tpm2.FlushContext{FlushHandle: srk.ObjectHandle}.Execute(t) }()
	l, err := tpm2.Load{
		ParentHandle: tpm2.AuthHandle{Handle: srk.ObjectHandle, Name: srk.Name, Auth: tpm2.PasswordAuth(nil)},
		InPrivate:    *priv,
		InPublic:     *pub,
	}.Execute(t)
	if err != nil {
		_ = t.Close()
		return fmt.Errorf("loading the key: %w", err)
	}
	b.t, b.key = t, tpm2.NamedHandle{Handle: l.ObjectHandle, Name: l.Name}
	return nil
}

func (b *tpmBackend) sign(digest []byte) ([]byte, error) {
	if err := b.ready(); err != nil {
		return nil, err
	}
	policy := policyPCR7
	if _, err := os.Stat(filepath.Join(b.keys, "approver.tpm")); err == nil {
		policy = b.policyApproved
	}
	rsp, err := tpm2.Sign{
		KeyHandle: tpm2.AuthHandle{Handle: b.key.Handle, Name: b.key.Name, Auth: tpm2.Policy(tpm2.TPMAlgSHA256, 16, policy)},
		Digest:    tpm2.TPM2BDigest{Buffer: digest},
		InScheme: tpm2.TPMTSigScheme{
			Scheme:  tpm2.TPMAlgECDSA,
			Details: tpm2.NewTPMUSigScheme(tpm2.TPMAlgECDSA, &tpm2.TPMSSchemeHash{HashAlg: tpm2.TPMAlgSHA256}),
		},
		// the key is not restricted: a null ticket
		Validation: tpm2.TPMTTKHashCheck{Tag: tpm2.TPMSTHashCheck, Hierarchy: tpm2.TPMRHNull},
	}.Execute(b.t)
	if err != nil {
		return nil, err
	}
	ecc, err := rsp.Signature.Signature.ECDSA()
	if err != nil {
		return nil, err
	}
	// as tpm2_sign -f plain wrote it for an ECDSA key, and as the
	// validators read it: DER
	return asn1.Marshal(struct{ R, S *big.Int }{
		new(big.Int).SetBytes(ecc.SignatureR.Buffer),
		new(big.Int).SetBytes(ecc.SignatureS.Buffer),
	})
}

// policyPCR7: a key dd-unlock-enrol made
func policyPCR7(t transport.TPM, h tpm2.TPMISHPolicy, _ tpm2.TPM2BNonce) error {
	_, err := tpm2.PolicyPCR{
		PolicySession: h,
		Pcrs: tpm2.TPMLPCRSelection{PCRSelections: []tpm2.TPMSPCRSelection{{
			Hash: tpm2.TPMAlgSHA256, PCRSelect: tpm2.PCClientCompatible.PCRs(7),
		}}},
	}.Execute(t)
	return err
}

// errNotApproved: no approval here for this boot at this epoch. No retry
// mends that; a boot the box did not approve has no use of its key
var errNotApproved = fmt.Errorf("this boot is not one the box approved: %w", fs.ErrNotExist)

// policyApproved: a key under the box's approver (dd-tpm-policy does the
// same for the shell's users): PCRs 4 and 7 now, the epoch now, and the
// approver's signature over that policy (tpm/bootkey.go writes it)
func (b *tpmBackend) policyApproved(t transport.TPM, h tpm2.TPMISHPolicy, _ tpm2.TPM2BNonce) error {
	if _, err := (tpm2.PolicyPCR{
		PolicySession: h,
		Pcrs: tpm2.TPMLPCRSelection{PCRSelections: []tpm2.TPMSPCRSelection{{
			Hash: tpm2.TPMAlgSHA256, PCRSelect: tpm2.PCClientCompatible.PCRs(4, 7),
		}}},
	}).Execute(t); err != nil {
		return err
	}
	nv, err := tpm2.NVReadPublic{NVIndex: tpm2.TPMHandle(attest.EpochIndex)}.Execute(t)
	if err != nil {
		return fmt.Errorf("the epoch counter: %w", err)
	}
	index := tpm2.NamedHandle{Handle: tpm2.TPMHandle(attest.EpochIndex), Name: nv.NVName}
	owner := tpm2.AuthHandle{Handle: tpm2.TPMRHOwner, Auth: tpm2.PasswordAuth(nil)}
	epoch, err := tpm2.NVRead{AuthHandle: owner, NVIndex: index, Size: 8}.Execute(t)
	if err != nil {
		return fmt.Errorf("the epoch: %w", err)
	}
	if _, err := (tpm2.PolicyNV{
		AuthHandle: owner, NVIndex: index, PolicySession: h,
		OperandB: tpm2.TPM2BOperand{Buffer: epoch.Data.Buffer}, Operation: tpm2.TPMEOEq,
	}).Execute(t); err != nil {
		return err
	}
	cur, err := tpm2.PolicyGetDigest{PolicySession: h}.Execute(t)
	if err != nil {
		return err
	}
	raw, err := os.ReadFile(filepath.Join(b.keys, "approvals", hex.EncodeToString(cur.PolicyDigest.Buffer)+".sig"))
	if errors.Is(err, fs.ErrNotExist) {
		return errNotApproved
	}
	if err != nil {
		return err
	}
	var sig struct{ R, S *big.Int }
	if rest, err := asn1.Unmarshal(raw, &sig); err != nil || len(rest) != 0 {
		return fmt.Errorf("an approval that is no signature: %v", err)
	}
	// the boot partition is anyone's who has the disk: never trust its sizes
	if sig.R.Sign() <= 0 || sig.S.Sign() <= 0 || sig.R.BitLen() > 256 || sig.S.BitLen() > 256 {
		return errors.New("an approval that is no P-256 signature")
	}
	pubRaw, err := os.ReadFile(filepath.Join(b.keys, "approver.tpm"))
	if err != nil {
		return err
	}
	pub, err := tpm2.Unmarshal[tpm2.TPM2BPublic](pubRaw)
	if err != nil {
		return fmt.Errorf("approver.tpm: %w", err)
	}
	// under the owner's hierarchy: the null one's ticket is good for nothing
	loaded, err := tpm2.LoadExternal{InPublic: *pub, Hierarchy: tpm2.TPMRHOwner}.Execute(t)
	if err != nil {
		return fmt.Errorf("loading the approver: %w", err)
	}
	defer func() { _, _ = tpm2.FlushContext{FlushHandle: loaded.ObjectHandle}.Execute(t) }()
	ticket, err := tpm2.VerifySignature{
		KeyHandle: tpm2.NamedHandle{Handle: loaded.ObjectHandle, Name: loaded.Name},
		Digest:    tpm2.TPM2BDigest{Buffer: attest.ApprovalDigest(cur.PolicyDigest.Buffer)},
		Signature: tpm2.TPMTSignature{
			SigAlg: tpm2.TPMAlgECDSA,
			Signature: tpm2.NewTPMUSignature(tpm2.TPMAlgECDSA, &tpm2.TPMSSignatureECC{
				Hash:       tpm2.TPMAlgSHA256,
				SignatureR: tpm2.TPM2BECCParameter{Buffer: sig.R.FillBytes(make([]byte, 32))},
				SignatureS: tpm2.TPM2BECCParameter{Buffer: sig.S.FillBytes(make([]byte, 32))},
			}),
		},
	}.Execute(t)
	if err != nil {
		return fmt.Errorf("the approval: %w", err)
	}
	_, err = tpm2.PolicyAuthorize{
		PolicySession:  h,
		ApprovedPolicy: tpm2.TPM2BDigest{Buffer: cur.PolicyDigest.Buffer},
		PolicyRef:      tpm2.TPM2BDigest{Buffer: attest.ApprovalRef},
		KeySign:        loaded.Name,
		CheckTicket:    ticket.Validation,
	}.Execute(t)
	return err
}

func (b *tpmBackend) reset() {
	if b.t != nil {
		_ = b.t.Close()
	}
	b.t = nil
}

// flushStale flushes the sessions the TPM holds, loaded or saved, on a
// connection of its own that holds none
func (b *tpmBackend) flushStale() (int, error) {
	b.reset()
	t, err := b.open()
	if err != nil {
		return 0, err
	}
	defer t.Close()
	n := 0
	var errs []error
	for _, first := range []tpm2.TPMHandle{tpm2.TPMHandle(tpm2.TPMHTHMACSession) << 24, tpm2.TPMHandle(tpm2.TPMHTPolicySession) << 24} {
		rsp, err := tpm2.GetCapability{Capability: tpm2.TPMCapHandles, Property: uint32(first), PropertyCount: 64}.Execute(t)
		if err != nil {
			errs = append(errs, err)
			continue
		}
		hs, err := rsp.CapabilityData.Data.Handles()
		if err != nil {
			errs = append(errs, err)
			continue
		}
		for _, h := range hs.Handle {
			// a loaded policy session is listed among the loaded ones
			if ht := tpm2.TPMHT(h >> 24); ht != tpm2.TPMHTHMACSession && ht != tpm2.TPMHTPolicySession {
				continue
			}
			if _, err := (tpm2.FlushContext{FlushHandle: h}).Execute(t); err != nil {
				errs = append(errs, fmt.Errorf("%#x: %w", uint32(h), err))
				continue
			}
			n++
		}
	}
	return n, errors.Join(errs...)
}

// release flushes the key and closes the connection: what a TPM spoken to
// without the kernel's resource manager (a test's swtpm) would keep
func (b *tpmBackend) release() {
	if b.t != nil {
		_, _ = tpm2.FlushContext{FlushHandle: b.key.Handle}.Execute(b.t)
	}
	b.reset()
}

// checkMessage is what -check-sign signs: no other message starts so
func checkMessage(box string) []byte { return []byte("commonty sign check v1\x00" + box) }

// enrolledKey: the key the fleet knows for this box (SPKI, base64), from
// the release's config, else the key it joins with
func enrolledKey(configPath, box, joinKey string) (string, error) {
	raw, err := os.ReadFile(configPath)
	if err != nil {
		return "", err
	}
	var cfg struct {
		Boxes map[string]struct {
			Key string `json:"key"`
		} `json:"boxes"`
	}
	if err := json.Unmarshal(raw, &cfg); err != nil {
		return "", fmt.Errorf("%s: %w", configPath, err)
	}
	if b, ok := cfg.Boxes[box]; ok && b.Key != "" {
		return b.Key, nil
	}
	if joinKey == "" {
		return "", fmt.Errorf("%s is not in %s, and no -join-key names its key", box, configPath)
	}
	k, err := os.ReadFile(joinKey)
	if err != nil {
		return "", err
	}
	return strings.TrimSpace(string(k)), nil
}

// checkSign signs checkMessage once on dev, under the box-wide lock held
// for the whole check, and checks it under spki: "<alg> sha256:<hex of
// the SPKI>" when it checks. Nothing is left loaded in the TPM
func checkSign(dev tpmDevice, lock, box, spki string) (string, error) {
	der, err := base64.StdEncoding.DecodeString(spki)
	if err != nil {
		return "", fmt.Errorf("the enrolled key: %w", err)
	}
	k, err := x509.ParsePKIXPublicKey(der)
	if err != nil {
		return "", fmt.Errorf("the enrolled key: %w", err)
	}
	pub, ok := k.(*ecdsa.PublicKey)
	if !ok || pub.Curve != elliptic.P256() {
		return "", errors.New("the enrolled key is not P-256")
	}
	fp := sha256.Sum256(der)
	if lock != "" {
		f, err := openLock(lock)
		if err != nil {
			return "", err
		}
		defer f.Close()
		deadline := time.Now().Add(30 * time.Second)
		for {
			err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
			if err == nil {
				break
			}
			if !errors.Is(err, syscall.EWOULDBLOCK) {
				return "", fmt.Errorf("%s: %w", lock, err)
			}
			if time.Now().After(deadline) {
				return "", fmt.Errorf("%s: held by another for 30 s", lock)
			}
			time.Sleep(50 * time.Millisecond)
		}
		defer func() { _ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN) }()
	}
	if r, ok := dev.(interface{ release() }); ok {
		defer r.release()
	} else {
		defer dev.reset()
	}
	msg := checkMessage(box)
	digest := sha256.Sum256(msg)
	sig, err := dev.sign(digest[:])
	if err != nil {
		return "", errSigning{err}
	}
	if !state.P256(spki, msg, base64.RawURLEncoding.EncodeToString(sig)) {
		why := "the signature does not check under the enrolled key sha256:" + hex.EncodeToString(fp[:])
		if b, ok := dev.(*tpmBackend); ok {
			if mine, err := b.publicKey(); err == nil && !mine.Equal(pub) {
				why += "; the key in " + filepath.Join(b.keys, "sig.pub") + " is not the enrolled one"
			}
		}
		return "", errors.New(why)
	}
	return "ecdsa-p256-sha256 sha256:" + hex.EncodeToString(fp[:]), nil
}

// publicKey: the key sig.pub holds
func (b *tpmBackend) publicKey() (*ecdsa.PublicKey, error) {
	raw, err := os.ReadFile(filepath.Join(b.keys, "sig.pub"))
	if err != nil {
		return nil, err
	}
	p, err := tpm2.Unmarshal[tpm2.TPM2BPublic](raw)
	if err != nil {
		return nil, err
	}
	pt, err := p.Contents()
	if err != nil {
		return nil, err
	}
	ecc, err := pt.Unique.ECC()
	if err != nil {
		return nil, err
	}
	return &ecdsa.PublicKey{Curve: elliptic.P256(),
		X: new(big.Int).SetBytes(ecc.X.Buffer), Y: new(big.Int).SetBytes(ecc.Y.Buffer)}, nil
}
