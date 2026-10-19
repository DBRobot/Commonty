package attest

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"crypto/x509/pkix"
	"encoding/binary"
	"encoding/hex"
	"math/big"
	"testing"
	"time"
)

type sec struct {
	name string
	data []byte
}

// mkPE: a PE32+ EFI application with these sections, in this order,
// starting at entry
func mkPE(entry uint32, list ...sec) []byte {
	var names []string
	secs := map[string][]byte{}
	for _, s := range list {
		names = append(names, s.name)
		secs[s.name] = s.data
	}
	const fileAlign, secAlign, optSize = 0x200, 0x1000, 240
	hdr := 0x40 + 4 + 20 + optSize + 40*len(names)
	headers := (hdr + fileAlign - 1) / fileAlign * fileAlign
	b := make([]byte, headers)
	copy(b, "MZ")
	binary.LittleEndian.PutUint32(b[0x3c:], 0x40)
	copy(b[0x40:], "PE\x00\x00")
	coff := b[0x44:]
	binary.LittleEndian.PutUint16(coff[0:], 0x8664)
	binary.LittleEndian.PutUint16(coff[2:], uint16(len(names)))
	binary.LittleEndian.PutUint16(coff[16:], optSize)
	binary.LittleEndian.PutUint16(coff[18:], 0x22)
	opt := b[0x58:]
	binary.LittleEndian.PutUint16(opt[0:], 0x20b)
	binary.LittleEndian.PutUint32(opt[16:], entry)
	binary.LittleEndian.PutUint64(opt[24:], 0x10000000)
	binary.LittleEndian.PutUint32(opt[32:], secAlign)
	binary.LittleEndian.PutUint32(opt[36:], fileAlign)
	binary.LittleEndian.PutUint32(opt[60:], uint32(headers))
	binary.LittleEndian.PutUint16(opt[68:], 10)
	binary.LittleEndian.PutUint32(opt[108:], 16)
	tbl := 0x58 + optSize
	va := uint32(secAlign)
	for i, n := range names {
		data := secs[n]
		raw := (len(data) + fileAlign - 1) / fileAlign * fileAlign
		h := b[tbl+40*i:]
		copy(h[0:8], n)
		binary.LittleEndian.PutUint32(h[8:], uint32(len(data)))
		binary.LittleEndian.PutUint32(h[12:], va)
		binary.LittleEndian.PutUint32(h[16:], uint32(raw))
		binary.LittleEndian.PutUint32(h[20:], uint32(len(b)))
		binary.LittleEndian.PutUint32(h[36:], 0x40000040)
		b = append(b, data...)
		b = append(b, make([]byte, raw-len(data))...)
		va += (uint32(len(data)) + secAlign - 1) / secAlign * secAlign
	}
	binary.LittleEndian.PutUint32(opt[56:], va)
	return b
}

// signed: img as sbsign leaves it - a certificate table at the end, named
// in the data directory, and a checksum
func signed(img []byte) []byte {
	out := append([]byte(nil), img...)
	cert := make([]byte, 64)
	binary.LittleEndian.PutUint32(cert, 64)
	dir := 0x58 + 112 + 4*8
	binary.LittleEndian.PutUint32(out[dir:], uint32(len(out)))
	binary.LittleEndian.PutUint32(out[dir+4:], uint32(len(cert)))
	binary.LittleEndian.PutUint32(out[0x58+64:], 0xdeadbeef)
	return append(out, cert...)
}

type bootFixture struct {
	loader, base, stub, kernel []byte
	pol                        *Boot
}

// one release's boot: its systemd-boot, its lanzaboote stub, its kernel,
// and a box's image of that stub as lzbt makes it
func release(t *testing.T) bootFixture {
	t.Helper()
	f := bootFixture{
		loader: mkPE(0x1000, sec{".text", []byte("systemd-boot")}, sec{".data", []byte("loader")}),
		base:   mkPE(0x1000, stubBase...),
		kernel: []byte("the release's kernel"),
	}
	f.stub = withSections(f.kernel)
	ld, _ := Authenticode(f.loader)
	st, err := StubOf(f.base)
	if err != nil {
		t.Fatal(err)
	}
	kh := sha256.Sum256(f.kernel)
	f.pol = &Boot{Loaders: []string{hex.EncodeToString(ld)}, Stubs: []Stub{st}, Kernels: []string{hex.EncodeToString(kh[:])}}
	return f
}

var stubBase = []sec{{".text", []byte("lanzaboote stub")}, {".rdata", []byte("strings")}}

// withSections: the release's stub with lzbt's sections for kernel added
// after its own, as objcopy adds them, then extra; an extra section of the
// stub's own name replaces it
func withSections(kernel []byte, extra ...sec) []byte {
	kh := sha256.Sum256(kernel)
	var list []sec
	for _, b := range stubBase {
		for _, e := range extra {
			if e.name == b.name {
				b = e
			}
		}
		list = append(list, b)
	}
	list = append(list,
		sec{".osrel", []byte("ID=nixos")}, sec{".cmdline", []byte("init=/nix/store/x/init")},
		sec{".initrd", []byte(`\EFI\nixos\initrd.efi`)}, sec{".linux", []byte(`\EFI\nixos\kernel.efi`)},
		sec{".initrdh", make([]byte, 32)}, sec{".linuxh", kh[:]})
	for _, e := range extra {
		if e.name != ".text" && e.name != ".rdata" {
			list = append(list, e)
		}
	}
	return mkPE(0x1000, list...)
}

func pcr4(apps ...[]byte) []event {
	log := []event{
		{pcr: 4, typ: evEFIAction, sha256: make([]byte, 32)},
		{pcr: 4, typ: evSeparator, sha256: make([]byte, 32)},
	}
	for _, a := range apps {
		d, _ := Authenticode(a)
		log = append(log, event{pcr: 4, typ: evEFIBootApplication, sha256: d})
	}
	return log
}

func TestAuthenticodeLeavesOutTheSignature(t *testing.T) {
	f := release(t)
	a, err := Authenticode(f.stub)
	if err != nil {
		t.Fatal(err)
	}
	b, err := Authenticode(signed(f.stub))
	if err != nil || hex.EncodeToString(a) != hex.EncodeToString(b) {
		t.Fatalf("signing changed the digest: %v", err)
	}
	other := append([]byte(nil), f.stub...)
	other[len(other)-1] ^= 1
	c, _ := Authenticode(other)
	if hex.EncodeToString(a) == hex.EncodeToString(c) {
		t.Fatal("a changed image, the same digest")
	}
}

func TestTheReleasesBootIsAccepted(t *testing.T) {
	f := release(t)
	if err := checkBoot(f.pol, pcr4(signed(f.loader), signed(f.stub)), signed(f.stub)); err != nil {
		t.Fatal(err)
	}
}

func TestShimInPCR4IsRefused(t *testing.T) {
	f := release(t)
	shim := mkPE(0x1000, sec{".text", []byte("shim")}, sec{".vendor_cert", []byte("a distro's key")})
	grub := mkPE(0x1000, sec{".text", []byte("grub")})
	// shim first, then what it chains to
	wantErr(t, checkBoot(f.pol, pcr4(shim, grub), grub), "not the release's systemd-boot")
	// shim between systemd-boot and ours
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, shim, f.stub), f.stub), "3 boot applications")
	// shim in the stub's place, sent as the stub
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, shim), shim), "not the release's lanzaboote stub")
}

func TestAnUnknownBootApplicationIsRefused(t *testing.T) {
	f := release(t)
	app := mkPE(0x1000, sec{".text", []byte("something else")})
	// one the box did not send
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, app), f.stub), "not the stub the box sent")
	// the stub with code of its own added
	extra := withSections(f.kernel, sec{".evil", []byte("more")})
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, extra), extra), "section of its own")
	// the stub with its code changed
	changed := withSections(f.kernel, sec{".text", []byte("lanzaboote stuB")})
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, changed), changed), "not the release's")
	// the stub naming another kernel
	other := withSections([]byte("another kernel"))
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, other), other), "kernel not the release's")
	// a boot loader's own PCR 4 measurement (an edited command line)
	log := append(pcr4(f.loader, f.stub), event{pcr: 4, typ: 0x0D, sha256: make([]byte, 32)})
	wantErr(t, checkBoot(f.pol, log, f.stub), "of kind")
	// a release that names no boot
	wantErr(t, checkBoot(nil, pcr4(f.loader, f.stub), f.stub), "names no boot images")
}

// PCR 7 without the authority events many firmwares leave out: Secure
// Boot on, a PK, the box's certificate in db
func TestPCR7WithoutAuthorityEvents(t *testing.T) {
	k, _ := ecdsa.GenerateKey(elliptic.P256(), rand.Reader)
	mk := func(cn string) []byte {
		tmpl := &x509.Certificate{SerialNumber: big.NewInt(1), Subject: pkix.Name{CommonName: cn}, NotBefore: time.Unix(0, 0), NotAfter: time.Unix(1<<31, 0)}
		der, err := x509.CreateCertificate(rand.Reader, tmpl, tmpl, &k.PublicKey, k)
		if err != nil {
			t.Fatal(err)
		}
		return der
	}
	ours, theirs := mk("box db"), mk("someone else")
	ev := func(typ uint32, guid []byte, name string, value []byte) event {
		data := efiVariable(guid, name, value)
		d := sha256.Sum256(data)
		return event{pcr: 7, typ: typ, sha256: d[:], data: data}
	}
	base := func(pk []byte, db ...[]byte) []event {
		var lists []byte
		for _, c := range db {
			lists = append(lists, efiSigList(c)...)
		}
		return []event{
			ev(evEFIVariableConfig, efiGlobal, "SecureBoot", []byte{1}),
			ev(evEFIVariableConfig, efiGlobal, "PK", pk),
			ev(evEFIVariableConfig, efiImageSecurity, "db", lists),
		}
	}
	pk := efiSigList(theirs)
	if _, err := secureBoot(base(pk, theirs, ours), ours); err != nil {
		t.Fatal(err)
	}
	_, err := secureBoot(base(pk, theirs), ours)
	wantErr(t, err, "not in db")
	_, err = secureBoot(base(nil, ours), ours)
	wantErr(t, err, "setup mode")
	// where the firmware does log authorities, one must be the box's
	auth := func(c []byte) event {
		return ev(evEFIVariableAuthory, efiImageSecurity, "db", append(make([]byte, 16), c...))
	}
	_, err = secureBoot(append(base(pk, theirs, ours), auth(theirs)), ours)
	wantErr(t, err, "nothing booted under the box's own")
	if _, err := secureBoot(append(base(pk, theirs, ours), auth(theirs), auth(ours)), ours); err != nil {
		t.Fatal(err)
	}
}

func efiVariable(guid []byte, name string, value []byte) []byte {
	b := append([]byte(nil), guid...)
	b = binary.LittleEndian.AppendUint64(b, uint64(len(name)))
	b = binary.LittleEndian.AppendUint64(b, uint64(len(value)))
	for _, c := range name {
		b = binary.LittleEndian.AppendUint16(b, uint16(c))
	}
	return append(b, value...)
}

func efiSigList(cert []byte) []byte {
	b := append([]byte(nil), efiCertX509...)
	b = binary.LittleEndian.AppendUint32(b, uint32(28+16+len(cert)))
	b = binary.LittleEndian.AppendUint32(b, 0)
	b = binary.LittleEndian.AppendUint32(b, uint32(16+len(cert)))
	b = append(b, make([]byte, 16)...)
	return append(b, cert...)
}
