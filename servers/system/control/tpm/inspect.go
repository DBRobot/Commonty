package tpm

import (
	"os"
	"strings"
	"time"

	"commonty/control/state/attest"
)

// Inspect reads this machine's EK, its certificate chain and the
// firmware's event log, as a join would send them, and judges them by
// cfg's roots: changing nothing in the TPM (the EK is made again from the
// TPM's seed, in its working memory, and flushed) and writing only into
// Dir. PCRsMatch: the log replayed gives the TPM's PCRs 0, 2, 4 and 7.
func (t *Box) Inspect(cfg *attest.Config) (*attest.Inspection, bool, error) {
	if err := os.MkdirAll(t.Dir, 0o700); err != nil {
		return nil, false, err
	}
	defer func() { _, _ = t.run("tpm2_flushcontext", "-t") }()
	if _, err := t.run("tpm2_createek", "-Q", "-c", t.p("ek.ctx"), "-G", "rsa", "-u", t.p("ek.pub")); err != nil {
		return nil, false, err
	}
	ek, err := os.ReadFile(t.p("ek.pub"))
	if err != nil {
		return nil, false, err
	}
	cert, err := t.ekCert()
	if err != nil {
		return nil, false, err
	}
	chain := [][]byte{cert}
	if raw, err := t.nv(nvChain); err == nil {
		chain = append(chain, splitDER(raw)...)
	}
	chain = t.issuers(chain)
	logRaw, err := os.ReadFile(t.Log)
	if err != nil {
		return nil, false, err
	}
	in, err := attest.Inspect(cfg, ek, chain, logRaw, time.Now())
	if err != nil {
		return nil, false, err
	}
	out, err := t.run("tpm2_pcrread", "sha256:0,2,4,7")
	if err != nil {
		return in, false, err
	}
	match := true
	for _, i := range []string{"0", "2", "4", "7"} {
		match = match && pcrIn(out, i, in.PCRs[i])
	}
	return in, match, nil
}

// pcrIn: tpm2_pcrread's line for PCR i says v
func pcrIn(out []byte, i, v string) bool {
	for _, l := range strings.Split(string(out), "\n") {
		k, val, ok := strings.Cut(strings.TrimSpace(l), ":")
		if ok && strings.TrimSpace(k) == i {
			return strings.TrimPrefix(strings.ToLower(strings.TrimSpace(val)), "0x") == strings.ToLower(v)
		}
	}
	return false
}
