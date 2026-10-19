package attest

import (
	"crypto"
	"crypto/sha256"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/asn1"
	"encoding/hex"
	"errors"
	"slices"
	"strings"
)

var (
	oidSAN          = asn1.ObjectIdentifier{2, 5, 29, 17}
	oidSubjectAttrs = asn1.ObjectIdentifier{2, 5, 29, 9}
	oidManufacturer = asn1.ObjectIdentifier{2, 23, 133, 2, 1}
	oidModel        = asn1.ObjectIdentifier{2, 23, 133, 2, 2}
	oidRSAESOAEP    = asn1.ObjectIdentifier{1, 2, 840, 113549, 1, 1, 7}
	// tcg-kp-EKCertificate, and the TCG's arc its certificate policies sit in
	oidEKCertificate = asn1.ObjectIdentifier{2, 23, 133, 8, 1}
	oidTCG           = asn1.ObjectIdentifier{2, 23, 133}
)

const (
	amdVendor = "414D4400"
	amdOrg    = "Advanced Micro Devices"
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

// ekCertKey: the EK certificate's key. Some makers' name it RSAES-OAEP, which Go leaves unparsed; the key itself is PKCS#1.
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

// ekMarked: the certificate says it is a TPM's EK certificate - the TCG's
// EK purpose, or a TCG certificate policy
func ekMarked(c *x509.Certificate) bool {
	for _, u := range c.UnknownExtKeyUsage {
		if u.Equal(oidEKCertificate) {
			return true
		}
	}
	for _, p := range c.PolicyIdentifiers {
		if len(p) > len(oidTCG) && p[:len(oidTCG)].Equal(oidTCG) {
			return true
		}
	}
	return false
}

// shared db certificates, sha256 of DER: Microsoft's (read from a Lenovo's
// db and KEK, 2026-10-19) and Canonical's shim key. A box claiming one as
// its own would pass with any boot those sign.
var sharedDBs = map[string]bool{
	"a1117f516a32cefcba3f2d1ace10a87972fd6bbe8fe0d0b996e09e65d802a503": true, // Microsoft Corporation KEK CA 2011
	"3cd3f0309edae228767a976dd40d9f4affc4fbd5218f2e8cc3c9dd97e8ac6f9d": true, // Microsoft Corporation KEK 2K CA 2023
	"e8e95f0733a55e8bad7be0a1413ee23c51fcea64b3c8fa6a786935fddcc71961": true, // Microsoft Windows Production PCA 2011
	"48e99b991f57fc52f76149599bff0a58c47154229b9f8d603ac40d3500248507": true, // Microsoft Corporation UEFI CA 2011
	"076f1fea90ac29155ebf77c17682f75f1fdd1be196da302dc8461e350a9ae330": true, // Windows UEFI CA 2023
	"f6124e34125bee3fe6d79a574eaa7b91c0e7bd9d929c1a321178efd611dad901": true, // Microsoft UEFI CA 2023
	"e5be3e64c6e66a281457ecdece0d6d0787577aad2a3a0144262c10c14ba8d8f1": true, // Microsoft Option ROM UEFI CA 2023
	"ed1fe72cb9ca31c9af5b757afcd733323d675825032e6ced7fe1ae9eb767998c": true, // Canonical Ltd. Master Certificate Authority
}

// sharedDB: one of the above, or any Microsoft made or signed
func sharedDB(c *x509.Certificate) bool {
	h := sha256.Sum256(c.Raw)
	return sharedDBs[hex.EncodeToString(h[:])] ||
		slices.Contains(c.Subject.Organization, "Microsoft Corporation") ||
		slices.Contains(c.Issuer.Organization, "Microsoft Corporation")
}
