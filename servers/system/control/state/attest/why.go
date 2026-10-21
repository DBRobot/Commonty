package attest

import (
	"encoding/base64"
	"errors"
	"strings"
)

// JudgeBoot judges what Begin's event log says booted - Secure Boot on
// under the box's own certificate (PCR 7), then the release's systemd-boot
// and lanzaboote image (PCR 4) - before any challenge, so a box that
// cannot pass is refused at once and with why. The quote later holds the
// TPM to this same log (VerifyResponse).
func JudgeBoot(cfg *Config, b Begin) error {
	pol, err := cfg.policy()
	if err != nil {
		return err
	}
	if pol.test.NoSecureBoot {
		return nil
	}
	raw, err := base64.StdEncoding.DecodeString(b.Log)
	if err != nil {
		return err
	}
	log, err := parseLog(raw)
	if err != nil {
		return err
	}
	var db, stub []byte
	if b.DB != "" {
		if db, err = base64.StdEncoding.DecodeString(b.DB); err != nil {
			return err
		}
	}
	if b.Stub != "" {
		if stub, err = base64.StdEncoding.DecodeString(b.Stub); err != nil {
			return err
		}
	}
	if _, err := secureBoot(log, db); err != nil {
		return err
	}
	return checkBoot(pol.boot, log, stub)
}

// Why: a refusal in the words the person who set the box up reads, on
// their Devices page and on the box. The judge's own words stay in the
// logs.
func Why(err error) string {
	msg := err.Error()
	switch {
	case errors.Is(err, ErrSoftware), errors.Is(err, ErrNoRoot), errors.Is(err, ErrEK):
		return "its TPM is not one a TPM maker certified"
	case strings.Contains(msg, "this TPM is already"):
		return "its TPM is already another box's"
	case strings.Contains(msg, "Secure Boot certificate is already"):
		return "its Secure Boot keys are already another box's"
	case errors.Is(err, ErrSecureBoot) && (strings.Contains(msg, "Secure Boot is off") || strings.Contains(msg, "setup mode")):
		return "Secure Boot is off"
	case errors.Is(err, ErrSecureBoot):
		return "Secure Boot is not on with this box's own keys"
	case errors.Is(err, ErrBoot):
		return "it did not start the fleet's own system"
	case strings.Contains(msg, "approver"), strings.Contains(msg, "not approved"), strings.Contains(msg, "the join key"):
		return "its key is not held by its TPM for this boot"
	}
	return "it could not prove its TPM"
}
