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
	// PCR 4's boot applications, Authenticode digests in the order run
	Apps          []string `json:"apps"`
	SecureBoot    string   `json:"secureBoot"` // "on", or why not (Why)
	SecureBootErr string   `json:"secureBootErr,omitempty"`
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
	apps, err := bootApps(log)
	if err != nil {
		return nil, err
	}
	for _, a := range apps {
		in.Apps = append(in.Apps, hex.EncodeToString(a))
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

// isNotInDB: the one refusal that needs the box's own certificate
func isNotInDB(err error) bool {
	return err != nil && errors.Is(err, ErrSecureBoot) && (err.Error() == fmt.Errorf("%w: the box's own certificate is not in db", ErrSecureBoot).Error())
}
