package main

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"errors"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"sync"
	"sync/atomic"
	"syscall"
	"testing"
	"time"

	"github.com/google/go-tpm/tpm2"
	"github.com/google/go-tpm/tpm2/transport"
	"github.com/google/go-tpm/tpm2/transport/tcp"
)

// fakeTPM: a TPM that fails as it is told, and notices two signatures at once
type fakeTPM struct {
	mu sync.Mutex
	// errors the next signatures fail with, in order
	fails []error
	// flushed clears the gap: until then every signature fails with it
	gapped                 bool
	busy                   atomic.Int32
	overlap                atomic.Bool
	signs, resets, flushes int
}

func (f *fakeTPM) sign(digest []byte) ([]byte, error) {
	if f.busy.Add(1) > 1 {
		f.overlap.Store(true)
	}
	defer f.busy.Add(-1)
	time.Sleep(time.Millisecond)
	f.mu.Lock()
	defer f.mu.Unlock()
	f.signs++
	if f.gapped {
		return nil, fmt.Errorf("read /dev/tpmrm0: %w", syscall.EFAULT)
	}
	if len(f.fails) > 0 {
		err := f.fails[0]
		f.fails = f.fails[1:]
		return nil, err
	}
	return append([]byte("sig:"), digest[:4]...), nil
}

func (f *fakeTPM) reset() { f.mu.Lock(); f.resets++; f.mu.Unlock() }

func (f *fakeTPM) flushStale() (int, error) {
	f.mu.Lock()
	defer f.mu.Unlock()
	f.flushes++
	if f.gapped {
		f.gapped = false
		return 1, nil
	}
	return 0, nil
}

// a signer on a fake clock: sleeping moves it on
func testSigner(dev tpmDevice) (*signer, *[]string) {
	logs := &[]string{}
	var mu sync.Mutex
	now := time.Unix(1792900000, 0)
	s := newSigner(dev)
	s.lock = ""
	s.now = func() time.Time { mu.Lock(); defer mu.Unlock(); return now }
	s.sleep = func(d time.Duration) { mu.Lock(); now = now.Add(d); mu.Unlock() }
	s.logf = func(f string, a ...any) { mu.Lock(); *logs = append(*logs, fmt.Sprintf(f, a...)); mu.Unlock() }
	return s, logs
}

func TestSignerSerializesSignatures(t *testing.T) {
	f := &fakeTPM{}
	s, _ := testSigner(f)
	var wg sync.WaitGroup
	for i := range 32 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			if _, err := s.Sign([]byte(fmt.Sprint("request ", i))); err != nil {
				t.Error(err)
			}
		}()
	}
	wg.Wait()
	if f.overlap.Load() {
		t.Fatal("two signatures reached the TPM at once")
	}
	if f.signs != 32 {
		t.Fatalf("%d signatures, want 32", f.signs)
	}
}

func TestSignerRetriesTransientErrors(t *testing.T) {
	f := &fakeTPM{fails: []error{
		tpm2.TPMRCRetry,
		errors.New("tcti: IO failure"),
		tpm2.TPMRCYielded,
	}}
	s, logs := testSigner(f)
	sig, err := s.Sign([]byte("renew svc/postgresql"))
	if err != nil {
		t.Fatalf("a signature that the fourth try makes failed: %v", err)
	}
	d := sha256.Sum256([]byte("renew svc/postgresql"))
	if want := base64.RawURLEncoding.EncodeToString(append([]byte("sig:"), d[:4]...)); sig != want {
		t.Fatalf("signature %q, want %q", sig, want)
	}
	if f.resets != 3 {
		t.Fatalf("%d resets, want one for each failure (3): the key is loaded afresh after an error", f.resets)
	}
	// one line when it fails, one when it is back: not one per try
	if len(*logs) != 2 || !strings.Contains((*logs)[0], "saying nothing more") || !strings.Contains((*logs)[1], "after 3 failed tries") {
		t.Fatalf("logs: %q", *logs)
	}
}

func TestSignerGivesUpWithinItsBudget(t *testing.T) {
	f := &fakeTPM{}
	for range 100 {
		f.fails = append(f.fails, errors.New("tcti: IO failure"))
	}
	s, logs := testSigner(f)
	start := s.now()
	_, err := s.Sign([]byte("heartbeat"))
	if !errors.As(err, new(errSigning)) {
		t.Fatalf("err %v, want an errSigning", err)
	}
	if took := s.now().Sub(start); took > s.budget {
		t.Fatalf("tried for %s, over its budget %s", took, s.budget)
	}
	tries := f.signs
	// the burst is older than one signature's tries now: one try a call
	s.sleep(s.budget)
	if _, err := s.Sign([]byte("heartbeat")); err == nil {
		t.Fatal("signed with the TPM still failing")
	}
	if f.signs != tries+1 {
		t.Fatalf("a call in a long burst tried %d times, want 1", f.signs-tries)
	}
	if len(*logs) != 1 {
		t.Fatalf("a burst logged %d lines, want 1: %q", len(*logs), *logs)
	}
}

func TestSignerDoesNotRetryWhatCannotPass(t *testing.T) {
	f := &fakeTPM{fails: []error{tpm2.TPMRCPolicyFail, nil}}
	s, _ := testSigner(f)
	if _, err := s.Sign([]byte("x")); err == nil {
		t.Fatal("signed past a policy failure")
	}
	if f.signs != 1 {
		t.Fatalf("%d tries of a policy failure, want 1", f.signs)
	}
}

// 2026-10-24 23:52: a session left saved in the TPM by an earlier
// dd-control, and every signature failed once the TPM's context counter
// had moved 65535 past it. Flushing it clears it
func TestSignerRecoversFromAContextGap(t *testing.T) {
	f := &fakeTPM{gapped: true}
	s, logs := testSigner(f)
	if _, err := s.Sign([]byte("renew svc/postgresql")); err != nil {
		t.Fatalf("not signed after a context gap: %v", err)
	}
	if f.flushes != 1 || f.signs != 2 {
		t.Fatalf("flushes %d, signs %d: want the stale sessions flushed once and the signature tried again", f.flushes, f.signs)
	}
	if !strings.Contains((*logs)[0], "flushed 1 stale sessions") {
		t.Fatalf("logs: %q", *logs)
	}
	// as the TPM says it when spoken to directly
	f.fails = []error{tpm2.TPMRCContextGap}
	if _, err := s.Sign([]byte("again")); err != nil || f.flushes != 2 {
		t.Fatalf("TPM_RC_CONTEXT_GAP: err %v, flushes %d", err, f.flushes)
	}
}

func TestTheLockIsRootsAlone(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "dd-tpm")
	f, err := openLock(filepath.Join(dir, "lock"))
	if err != nil {
		t.Fatal(err)
	}
	f.Close()
	for p, want := range map[string]os.FileMode{dir: 0o700, filepath.Join(dir, "lock"): 0o600} {
		if fi, _ := os.Stat(p); fi.Mode().Perm() != want {
			t.Fatalf("%s: %v, want %v", p, fi.Mode().Perm(), want)
		}
	}
	// one others may open is not used: they could hold it
	_ = os.Chmod(filepath.Join(dir, "lock"), 0o644)
	if _, err := openLock(filepath.Join(dir, "lock")); err == nil {
		t.Fatal("used a lock others may open")
	}
	_ = os.Chmod(filepath.Join(dir, "lock"), 0o600)
	_ = os.Chmod(dir, 0o755)
	if _, err := openLock(filepath.Join(dir, "lock")); err == nil {
		t.Fatal("used a lock in a directory others may enter")
	}
}

func TestSignerHoldsTheBoxWideLock(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "dd-tpm")
	lock := filepath.Join(dir, "lock")
	if f, err := openLock(lock); err != nil {
		t.Fatal(err)
	} else {
		f.Close()
	}
	held := make(chan struct{})
	release := make(chan struct{})
	go func() {
		fd, _ := os.Open(lock)
		_ = syscall.Flock(int(fd.Fd()), syscall.LOCK_EX)
		close(held)
		<-release
		_ = syscall.Flock(int(fd.Fd()), syscall.LOCK_UN)
		fd.Close()
	}()
	<-held
	f := &fakeTPM{}
	s := newSigner(f)
	s.lock = lock
	s.logf = func(string, ...any) {}
	done := make(chan struct{})
	go func() { _, _ = s.Sign([]byte("x")); close(done) }()
	select {
	case <-done:
		t.Fatal("signed while another held the TPM's lock")
	case <-time.After(200 * time.Millisecond):
	}
	close(release)
	select {
	case <-done:
	case <-time.After(3 * time.Second):
		t.Fatal("not signed once the lock was let go")
	}
}

// The submit path: numbered, signed and sent in one turn, so what leaves
// leaves in order, however slow a signature is
func TestRequestsLeaveInOrder(t *testing.T) {
	f := &fakeTPM{}
	s, _ := testSigner(f)
	var mu sync.Mutex
	var seq uint64
	var sent []uint64
	submit := func() {
		mu.Lock()
		defer mu.Unlock()
		q := seq + 1
		if _, err := s.Sign([]byte(fmt.Sprint(q))); err != nil {
			t.Error(err)
			return
		}
		seq = q
		sent = append(sent, q)
	}
	var wg sync.WaitGroup
	for range 50 {
		wg.Add(1)
		go func() { defer wg.Done(); submit() }()
	}
	wg.Wait()
	for i := 1; i < len(sent); i++ {
		if sent[i] <= sent[i-1] {
			t.Fatalf("sent %d after %d: the validators refuse the second", sent[i], sent[i-1])
		}
	}
}

// Against a software TPM (swtpm) with a key made as dd-unlock-enrol makes
// it (tpm2-tools): the same primary, a signature its public key checks,
// and a session left saved by tpm2-tools flushed. DD_TEST_SWTPM is the
// swtpm's server and control ports (host:port,host:port), DD_TEST_TPM_KEYS
// the directory with sig.pub, sig.priv and spki.der
func TestSignWithSoftwareTPM(t *testing.T) {
	addrs, keys := os.Getenv("DD_TEST_SWTPM"), os.Getenv("DD_TEST_TPM_KEYS")
	if addrs == "" || keys == "" {
		t.Skip("DD_TEST_SWTPM and DD_TEST_TPM_KEYS not set")
	}
	cmd, plat, _ := strings.Cut(addrs, ",")
	b := newTPMBackend(keys)
	b.open = func() (transport.TPMCloser, error) {
		return tcp.Open(tcp.Config{CommandAddress: cmd, PlatformAddress: plat})
	}
	s := newSigner(b)
	s.lock = ""
	der, err := os.ReadFile(filepath.Join(keys, "spki.der"))
	if err != nil {
		t.Fatal(err)
	}
	pk, err := x509.ParsePKIXPublicKey(der)
	if err != nil {
		t.Fatal(err)
	}
	for i := range 3 {
		msg := []byte(fmt.Sprint("commonty heartbeat ", i))
		sig, err := s.Sign(msg)
		if err != nil {
			t.Fatal(err)
		}
		raw, _ := base64.RawURLEncoding.DecodeString(sig)
		d := sha256.Sum256(msg)
		if !ecdsa.VerifyASN1(pk.(*ecdsa.PublicKey), d[:], raw) {
			t.Fatalf("signature %d does not check under the enrolled key", i)
		}
	}
	// -check-sign: checks under the enrolled key and leaves nothing loaded
	b.release()
	if _, err := checkSign(b, "", "node1", base64.StdEncoding.EncodeToString(der)); err != nil {
		t.Fatal(err)
	}
	t2, err := b.open()
	if err != nil {
		t.Fatal(err)
	}
	for _, first := range []tpm2.TPMHT{tpm2.TPMHTTransient, tpm2.TPMHTHMACSession, tpm2.TPMHTPolicySession} {
		rsp, err := tpm2.GetCapability{Capability: tpm2.TPMCapHandles, Property: uint32(first) << 24, PropertyCount: 64}.Execute(t2)
		if err != nil {
			t.Fatal(err)
		}
		hs, _ := rsp.CapabilityData.Data.Handles()
		for _, h := range hs.Handle {
			if tpm2.TPMHT(h>>24) == first {
				t.Fatalf("-check-sign left %#x in the TPM", uint32(h))
			}
		}
	}
	_ = t2.Close()
	// a session tpm2-tools saved and nobody flushed, as a killed dd-control left one
	if tools, err := exec.LookPath("tpm2_startauthsession"); err == nil {
		b.reset() // swtpm serves one connection at a time
		ctx := filepath.Join(t.TempDir(), "s.ctx")
		c := exec.Command(tools, "-Q", "--policy-session", "-S", ctx)
		c.Env = append(os.Environ(), "TPM2TOOLS_TCTI=swtpm:host="+strings.Split(cmd, ":")[0]+",port="+strings.Split(cmd, ":")[1])
		if out, err := c.CombinedOutput(); err != nil {
			t.Fatalf("tpm2_startauthsession: %v: %s", err, out)
		}
		n, err := b.flushStale()
		if err != nil || n < 1 {
			t.Fatalf("flushStale: %d, %v; want the saved session flushed", n, err)
		}
		if _, err := s.Sign([]byte("after the flush")); err != nil {
			t.Fatal(err)
		}
	}
}

func softKeyFile(t *testing.T) (string, string) {
	k, err := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	der, _ := x509.MarshalPKCS8PrivateKey(k)
	path := filepath.Join(t.TempDir(), "box.key")
	if err := os.WriteFile(path, der, 0o600); err != nil {
		t.Fatal(err)
	}
	spki, _ := x509.MarshalPKIXPublicKey(&k.PublicKey)
	return path, base64.StdEncoding.EncodeToString(spki)
}

func TestCheckSign(t *testing.T) {
	key, spki := softKeyFile(t)
	cfg := filepath.Join(t.TempDir(), "config.json")
	_ = os.WriteFile(cfg, []byte(`{"boxes":{"node1":{"key":"`+spki+`"}}}`), 0o644)
	got, err := enrolledKey(cfg, "node1", "")
	if err != nil || got != spki {
		t.Fatalf("enrolledKey: %q, %v", got, err)
	}
	if _, err := enrolledKey(cfg, "node9", ""); err == nil {
		t.Fatal("a box the config does not list, with no -join-key, has a key")
	}
	lock := filepath.Join(t.TempDir(), "dd-tpm", "lock")
	out, err := checkSign(softDevice(key), lock, "node1", spki)
	der, _ := base64.StdEncoding.DecodeString(spki)
	fp := sha256.Sum256(der)
	if err != nil || out != fmt.Sprintf("ecdsa-p256-sha256 sha256:%x", fp) {
		t.Fatalf("checkSign: %q, %v", out, err)
	}
	// made as the signer makes it
	for p, want := range map[string]os.FileMode{filepath.Dir(lock): 0o700, lock: 0o600} {
		if fi, _ := os.Stat(p); fi.Mode().Perm() != want {
			t.Fatalf("%s: %v, want %v", p, fi.Mode().Perm(), want)
		}
	}
	// another box's key does not pass
	_, other := softKeyFile(t)
	if _, err := checkSign(softDevice(key), lock, "node1", other); err == nil || !strings.Contains(err.Error(), "does not check") {
		t.Fatalf("checked under a key not the box's: %v", err)
	}
	// a TPM that fails says so
	f := &fakeTPM{fails: []error{tpm2.TPMRCPolicyFail}}
	if _, err := checkSign(f, lock, "node1", spki); !errors.Is(err, tpm2.TPMRCPolicyFail) {
		t.Fatalf("a failed signature: %v", err)
	}
	if f.resets != 1 {
		t.Fatalf("the connection was not let go: %d resets", f.resets)
	}
}

func TestCheckSignWaitsForTheLock(t *testing.T) {
	key, spki := softKeyFile(t)
	lock := filepath.Join(t.TempDir(), "dd-tpm", "lock")
	held, err := openLock(lock)
	if err != nil {
		t.Fatal(err)
	}
	_ = syscall.Flock(int(held.Fd()), syscall.LOCK_EX)
	done := make(chan error, 1)
	go func() { _, err := checkSign(softDevice(key), lock, "node1", spki); done <- err }()
	select {
	case <-done:
		t.Fatal("signed while another held the TPM's lock")
	case <-time.After(200 * time.Millisecond):
	}
	_ = syscall.Flock(int(held.Fd()), syscall.LOCK_UN)
	held.Close()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("not signed once the lock was let go")
	}
	// one others may open is refused, not signed without
	_ = os.Chmod(lock, 0o644)
	if _, err := checkSign(softDevice(key), lock, "node1", spki); err == nil {
		t.Fatal("used a lock others may open")
	}
}
