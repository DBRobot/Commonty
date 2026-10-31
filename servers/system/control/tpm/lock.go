package tpm

import (
	"errors"
	"fmt"
	"log"
	"os"
	"path/filepath"
	"syscall"
	"time"
)

// LockPath is the file every user of the TPM on a box takes (flock) around
// its use: dd-control's signer, dd-boot-key, a joined box's proof, and the
// scripts that sign with tpm2-tools. One holding a session saved between
// tpm2-tools commands, killed there, leaves it in the TPM for good; held
// here, no signer flushes it mid-use (cmd/dd-control/tpmsign.go).
const LockPath = "/run/dd-tpm/lock"

// OpenLock opens the lock file, made 0600 in a 0700 directory if missing,
// and refuses one that anybody but this process's user owns or may open:
// whoever can open it can hold it
func OpenLock(path string) (*os.File, error) {
	_ = os.Mkdir(filepath.Dir(path), 0o700)
	f, err := os.OpenFile(path, os.O_RDONLY|os.O_CREATE, 0o600)
	if err != nil {
		return nil, err
	}
	for _, p := range []string{filepath.Dir(path), path} {
		fi, err := os.Stat(p)
		if err != nil {
			_ = f.Close()
			return nil, err
		}
		st, ok := fi.Sys().(*syscall.Stat_t)
		if !ok || int(st.Uid) != os.Getuid() || fi.Mode().Perm()&0o077 != 0 {
			_ = f.Close()
			return nil, fmt.Errorf("%s: not %d's alone (%v, uid %d)", p, os.Getuid(), fi.Mode().Perm(), st.Uid)
		}
	}
	return f, nil
}

// HoldLock waits for the lock, up to wait, and returns its release. Past
// that, or with no lock to be had, it says so and goes on without: what
// holds the TPM's users up longer is a fault of its own, and a box must
// not stop approving its next boot for it (as the scripts' flock -w || true).
func HoldLock(path string, wait time.Duration) func() {
	if path == "" {
		return func() {}
	}
	f, err := OpenLock(path)
	if err != nil {
		log.Printf("tpm: %v; going on without the lock", err)
		return func() {}
	}
	deadline := time.Now().Add(wait)
	for {
		err := syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
		if err == nil {
			return func() { _ = syscall.Flock(int(f.Fd()), syscall.LOCK_UN); _ = f.Close() }
		}
		if !errors.Is(err, syscall.EWOULDBLOCK) || time.Now().After(deadline) {
			log.Printf("tpm: %s held for %s; going on without it", path, wait)
			_ = f.Close()
			return func() {}
		}
		time.Sleep(50 * time.Millisecond)
	}
}
