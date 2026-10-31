package attest

import (
	"crypto/sha256"
	"crypto/x509"
	"encoding/hex"
	"errors"
	"fmt"
	"time"

	"github.com/google/go-tpm/tpm2"
)

// Inspection is what a machine's TPM and firmware say, as the judge would
// read them, for a look at real hardware before it joins (attest-check
// inspect): nothing here is sent anywhere or changes the TPM.
type Inspection struct {
	Vendor   string   `json:"vendor"`
	Model    string   `json:"model,omitempty"`
	EK       string   `json:"ek"`             // sha256 of the EK's SPKI, hex
	Chain    []string `json:"chain"`          // each certificate's subject, EK first
	Root     string   `json:"root,omitempty"` // the release's root it chains to
	ChainErr string   `json:"chainErr,omitempty"`
	// the event log replayed, PCRs 0, 2, 4 and 7
	PCRs map[string]string `json:"pcrs"`
	// PCR 4's boot applications, Authenticode digests in the order run; a
	// log the judge would refuse for one of PCR 4's events says why, as a
	// box's own firmware is what this look is for
	Apps    []string `json:"apps"`
	AppsErr string   `json:"appsErr,omitempty"`
	// each event kind (hex) PCRs 4 and 7 hold, in order, the judge accepting
	// it or not: what a firmware the judge has not seen does
	Events4       []EventKind `json:"events4"`
	Events7       []EventKind `json:"events7"`
	SecureBoot    string      `json:"secureBoot"` // "on", or why not (Why)
	SecureBootErr string      `json:"secureBootErr,omitempty"`
}

// Inspect reads ek (TPM2B_PUBLIC), its certificate chain (DER, EK first)
// and the firmware's event log, at time at, against cfg's roots
func Inspect(cfg *Config, ek []byte, chain [][]byte, logRaw []byte, at time.Time) (*Inspection, error) {
	pol, err := cfg.policy()
	if err != nil {
		return nil, err
	}
	in := &Inspection{PCRs: map[string]string{}}
	p2b, err := tpm2.Unmarshal[tpm2.TPM2BPublic](ek)
	if err != nil {
		return nil, fmt.Errorf("the EK: %v", err)
	}
	pub, err := p2b.Contents()
	if err != nil {
		return nil, fmt.Errorf("the EK: %v", err)
	}
	ekPub, err := tpm2.Pub(*pub)
	if err != nil {
		return nil, fmt.Errorf("the EK: %v", err)
	}
	if err := ekShape(pub); err != nil {
		return nil, err
	}
	spki, err := x509.MarshalPKIXPublicKey(ekPub)
	if err != nil {
		return nil, err
	}
	h := sha256.Sum256(spki)
	in.EK = hex.EncodeToString(h[:])
	if len(chain) == 0 {
		return nil, errors.New("no EK certificate")
	}
	var certs []*x509.Certificate
	for i, der := range chain {
		c, err := x509.ParseCertificate(der)
		if err != nil {
			return nil, fmt.Errorf("certificate %d: %v", i, err)
		}
		certs = append(certs, c)
		in.Chain = append(in.Chain, c.Subject.String()+" (issued by "+c.Issuer.String()+")")
	}
	key, err := ekCertKey(certs[0])
	if err != nil {
		return nil, err
	}
	if !samePub(key, ekPub) {
		return nil, errors.New("the EK certificate is for another key")
	}
	in.Vendor, in.Model, err = verifyChain(pol, certs, at)
	if err != nil {
		in.ChainErr = err.Error()
	} else {
		in.Root = vendorName(in.Vendor)
	}
	log, err := parseLog(logRaw)
	if err != nil {
		return nil, fmt.Errorf("the event log: %v", err)
	}
	pcrs, err := replay(log)
	if err != nil {
		return nil, err
	}
	for _, i := range PCRs {
		v := pcrs[i]
		in.PCRs[fmt.Sprint(i)] = hex.EncodeToString(v[:])
	}
	if apps, err := bootApps(log); err != nil {
		in.AppsErr = err.Error()
	} else {
		for _, a := range apps {
			in.Apps = append(in.Apps, hex.EncodeToString(a))
		}
	}
	for _, e := range log {
		switch e.pcr {
		case 4:
			in.Events4 = append(in.Events4, EventKind{Kind: fmt.Sprintf("%#x", e.typ), Data: printable(e.data)})
		case 7:
			in.Events7 = append(in.Events7, EventKind{Kind: fmt.Sprintf("%#x", e.typ), Data: printable(e.data)})
		}
	}
	// the box's own certificate is not known here: whether Secure Boot is
	// on, with a platform key, is what this says
	if _, err := secureBoot(log, nil); err != nil && !isNotInDB(err) {
		in.SecureBoot, in.SecureBootErr = Why(err), err.Error()
	} else {
		in.SecureBoot = "on"
	}
	return in, nil
}

// EventKind is one event's kind in a log, and its data where it is text
type EventKind struct {
	Kind string `json:"kind"`
	Data string `json:"data,omitempty"`
}

// printable: an event's data if it is short text (an EV_EFI_ACTION's)
func printable(b []byte) string {
	if len(b) == 0 || len(b) > 80 {
		return ""
	}
	for _, c := range b {
		if c < 0x20 || c > 0x7e {
			return ""
		}
	}
	return string(b)
}

// isNotInDB: the one refusal that needs the box's own certificate
func isNotInDB(err error) bool {
	return err != nil && errors.Is(err, ErrSecureBoot) && (err.Error() == fmt.Errorf("%w: the box's own certificate is not in db", ErrSecureBoot).Error())
}
