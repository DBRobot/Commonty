package attest

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"slices"
	"unicode/utf16"
)

// The firmware's event log (TCG PC Client Platform Firmware Profile, the
// crypto-agile form every UEFI TPM 2.0 machine writes): what was measured
// into each PCR, in order. Replayed, it must give the PCRs the TPM quoted;
// then its PCR 7 events say whether Secure Boot was on and which db
// certificate let the boot loader run.

const (
	evNoAction           = 0x00000003
	evEFIVariableConfig  = 0x80000001
	evEFIVariableAuthory = 0x800000E0

	maxLog    = 1 << 20
	maxEvents = 10000
)

type event struct {
	pcr    uint32
	typ    uint32
	sha256 []byte // nil where the log has no SHA-256 digest for it
	data   []byte
}

type reader struct {
	b   []byte
	err error
}

func (r *reader) take(n int) []byte {
	if r.err != nil || n < 0 || n > len(r.b) {
		r.err = errors.New("cut short")
		return nil
	}
	out := r.b[:n]
	r.b = r.b[n:]
	return out
}

func (r *reader) u16() uint16 {
	b := r.take(2)
	if b == nil {
		return 0
	}
	return binary.LittleEndian.Uint16(b)
}

func (r *reader) u32() uint32 {
	b := r.take(4)
	if b == nil {
		return 0
	}
	return binary.LittleEndian.Uint32(b)
}

func parseLog(b []byte) ([]event, error) {
	if len(b) > maxLog {
		return nil, fmt.Errorf("%d bytes; at most %d", len(b), maxLog)
	}
	r := &reader{b: b}
	// the first event is SHA-1 form and says which digests the rest carry
	r.u32()
	if r.u32() != evNoAction {
		return nil, errors.New("not a crypto-agile log")
	}
	r.take(20)
	spec := &reader{b: r.take(int(r.u32()))}
	if r.err != nil {
		return nil, r.err
	}
	if !bytes.Equal(spec.take(16), []byte("Spec ID Event03\x00")) {
		return nil, errors.New("not a crypto-agile log")
	}
	spec.take(8) // platform class, version, errata, uintn size
	n := spec.u32()
	if n == 0 || n > 16 {
		return nil, errors.New("a digest list of no size")
	}
	sizes := map[uint16]int{}
	var order []uint16
	for range n {
		alg, size := spec.u16(), spec.u16()
		sizes[alg] = int(size)
		order = append(order, alg)
	}
	if spec.err != nil {
		return nil, spec.err
	}
	var out []event
	for len(r.b) > 0 {
		if len(out) == maxEvents {
			return nil, errors.New("too many events")
		}
		e := event{pcr: r.u32(), typ: r.u32()}
		count := r.u32()
		if count > uint32(len(order)) {
			return nil, errors.New("more digests than the log has kinds")
		}
		for range count {
			alg := r.u16()
			size, ok := sizes[alg]
			if !ok {
				return nil, fmt.Errorf("a digest of kind %#x the log did not name", alg)
			}
			d := r.take(size)
			if alg == 0x000B {
				e.sha256 = d
			}
		}
		e.data = r.take(int(r.u32()))
		if r.err != nil {
			return nil, fmt.Errorf("event %d: %v", len(out), r.err)
		}
		if e.pcr > 23 {
			return nil, fmt.Errorf("event %d: PCR %d", len(out), e.pcr)
		}
		out = append(out, e)
	}
	return out, nil
}

// replay: the SHA-256 bank, as the log says it was extended
func replay(log []event) (map[int][32]byte, error) {
	pcrs := map[int][32]byte{}
	for i, e := range log {
		if e.typ == evNoAction {
			// a locality other than 0 starts PCR 0 at that number
			if e.pcr == 0 && bytes.HasPrefix(e.data, []byte("StartupLocality\x00")) && len(e.data) == 17 {
				var v [32]byte
				v[31] = e.data[16]
				pcrs[0] = v
			}
			continue
		}
		if e.sha256 == nil {
			return nil, fmt.Errorf("event %d has no SHA-256 digest", i)
		}
		cur := pcrs[int(e.pcr)]
		pcrs[int(e.pcr)] = sha256.Sum256(append(cur[:], e.sha256...))
	}
	return pcrs, nil
}

var efiGlobal = []byte{0x61, 0xdf, 0xe4, 0x8b, 0xca, 0x93, 0xd2, 0x11, 0xaa, 0x0d, 0x00, 0xe0, 0x98, 0x03, 0x2b, 0x8c}

// variable: a UEFI_VARIABLE_DATA - its GUID, name and value
func variable(b []byte) (guid []byte, name string, value []byte, err error) {
	if len(b) < 32 {
		return nil, "", nil, errors.New("short")
	}
	guid = b[:16]
	nlen := binary.LittleEndian.Uint64(b[16:24])
	vlen := binary.LittleEndian.Uint64(b[24:32])
	rest := b[32:]
	if nlen > uint64(len(rest))/2 || vlen != uint64(len(rest))-2*nlen {
		return nil, "", nil, errors.New("sizes do not add up")
	}
	u := make([]uint16, nlen)
	for i := range u {
		u[i] = binary.LittleEndian.Uint16(rest[2*i:])
	}
	return guid, string(utf16.Decode(u)), rest[2*nlen:], nil
}

// measured: the digest is of the data itself (or, for older firmware's
// variable events, of the value alone), so what the data says was what
// was extended
func measured(e event, value []byte) bool {
	d := sha256.Sum256(e.data)
	v := sha256.Sum256(value)
	return bytes.Equal(e.sha256, d[:]) || bytes.Equal(e.sha256, v[:])
}

var (
	// EFI_IMAGE_SECURITY_DATABASE_GUID: db and dbx
	efiImageSecurity = []byte{0xcb, 0xb2, 0x19, 0xd7, 0x3a, 0x3d, 0x96, 0x45, 0xa3, 0xbc, 0xda, 0xd0, 0x0e, 0x67, 0x65, 0x6f}
	// EFI_CERT_X509_GUID
	efiCertX509 = []byte{0xa1, 0x59, 0xc0, 0xa5, 0xe4, 0x94, 0xa7, 0x4a, 0x87, 0xb5, 0xab, 0x15, 0x5c, 0x2b, 0xf0, 0x72}
)

// x509s: the certificates in an EFI_SIGNATURE_LIST array (a db's value)
func x509s(v []byte) ([][]byte, error) {
	var out [][]byte
	for len(v) > 0 {
		if len(v) < 28 {
			return nil, errors.New("a signature list cut short")
		}
		lsize := binary.LittleEndian.Uint32(v[16:])
		hsize := binary.LittleEndian.Uint32(v[20:])
		ssize := binary.LittleEndian.Uint32(v[24:])
		if uint64(lsize) > uint64(len(v)) || uint64(28)+uint64(hsize) > uint64(lsize) || ssize <= 16 {
			return nil, errors.New("a signature list's sizes do not add up")
		}
		body := v[28+hsize : lsize]
		if len(body)%int(ssize) != 0 {
			return nil, errors.New("a signature list's sizes do not add up")
		}
		if bytes.Equal(v[:16], efiCertX509) {
			for ; len(body) > 0; body = body[ssize:] {
				out = append(out, body[16:ssize])
			}
		}
		v = v[lsize:]
	}
	return out, nil
}

// secureBoot: PCR 7 says Secure Boot was on with keys enrolled (a PK: not
// setup mode), and the box's own certificate (its installer's record) in
// the db measured. Where the firmware logs which db entry let a boot
// application run (EV_EFI_VARIABLE_AUTHORITY), one must be the box's; many
// firmwares log none, and then PCR 4 (bootApps) says what ran.
func secureBoot(log []event, db []byte) (bool, error) {
	on, seen, pk, inDB, authorities, ours := true, false, false, false, 0, false
	for _, e := range log {
		if e.pcr != 7 || (e.typ != evEFIVariableConfig && e.typ != evEFIVariableAuthory) {
			continue
		}
		guid, name, value, err := variable(e.data)
		if err != nil {
			return false, fmt.Errorf("a PCR 7 event: %v", err)
		}
		if !measured(e, value) {
			return false, errors.New("a PCR 7 event whose data is not what was measured")
		}
		switch {
		case e.typ == evEFIVariableConfig && name == "SecureBoot" && bytes.Equal(guid, efiGlobal):
			seen = true
			on = on && bytes.Equal(value, []byte{1})
		case e.typ == evEFIVariableConfig && name == "PK" && bytes.Equal(guid, efiGlobal):
			pk = len(value) > 0
		case e.typ == evEFIVariableConfig && name == "db" && bytes.Equal(guid, efiImageSecurity):
			certs, err := x509s(value)
			if err != nil {
				return false, fmt.Errorf("the measured db: %v", err)
			}
			for _, c := range certs {
				inDB = inDB || (len(db) > 0 && bytes.Equal(c, db))
			}
		case e.typ == evEFIVariableAuthory && name == "db" && bytes.Equal(guid, efiImageSecurity):
			authorities++
			// an EFI_SIGNATURE_DATA: its owner, then the certificate
			if len(db) > 0 && len(value) > 16 && bytes.Equal(value[16:], db) {
				ours = true
			}
		}
	}
	switch {
	case !seen || !on:
		return false, fmt.Errorf("%w: Secure Boot is off", ErrSecureBoot)
	case !pk:
		return false, fmt.Errorf("%w: no platform key (setup mode)", ErrSecureBoot)
	case !inDB:
		return false, fmt.Errorf("%w: the box's own certificate is not in db", ErrSecureBoot)
	case authorities > 0 && !ours:
		return false, fmt.Errorf("%w: nothing booted under the box's own certificate", ErrSecureBoot)
	}
	return true, nil
}

const (
	evSeparator          = 0x00000004
	evEFIAction          = 0x80000007
	evEFIBootApplication = 0x80000003
)

// bootApps: what PCR 4 says ran - each boot application's Authenticode
// digest, in order. Any other kind of PCR 4 event (a boot loader's own
// measurement: an edited command line, a file) makes the boot unknown.
func bootApps(log []event) ([][]byte, error) {
	var apps [][]byte
	for i, e := range log {
		if e.pcr != 4 {
			continue
		}
		switch e.typ {
		case evNoAction, evSeparator, evEFIAction:
		case evEFIBootApplication:
			apps = append(apps, e.sha256)
		default:
			return nil, fmt.Errorf("PCR 4 event %d is of kind %#x", i, e.typ)
		}
	}
	return apps, nil
}

// Boot is what the release booted: systemd-boot as lanzaboote installs it
// (any of the release's), its stub, and the kernels a stub may name.
type Boot struct {
	Loaders []string `json:"loaders"` // Authenticode SHA-256, hex
	Stubs   []Stub   `json:"stubs"`
	Kernels []string `json:"kernels"` // sha256 of the kernel file, hex (lzbt's .linuxh)
}

// checkBoot: PCR 4 holds exactly two boot applications - one of the
// release's systemd-boot, then stub, the box's lanzaboote image, which is
// one of the release's stubs with only lzbt's sections added and naming
// one of its kernels. Anything else (shim, a grub, Windows' boot manager,
// another stub) and the boot is not ours.
func checkBoot(b *Boot, log []event, stub []byte) error {
	if b == nil {
		return fmt.Errorf("%w: the release names no boot images", ErrSecureBoot)
	}
	apps, err := bootApps(log)
	if err != nil {
		return fmt.Errorf("%w: %v", ErrSecureBoot, err)
	}
	if len(apps) != 2 {
		return fmt.Errorf("%w: %d boot applications ran; systemd-boot and the stub only", ErrSecureBoot, len(apps))
	}
	if !slices.Contains(b.Loaders, hex.EncodeToString(apps[0])) {
		return fmt.Errorf("%w: the first boot application is not the release's systemd-boot", ErrSecureBoot)
	}
	d, err := Authenticode(stub)
	if err != nil {
		return fmt.Errorf("%w: the stub: %v", ErrSecureBoot, err)
	}
	if !bytes.Equal(d, apps[1]) {
		return fmt.Errorf("%w: the second boot application is not the stub the box sent", ErrSecureBoot)
	}
	var last error = errors.New("no stub")
	for _, st := range b.Stubs {
		k, err := stubKernel(st, stub)
		if err != nil {
			last = err
			continue
		}
		if !slices.Contains(b.Kernels, hex.EncodeToString(k)) {
			return fmt.Errorf("%w: the stub names a kernel not the release's", ErrSecureBoot)
		}
		return nil
	}
	return fmt.Errorf("%w: not the release's lanzaboote stub: %v", ErrSecureBoot, last)
}
