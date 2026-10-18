package attest

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"fmt"
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

// secureBoot: PCR 7 says Secure Boot was on, and db - the box's own
// certificate, its installer's record - is among those that let what
// booted run: in db with keys of its own (lanzaboote), or in MOK after a
// Microsoft-signed shim. Others may be there too (shim's, an option ROM's);
// what ran under them changed PCR 7 from what the join key holds to.
func secureBoot(log []event, db []byte) (bool, error) {
	on, seen, ours := true, false, false
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
		case e.typ == evEFIVariableAuthory && len(db) > 0:
			// an EFI_SIGNATURE_DATA (its owner, then the certificate), or
			// the certificate alone as shim logs it
			if bytes.Equal(value, db) || (len(value) > 16 && bytes.Equal(value[16:], db)) {
				ours = true
			}
		}
	}
	switch {
	case !seen || !on:
		return false, fmt.Errorf("%w: Secure Boot is off", ErrSecureBoot)
	case !ours:
		return false, fmt.Errorf("%w: nothing booted under the box's own certificate", ErrSecureBoot)
	}
	return true, nil
}
