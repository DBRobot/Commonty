// attest-check drives an attestation end to end against a TPM - a test's
// swtpm - with the same code a box and the validators run: the box's
// Begin, a challenge from each validator named, the box's answer, and the
// agreed state's judgement. For tests only (nix/tests/attest.nix,
// state/attest/testdata/make-vector.sh).
//
//	attest-check make-ek-cert -ek ek.pub -ca-key k.pem -ca-cert c.pem -o ek.der
//	attest-check make-log -db db.der -loader l.efi -stub s.efi -o log.bin
//	attest-check boot -loader systemd-bootx64.efi -stub lanzaboote_stub.efi -kernel bzImage -params '...'   (a release's Boot, JSON)
//	attest-check run -roots dir -keys dir -dir dir -log f [-db f] [-stub f -boot f] [-software] [-no-secure-boot] [-vector out.json]
package main

import (
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/asn1"
	"encoding/base64"
	"encoding/binary"
	"encoding/hex"
	"encoding/json"
	"encoding/pem"
	"flag"
	"fmt"
	"log"
	"math/big"
	"os"
	"path/filepath"
	"strings"
	"time"
	"unicode/utf16"

	"commonty/control/state/attest"
	"commonty/control/tpm"

	"github.com/google/go-tpm/tpm2"
)

func main() {
	log.SetFlags(0)
	if len(os.Args) < 2 {
		log.Fatal("attest-check make-ek-cert | make-log | run")
	}
	switch os.Args[1] {
	case "make-ek-cert":
		makeEKCert(os.Args[2:])
	case "make-log":
		makeLog(os.Args[2:])
	case "run":
		run(os.Args[2:])
	case "boot":
		bootPolicy(os.Args[2:])
	default:
		log.Fatalf("no such command: %s", os.Args[1])
	}
}

func must(err error) {
	if err != nil {
		log.Fatal(err)
	}
}

// makeEKCert: an EK certificate as a TPM maker writes one, for swtpm's EK,
// from a test CA: swtpm's vendor id, so only the test switch lets it in
func makeEKCert(args []string) {
	fs := flag.NewFlagSet("make-ek-cert", flag.ExitOnError)
	ekPath := fs.String("ek", "", "the EK's TPM2B_PUBLIC")
	caKey := fs.String("ca-key", "", "the CA's key, PKCS#8 PEM")
	caCert := fs.String("ca-cert", "", "the CA's certificate, PEM")
	out := fs.String("o", "", "the EK certificate, DER")
	_ = fs.Parse(args)
	raw, err := os.ReadFile(*ekPath)
	must(err)
	p2b, err := tpm2.Unmarshal[tpm2.TPM2BPublic](raw)
	must(err)
	pub, err := p2b.Contents()
	must(err)
	ek, err := tpm2.Pub(*pub)
	must(err)
	kp, err := os.ReadFile(*caKey)
	must(err)
	blk, _ := pem.Decode(kp)
	key, err := x509.ParsePKCS8PrivateKey(blk.Bytes)
	must(err)
	cp, err := os.ReadFile(*caCert)
	must(err)
	blk, _ = pem.Decode(cp)
	ca, err := x509.ParseCertificate(blk.Bytes)
	must(err)
	san := tpmSAN("id:00001014", "swtpm", "id:20191023")
	tmpl := &x509.Certificate{
		SerialNumber:       big.NewInt(time.Now().UnixNano()),
		NotBefore:          time.Now().Add(-time.Hour),
		NotAfter:           time.Now().AddDate(10, 0, 0),
		KeyUsage:           x509.KeyUsageKeyEncipherment,
		UnknownExtKeyUsage: []asn1.ObjectIdentifier{{2, 23, 133, 8, 1}},
		ExtraExtensions:    []pkix.Extension{{Id: asn1.ObjectIdentifier{2, 5, 29, 17}, Critical: true, Value: san}},
	}
	der, err := x509.CreateCertificate(rand.Reader, tmpl, ca, ek, key)
	must(err)
	must(os.WriteFile(*out, der, 0o644))
}

// tpmSAN: the alternative name a TPM's EK certificate carries
func tpmSAN(manufacturer, model, version string) []byte {
	attr := func(oid asn1.ObjectIdentifier, v string) pkix.AttributeTypeAndValue {
		return pkix.AttributeTypeAndValue{Type: oid, Value: v}
	}
	name := pkix.RDNSequence{
		{attr(asn1.ObjectIdentifier{2, 23, 133, 2, 1}, manufacturer)},
		{attr(asn1.ObjectIdentifier{2, 23, 133, 2, 2}, model)},
		{attr(asn1.ObjectIdentifier{2, 23, 133, 2, 3}, version)},
	}
	nb, err := asn1.Marshal(name)
	must(err)
	dir, err := asn1.Marshal(asn1.RawValue{Class: asn1.ClassContextSpecific, Tag: 4, IsCompound: true, Bytes: nb})
	must(err)
	out, err := asn1.Marshal(asn1.RawValue{Tag: asn1.TagSequence, IsCompound: true, Bytes: dir})
	must(err)
	return out
}

// makeLog: an event log as UEFI firmware writes one, booted under Secure
// Boot with db's certificate, and what to extend so the TPM agrees
func makeLog(args []string) {
	fs := flag.NewFlagSet("make-log", flag.ExitOnError)
	dbPath := fs.String("db", "", "the db certificate, DER")
	loader := fs.String("loader", "", "the first boot application (systemd-boot), PE")
	stub := fs.String("stub", "", "the second (the box's lanzaboote image), PE")
	off := fs.Bool("off", false, "Secure Boot off")
	out := fs.String("o", "", "the log")
	_ = fs.Parse(args)
	db, err := os.ReadFile(*dbPath)
	must(err)
	global := []byte{0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c}
	sb := byte(1)
	if *off {
		sb = 0
	}
	security := []byte{0xcb, 0xb2, 0x19, 0xd7, 0x3a, 0x3d, 0x96, 0x45, 0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f}
	type ev struct {
		pcr, typ uint32
		data     []byte
		digest   []byte // what was extended, where not the data's hash
	}
	// db as firmware keeps it: one EFI_SIGNATURE_LIST of one X.509 entry,
	// and no authority event, as many firmwares log none
	evs := []ev{
		{0, 0x80000008, []byte("test firmware"), nil}, // EV_S_CRTM_VERSION
		{7, 0x80000001, variable(global, "SecureBoot", []byte{sb}), nil},
		{7, 0x80000001, variable(global, "PK", sigList([]byte("pk"))), nil},
		{7, 0x80000001, variable(global, "KEK", sigList([]byte("kek"))), nil},
		{7, 0x80000001, variable(security, "db", sigList(db)), nil},
		{0, 4, []byte{0, 0, 0, 0}, nil}, // EV_SEPARATOR
		{2, 4, []byte{0, 0, 0, 0}, nil},
		{4, 0x80000007, []byte("Calling EFI Application from Boot Option"), nil},
		{4, 4, []byte{0, 0, 0, 0}, nil},
		{7, 4, []byte{0, 0, 0, 0}, nil},
	}
	// EV_EFI_BOOT_SERVICES_APPLICATION: each image's Authenticode digest
	for _, f := range []string{*loader, *stub} {
		if f == "" {
			continue
		}
		img, err := os.ReadFile(f)
		must(err)
		d, err := attest.Authenticode(img)
		must(err)
		evs = append(evs, ev{4, 0x80000003, []byte("a boot application"), d})
	}
	var b []byte
	le32 := func(v uint32) { b = binary.LittleEndian.AppendUint32(b, v) }
	le16 := func(v uint16) { b = binary.LittleEndian.AppendUint16(b, v) }
	spec := []byte("Spec ID Event03\x00")
	spec = append(spec, 0, 0, 0, 0, 0, 2, 0, 2)
	spec = binary.LittleEndian.AppendUint32(spec, 1)
	spec = append(spec, 0x0B, 0x00, 32, 0x00, 0)
	le32(0)
	le32(3)
	b = append(b, make([]byte, 20)...)
	le32(uint32(len(spec)))
	b = append(b, spec...)
	for _, e := range evs {
		d := e.digest
		if d == nil {
			h := sha256.Sum256(e.data)
			d = h[:]
		}
		le32(e.pcr)
		le32(e.typ)
		le32(1)
		le16(0x0B)
		b = append(b, d...)
		le32(uint32(len(e.data)))
		b = append(b, e.data...)
		fmt.Printf("%d:%s\n", e.pcr, hex.EncodeToString(d))
	}
	must(os.WriteFile(*out, b, 0o644))
}

// sigList: one EFI_SIGNATURE_LIST of one X.509 entry, its owner zero
func sigList(cert []byte) []byte {
	x509GUID := []byte{0xa1, 0x59, 0xc0, 0xa5, 0xe4, 0x94, 0xa7, 0x4a, 0x87, 0xb5, 0xab, 0x15, 0x5c, 0x2b, 0xf0, 0x72}
	b := append([]byte(nil), x509GUID...)
	b = binary.LittleEndian.AppendUint32(b, uint32(28+16+len(cert)))
	b = binary.LittleEndian.AppendUint32(b, 0)
	b = binary.LittleEndian.AppendUint32(b, uint32(16+len(cert)))
	b = append(b, make([]byte, 16)...)
	return append(b, cert...)
}

// bootPolicy: a release's Boot from its systemd-boot, lanzaboote stub and
// kernel, as the judge reads them
func bootPolicy(args []string) {
	fs := flag.NewFlagSet("boot", flag.ExitOnError)
	loader := fs.String("loader", "", "systemd-bootx64.efi")
	stub := fs.String("stub", "", "lanzaboote_stub.efi, as built")
	kernel := fs.String("kernel", "", "the kernel file the stub names")
	params := fs.String("params", "", "the kernel parameters after init=, space-joined")
	_ = fs.Parse(args)
	img, err := os.ReadFile(*loader)
	must(err)
	d, err := attest.Authenticode(img)
	must(err)
	img, err = os.ReadFile(*stub)
	must(err)
	st, err := attest.StubOf(img)
	must(err)
	k, err := os.ReadFile(*kernel)
	must(err)
	kh := sha256.Sum256(k)
	out, _ := json.MarshalIndent(attest.Boot{
		Loaders: []string{hex.EncodeToString(d)},
		Stubs:   []attest.Stub{st},
		Kernels: []string{hex.EncodeToString(kh[:])},
		Params:  []string{*params},
	}, "", " ")
	fmt.Println(string(out))
}

func variable(guid []byte, name string, value []byte) []byte {
	u := utf16.Encode([]rune(name))
	b := append([]byte(nil), guid...)
	b = binary.LittleEndian.AppendUint64(b, uint64(len(u)))
	b = binary.LittleEndian.AppendUint64(b, uint64(len(value)))
	for _, c := range u {
		b = binary.LittleEndian.AppendUint16(b, c)
	}
	return append(b, value...)
}

// Vector is one whole attestation, for the unit tests to judge again.
type Vector struct {
	Config     attest.Config      `json:"config"`
	Box        string             `json:"box"`
	Key        string             `json:"key"`
	At         int64              `json:"at"`
	Voters     []string           `json:"voters"`
	Begin      attest.Begin       `json:"begin"`
	Challenges []attest.Challenge `json:"challenges"`
	Response   attest.Response    `json:"response"`
}

func run(args []string) {
	fs := flag.NewFlagSet("run", flag.ExitOnError)
	roots := fs.String("roots", "", "the makers' roots: <vendor id>/<name>.pem")
	keys := fs.String("keys", "/boot/dd/unlock", "the join key's files")
	dir := fs.String("dir", "/var/lib/dd-control/attest", "where the AK is kept")
	logPath := fs.String("log", "/sys/kernel/security/tpm0/binary_bios_measurements", "the event log")
	db := fs.String("db", "", "the box's own Secure Boot certificate")
	software := fs.Bool("software", false, "let a software TPM in")
	noSB := fs.Bool("no-secure-boot", false, "let a boot without Secure Boot in")
	stub := fs.String("stub", "", "the lanzaboote image this boot ran")
	boot := fs.String("boot", "", "the release's Boot (attest-check boot), JSON")
	voters := fs.String("voters", "node1,node2", "the box voters who challenge")
	vector := fs.String("vector", "", "write the whole exchange here")
	_ = fs.Parse(args)

	cfg := attest.Config{Roots: map[string][]string{}}
	if *software || *noSB {
		cfg.Test = &attest.Test{SoftwareTPM: *software, NoSecureBoot: *noSB}
	}
	if *boot != "" {
		raw, err := os.ReadFile(*boot)
		must(err)
		cfg.Boot = &attest.Boot{}
		must(json.Unmarshal(raw, cfg.Boot))
	}
	vendors, err := os.ReadDir(*roots)
	must(err)
	for _, v := range vendors {
		files, _ := filepath.Glob(filepath.Join(*roots, v.Name(), "*.pem"))
		for _, f := range files {
			raw, err := os.ReadFile(f)
			must(err)
			cfg.Roots[v.Name()] = append(cfg.Roots[v.Name()], string(raw))
		}
	}

	box := tpm.Default()
	box.Keys, box.Dir, box.Log, box.DB, box.Fetch = *keys, *dir, *logPath, *db, nil
	box.Stub, box.ESP = *stub, ""
	if t := os.Getenv("TPM2TOOLS_TCTI"); t != "" {
		box.TCTI = t
		box.NoRM = strings.HasPrefix(t, "swtpm:")
	}
	begin, err := box.Begin()
	must(err)
	key := joinSPKI(begin.Join)
	der, _ := base64.StdEncoding.DecodeString(key)
	h := sha256.Sum256(der)
	name := "j" + hex.EncodeToString(h[:])[:10]
	now := time.Now().UnixMilli()
	vs := strings.Split(*voters, ",")

	st := attest.New()
	if _, err := st.CheckOpen(&cfg, name, key, begin, now); err != nil {
		log.Fatalf("begin: %v", err)
	}
	st.Open(&cfg, name, key, begin, now)
	var chals []attest.Challenge
	for _, v := range vs {
		c, err := tpm.MakeChallenge(begin, name, v)
		must(err)
		if err := st.CheckChallenge(vs, v, c, now); err != nil {
			log.Fatalf("challenge: %v", err)
		}
		st.ApplyChallenge(c)
		chals = append(chals, c)
	}
	resp, err := box.Respond(name, st.Pending[name].Challenges)
	must(err)
	if *vector != "" {
		v := Vector{Config: cfg, Box: name, Key: key, At: now, Voters: vs, Begin: begin, Challenges: chals, Response: resp}
		out, _ := json.MarshalIndent(v, "", " ")
		must(os.WriteFile(*vector, append(out, '\n'), 0o644))
	}
	rec, err := st.CheckResponse(&cfg, vs, name, resp, now)
	if err != nil {
		log.Fatalf("response: %v", err)
	}
	st.ApplyResponse(&cfg, vs, name, resp, now)
	out, _ := json.Marshal(rec)
	fmt.Printf("attested %s: %s\n", name, out)
}

func joinSPKI(join string) string {
	raw, err := base64.StdEncoding.DecodeString(join)
	must(err)
	p2b, err := tpm2.Unmarshal[tpm2.TPM2BPublic](raw)
	must(err)
	pub, err := p2b.Contents()
	must(err)
	k, err := tpm2.Pub(*pub)
	must(err)
	der, err := x509.MarshalPKIXPublicKey(k)
	must(err)
	return base64.StdEncoding.EncodeToString(der)
}
