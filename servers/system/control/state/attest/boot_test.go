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
	"slices"
	"testing"
	"time"
)

type sec struct {
	name  string
	data  []byte
	chars uint32 // 0: read-only data
}

const textChars = 0x60000020 // code, execute, read

// mkPE: a PE32+ EFI application with these sections, in this order, its
// entry at entry, its headers with room for more sections and its image
// with room past them
func mkPE(entry uint32, list ...sec) []byte {
	const fileAlign, secAlign, optSize = 0x200, 0x1000, 240
	headers := 0x400
	b := make([]byte, headers)
	copy(b, "MZ")
	binary.LittleEndian.PutUint32(b[0x3c:], 0x40)
	copy(b[0x40:], "PE\x00\x00")
	coff := b[0x44:]
	binary.LittleEndian.PutUint16(coff[0:], 0x8664)
	binary.LittleEndian.PutUint16(coff[2:], uint16(len(list)))
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
	va := uint32(secAlign)
	for i, s := range list {
		b = putSection(b, i, s, va)
		va += (uint32(len(s.data)) + secAlign - 1) / secAlign * secAlign
	}
	binary.LittleEndian.PutUint32(b[0x58+56:], va+secAlign) // room for lzbt's
	return b
}

// putSection: s as section i at va, its data at the end of the file
func putSection(b []byte, i int, s sec, va uint32) []byte {
	const fileAlign = 0x200
	if s.chars == 0 {
		s.chars = 0x40000040
	}
	raw := (len(s.data) + fileAlign - 1) / fileAlign * fileAlign
	h := b[0x58+240+40*i:]
	copy(h[0:8], s.name)
	binary.LittleEndian.PutUint32(h[8:], uint32(len(s.data)))
	binary.LittleEndian.PutUint32(h[12:], va)
	binary.LittleEndian.PutUint32(h[16:], uint32(raw))
	binary.LittleEndian.PutUint32(h[20:], uint32(len(b)))
	binary.LittleEndian.PutUint32(h[36:], s.chars)
	b = append(b, s.data...)
	return append(b, make([]byte, raw-len(s.data))...)
}

// addSections: base with sections added after its own as lzbt's objcopy
// adds them - packed from the end of the last, the section count raised;
// va, where not 0, places each at an address of the test's choosing
func addSections(base []byte, list ...sec) []byte {
	b := slices.Clone(base)
	n := int(binary.LittleEndian.Uint16(b[0x46:]))
	last := b[0x58+240+40*(n-1):]
	va := binary.LittleEndian.Uint32(last[12:]) + binary.LittleEndian.Uint32(last[8:])
	for i, s := range list {
		b = putSection(b, n+i, s, va)
		va += uint32(len(s.data))
	}
	binary.LittleEndian.PutUint16(b[0x46:], uint16(n+len(list)))
	binary.LittleEndian.PutUint32(b[0x48:], 0x6ad671db) // objcopy's time stamp
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

const (
	testParams  = "usbcore.autosuspend=-1 root=fstab"
	testCmdline = "init=/nix/store/0123456789abcdfghijklmnpqrsvwxyz-nixos-system-jbox-26.05/init " + testParams
)

type bootFixture struct {
	loader, base, stub, kernel, initrd []byte
	pol                                *Boot
}

var stubBase = []sec{{".text", []byte("lanzaboote stub"), textChars}, {".rdata", []byte("strings"), 0}}

// one release's boot: its systemd-boot, its lanzaboote stub, its kernel,
// and a box's image of that stub as lzbt makes it
func release(t *testing.T) bootFixture {
	t.Helper()
	f := bootFixture{
		loader: mkPE(0x1000, sec{".text", []byte("systemd-boot"), textChars}, sec{".data", []byte("loader"), 0}),
		base:   mkPE(0x1000, stubBase...),
		kernel: []byte("the release's kernel"),
		initrd: []byte("the box's initrd"),
	}
	f.stub = f.image(lzbt(f.kernel, f.initrd, testCmdline))
	ld, _ := Authenticode(f.loader)
	st, err := StubOf(f.base)
	if err != nil {
		t.Fatal(err)
	}
	kh := sha256.Sum256(f.kernel)
	f.pol = &Boot{Loaders: []string{hex.EncodeToString(ld)}, Stubs: []Stub{st}, Kernels: []string{hex.EncodeToString(kh[:])}, Params: []string{testParams}}
	return f
}

func (f bootFixture) image(list []sec) []byte { return addSections(f.base, list...) }

// lzbt: the sections lzbt adds, in its order
func lzbt(kernel, initrd []byte, cmdline string) []sec {
	kh, ih := sha256.Sum256(kernel), sha256.Sum256(initrd)
	return []sec{
		{".osrel", []byte("ID=nixos"), 0}, {".cmdline", []byte(cmdline), 0},
		{".initrd", []byte(`\EFI\nixos\initrd.efi`), 0}, {".linux", []byte(`\EFI\nixos\kernel.efi`), 0},
		{".initrdh", ih[:], 0}, {".linuxh", kh[:], 0},
	}
}

func ev(pcr, typ uint32, data []byte) event {
	d := sha256.Sum256(data)
	return event{pcr: pcr, typ: typ, sha256: d[:], data: data}
}

func pcr4(apps ...[]byte) []event {
	log := []event{
		ev(4, evEFIAction, []byte("Calling EFI Application from Boot Option")),
		ev(4, evSeparator, []byte{0, 0, 0, 0}),
	}
	for _, a := range apps {
		d, _ := Authenticode(a)
		log = append(log, event{pcr: 4, typ: evEFIBootApplication, sha256: d, data: []byte("image load event")})
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
	shim := mkPE(0x1000, sec{".text", []byte("shim"), textChars}, sec{".vendor_cert", []byte("a distro's key"), 0})
	grub := mkPE(0x1000, sec{".text", []byte("grub"), textChars})
	// shim first, then what it chains to
	wantErr(t, checkBoot(f.pol, pcr4(shim, grub), grub), "not the release's systemd-boot")
	// shim between systemd-boot and ours
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, shim, f.stub), f.stub), "3 boot applications")
	// shim in the stub's place, sent as the stub
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, shim), shim), "not the release's lanzaboote stub")
}

func TestAnUnknownBootApplicationIsRefused(t *testing.T) {
	f := release(t)
	app := mkPE(0x1000, sec{".text", []byte("something else"), textChars})
	// one the box did not send
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, app), f.stub), "not the stub the box sent")
	// the stub naming another kernel
	other := f.image(lzbt([]byte("another kernel"), f.initrd, testCmdline))
	wantErr(t, checkBoot(f.pol, pcr4(f.loader, other), other), "kernel not the release's")
	// a boot loader's own PCR 4 measurement (an edited command line)
	log := append(pcr4(f.loader, f.stub), event{pcr: 4, typ: 0x0D, sha256: make([]byte, 32)})
	wantErr(t, checkBoot(f.pol, log, f.stub), "kind 0xd")
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
	v7 := func(typ uint32, guid []byte, name string, value []byte) event {
		return ev(7, typ, efiVariable(guid, name, value))
	}
	base := func(pk []byte, db ...[]byte) []event {
		var lists []byte
		for _, c := range db {
			lists = append(lists, efiSigList(c)...)
		}
		return []event{
			v7(evEFIVariableConfig, efiGlobal, "SecureBoot", []byte{1}),
			v7(evEFIVariableConfig, efiGlobal, "PK", pk),
			v7(evEFIVariableConfig, efiImageSecurity, "db", lists),
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
		return v7(evEFIVariableAuthory, efiImageSecurity, "db", append(make([]byte, 16), c...))
	}
	_, err = secureBoot(append(base(pk, theirs, ours), auth(theirs)), ours)
	wantErr(t, err, "nothing booted under the box's own")
	if _, err := secureBoot(append(base(pk, theirs, ours), auth(theirs), auth(ours)), ours); err != nil {
		t.Fatal(err)
	}
	// Secure Boot off, keys or none: said so in the person's words
	off := base(pk, theirs, ours)
	off[0] = v7(evEFIVariableConfig, efiGlobal, "SecureBoot", []byte{0})
	_, err = secureBoot(off, ours)
	if Why(err) != "Secure Boot is off" {
		t.Fatalf("%v: %q", err, Why(err))
	}
	_, err = secureBoot(base(nil, ours), ours)
	if Why(err) != "Secure Boot is off" {
		t.Fatalf("%v: %q", err, Why(err))
	}
	_, err = secureBoot(base(pk, theirs), ours)
	if Why(err) != "Secure Boot is not on with this box's own keys" {
		t.Fatalf("%v: %q", err, Why(err))
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

// a crafted image whose release sections all match must still be refused:
// a digest that passes is of the image, headers and all
func TestACraftedStubIsRefused(t *testing.T) {
	f := release(t)
	ok := lzbt(f.kernel, f.initrd, testCmdline)
	try := func(img []byte, part string) {
		t.Helper()
		wantErr(t, checkBoot(f.pol, pcr4(f.loader, img), img), part)
	}
	// code added among lzbt's sections
	code := slices.Clone(ok)
	code[0].chars = textChars
	try(f.image(code), "code or writable")
	// the stub's own code changed
	changed := mkPE(0x1000, sec{".text", []byte("lanzaboote stuB"), textChars}, stubBase[1])
	try(addSections(changed, ok...), "(.text) is not the release's")
	// a seventh section
	try(f.image(append(slices.Clone(ok), sec{".extra", []byte("more"), 0})), "sections")
	// lzbt's sections in another order
	swapped := slices.Clone(ok)
	swapped[4], swapped[5] = swapped[5], swapped[4]
	try(f.image(swapped), "not lzbt's")
	// a header changed: a data directory, the entry point, DllCharacteristics
	for _, off := range []int{0x58 + 112 + 8*5, 0x58 + 16, 0x58 + 70} {
		img := f.image(ok)
		img[off] ^= 1
		try(img, "headers are not the release's")
	}
	// lzbt's sections over the stub's own addresses
	img := f.image(ok)
	binary.LittleEndian.PutUint32(img[0x58+240+40*2+12:], 0x1000)
	try(img, "not past the stub's own")
	// past the image the firmware loads
	big := slices.Clone(ok)
	big[1].data = make([]byte, 0x3000)
	try(f.image(big), "not past the stub's own, inside the image")
	// a section with a long name, read from a string table
	img = f.image(ok)
	copy(img[0x58+240+40*2:], "/4\x00\x00\x00\x00\x00\x00")
	try(img, "long name")
	// the release's stub with its entry point outside its code
	base := mkPE(0x2000, stubBase...)
	st, err := StubOf(base)
	if err != nil {
		t.Fatal(err)
	}
	pol := *f.pol
	pol.Stubs = []Stub{st}
	img = addSections(base, ok...)
	wantErr(t, checkBoot(&pol, pcr4(f.loader, img), img), "entry point")
}

func TestTheStubsCommandLineAndInitrdAreTheReleases(t *testing.T) {
	f := release(t)
	try := func(img []byte, part string) {
		t.Helper()
		wantErr(t, checkBoot(f.pol, pcr4(f.loader, img), img), part)
	}
	try(f.image(lzbt(f.kernel, f.initrd, testCmdline+" init=/bin/sh")), "command line")
	try(f.image(lzbt(f.kernel, f.initrd, "init=/bin/sh "+testParams)), "command line")
	// initrds: not judged where the release lists none, judged where it does
	pol := *f.pol
	ih := sha256.Sum256([]byte("another initrd"))
	pol.Initrds = []string{hex.EncodeToString(ih[:])}
	wantErr(t, checkBoot(&pol, pcr4(f.loader, f.stub), f.stub), "initrd not the release's")
	ih = sha256.Sum256(f.initrd)
	pol.Initrds = append(pol.Initrds, hex.EncodeToString(ih[:]))
	if err := checkBoot(&pol, pcr4(f.loader, f.stub), f.stub); err != nil {
		t.Fatal(err)
	}
}

// what PCR 4 and PCR 7 events the judge passes over must say what was
// extended
func TestSkippedEventsAreBoundToTheirData(t *testing.T) {
	f := release(t)
	try := func(e event, part string) {
		t.Helper()
		log := append(pcr4(f.loader, f.stub), e)
		wantErr(t, checkBoot(f.pol, log, f.stub), part)
	}
	try(ev(4, evSeparator, []byte{1, 0, 0, 0}), "not one a boot of ours makes")
	unbound := ev(4, evSeparator, []byte{0, 0, 0, 0})
	unbound.sha256 = make([]byte, 32)
	try(unbound, "not one a boot of ours makes")
	try(ev(4, evEFIAction, []byte("Something Else")), "not one a boot of ours makes")
	action := ev(4, evEFIAction, []byte("Exit Boot Services Invocation"))
	action.sha256 = make([]byte, 32)
	try(action, "not one a boot of ours makes")

	// PCR 7: an event of another kind is refused, not passed over
	db := []byte("box db")
	good := []event{
		ev(7, evEFIVariableConfig, efiVariable(efiGlobal, "SecureBoot", []byte{1})),
		ev(7, evEFIVariableConfig, efiVariable(efiGlobal, "PK", []byte("pk"))),
		ev(7, evEFIVariableConfig, efiVariable(efiImageSecurity, "db", efiSigList(db))),
		ev(7, evSeparator, []byte{0, 0, 0, 0}),
	}
	if _, err := secureBoot(good, db); err != nil {
		t.Fatal(err)
	}
	_, err := secureBoot(append(slices.Clone(good), ev(7, evEFIAction, []byte("UEFI Debug Mode"))), db)
	wantErr(t, err, "of kind")
	bad := ev(7, evSeparator, []byte{0, 0, 0, 0})
	bad.sha256 = make([]byte, 32)
	_, err = secureBoot(append(slices.Clone(good), bad), db)
	wantErr(t, err, "not one")
}
