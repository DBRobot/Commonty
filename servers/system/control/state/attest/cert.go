package attest

import (
	"crypto"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/asn1"
	"encoding/hex"
	"errors"
	"strings"
)

var (
	oidSAN          = asn1.ObjectIdentifier{2, 5, 29, 17}
	oidSubjectAttrs = asn1.ObjectIdentifier{2, 5, 29, 9}
	oidManufacturer = asn1.ObjectIdentifier{2, 23, 133, 2, 1}
	oidModel        = asn1.ObjectIdentifier{2, 23, 133, 2, 2}
	oidRSAESOAEP    = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 1, 7}
)

// the makers whose roots the release ships, for messages
var vendors = map[string]string{
	"494E5443": "Intel",
	"414D4400": "AMD",
	"49465800": "Infineon",
	"53544D20": "STMicroelectronics",
	"4E544300": "Nuvoton",
	"00001014": "swtpm",
	"49424D00": "IBM",
}

func vendorName(id string) string {
	if n, ok := vendors[id]; ok {
		return n
	}
	return "maker " + id
}

// tpmInfo: the TPM's maker (TCG vendor id, upper hex) and model, from the
// EK certificate's subject alternative name (TCG EK Credential Profile),
// or its subject where an old certificate put them
func tpmInfo(c *x509.Certificate) (vendor, model string) {
	var names []pkix.AttributeTypeAndValue
	for _, e := range c.Extensions {
		if !e.Id.Equal(oidSAN) {
			continue
		}
		var gns []asn1.RawValue
		if _, err := asn1.Unmarshal(e.Value, &gns); err != nil {
			continue
		}
		for _, gn := range gns {
			if gn.Class != asn1.ClassContextSpecific || gn.Tag != 4 {
				continue
			}
			var rdns pkix.RDNSequence
			if _, err := asn1.Unmarshal(gn.Bytes, &rdns); err != nil {
				continue
			}
			for _, set := range rdns {
				names = append(names, set...)
			}
		}
	}
	names = append(names, c.Subject.Names...)
	for _, n := range names {
		s, _ := n.Value.(string)
		switch {
		case n.Type.Equal(oidManufacturer) && vendor == "":
			vendor = vendorID(s)
		case n.Type.Equal(oidModel) && model == "":
			model = strings.TrimSpace(s)
		}
	}
	return vendor, model
}

// vendorID: "id:494E5443" -> "494E5443"
func vendorID(s string) string {
	s = strings.TrimPrefix(strings.TrimSpace(s), "id:")
	b, err := hex.DecodeString(s)
	if err != nil || len(b) == 0 || len(b) > 4 {
		return ""
	}
	return strings.ToUpper(hex.EncodeToString(append(make([]byte, 4-len(b)), b...)))
}

// tpmExtensionsHandled: an EK certificate marks its alternative name
// critical and holds only a directory name there, which Go leaves
// unhandled; tpmInfo reads it. The subject directory attributes (the TPM
// spec it meets) say nothing to check.
func tpmExtensionsHandled(c *x509.Certificate) {
	var keep []asn1.ObjectIdentifier
	for _, id := range c.UnhandledCriticalExtensions {
		if !id.Equal(oidSAN) && !id.Equal(oidSubjectAttrs) {
			keep = append(keep, id)
		}
	}
	c.UnhandledCriticalExtensions = keep
}

// ekCertKey: the EK certificate's key. Some makers' (AMD's among them) name
// it RSAES-OAEP, which Go leaves unparsed; the key itself is PKCS#1.
func ekCertKey(c *x509.Certificate) (crypto.PublicKey, error) {
	if c.PublicKey != nil {
		return c.PublicKey, nil
	}
	var spki struct {
		Algorithm pkix.AlgorithmIdentifier
		Key       asn1.BitString
	}
	if rest, err := asn1.Unmarshal(c.RawSubjectPublicKeyInfo, &spki); err != nil || len(rest) > 0 {
		return nil, errors.New("the EK certificate's key does not parse")
	}
	if !spki.Algorithm.Algorithm.Equal(oidRSAESOAEP) {
		return nil, errors.New("the EK certificate's key is of a kind not known")
	}
	return x509.ParsePKCS1PublicKey(spki.Key.RightAlign())
}
