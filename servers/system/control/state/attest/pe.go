package attest

import (
	"bytes"
	"crypto/sha256"
	"debug/pe"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"sort"
)

// Authenticode is a PE image's SHA-256 Authenticode digest: what UEFI
// firmware extends into PCR 4 for each boot application it starts. It
// leaves out the checksum, the certificate table and its directory entry,
// so it is the same whichever key signed the image.
func Authenticode(img []byte) ([]byte, error) {
	if len(img) < 0x40 {
		return nil, errors.New("not a PE image")
	}
	peOff := int(binary.LittleEndian.Uint32(img[0x3c:]))
	if peOff < 0 || peOff+24 > len(img) || !bytes.Equal(img[peOff:peOff+4], []byte("PE\x00\x00")) {
		return nil, errors.New("not a PE image")
	}
	nsec := int(binary.LittleEndian.Uint16(img[peOff+6:]))
	optSize := int(binary.LittleEndian.Uint16(img[peOff+20:]))
	opt := peOff + 24
	if opt+optSize > len(img) || optSize < 96 {
		return nil, errors.New("a PE optional header cut short")
	}
	var dirs int
	switch binary.LittleEndian.Uint16(img[opt:]) {
	case 0x10b:
		dirs = opt + 96
	case 0x20b:
		dirs = opt + 112
	default:
		return nil, errors.New("neither PE32 nor PE32+")
	}
	csum := opt + 64
	certEntry := dirs + 4*8
	headers := int(binary.LittleEndian.Uint32(img[opt+60:]))
	if certEntry+8 > opt+optSize || headers > len(img) || headers < certEntry+8 {
		return nil, errors.New("PE headers out of place")
	}
	certOff := int(binary.LittleEndian.Uint32(img[certEntry:]))
	certSize := int(binary.LittleEndian.Uint32(img[certEntry+4:]))

	h := sha256.New()
	h.Write(img[:csum])
	h.Write(img[csum+4 : certEntry])
	h.Write(img[certEntry+8 : headers])
	type raw struct{ off, size int }
	var secs []raw
	tbl := opt + optSize
	if tbl+40*nsec > len(img) {
		return nil, errors.New("PE section table cut short")
	}
	for i := range nsec {
		s := img[tbl+40*i:]
		size := int(binary.LittleEndian.Uint32(s[16:]))
		off := int(binary.LittleEndian.Uint32(s[20:]))
		if size == 0 {
			continue
		}
		if off < 0 || size < 0 || off+size > len(img) {
			return nil, errors.New("a PE section outside the file")
		}
		secs = append(secs, raw{off, size})
	}
	sort.Slice(secs, func(i, j int) bool { return secs[i].off < secs[j].off })
	sum := headers
	for _, s := range secs {
		h.Write(img[s.off : s.off+s.size])
		sum += s.size
	}
	end := len(img)
	if certSize > 0 {
		if certOff+certSize != len(img) || certOff < sum {
			return nil, errors.New("the PE certificate table is not at the end")
		}
		end = certOff
	}
	if end > sum {
		h.Write(img[sum:end])
	}
	return h.Sum(nil), nil
}

// Stub is a lanzaboote stub as the release built it, before the box's
// install added its own sections: where it starts, and each section's
// address and contents.
type Stub struct {
	Entry    uint32            `json:"entry"`
	Sections map[string]string `json:"sections"` // name -> sha256(address LE32 || raw data), hex
}

// the sections lzbt adds to the stub for each generation
var stubAdded = map[string]bool{".osrel": true, ".cmdline": true, ".initrd": true, ".linux": true, ".initrdh": true, ".linuxh": true}

func peSections(img []byte) (entry uint32, secs map[string]*pe.Section, err error) {
	f, err := pe.NewFile(bytes.NewReader(img))
	if err != nil {
		return 0, nil, err
	}
	switch o := f.OptionalHeader.(type) {
	case *pe.OptionalHeader64:
		entry = o.AddressOfEntryPoint
	case *pe.OptionalHeader32:
		entry = o.AddressOfEntryPoint
	default:
		return 0, nil, errors.New("no PE optional header")
	}
	secs = map[string]*pe.Section{}
	for _, s := range f.Sections {
		if secs[s.Name] != nil {
			return 0, nil, fmt.Errorf("two PE sections named %s", s.Name)
		}
		secs[s.Name] = s
	}
	return entry, secs, nil
}

func sectionHash(s *pe.Section) (string, error) {
	data, err := s.Data()
	if err != nil {
		return "", err
	}
	h := sha256.New()
	_ = binary.Write(h, binary.LittleEndian, s.VirtualAddress)
	h.Write(data)
	return hex.EncodeToString(h.Sum(nil)), nil
}

// StubOf describes the release's own stub (lanzaboote_stub.efi).
func StubOf(img []byte) (Stub, error) {
	entry, secs, err := peSections(img)
	if err != nil {
		return Stub{}, err
	}
	st := Stub{Entry: entry, Sections: map[string]string{}}
	for name, s := range secs {
		if st.Sections[name], err = sectionHash(s); err != nil {
			return Stub{}, err
		}
	}
	return st, nil
}

// stubKernel: img is st with only lzbt's sections added; the kernel hash
// it holds (.linuxh)
func stubKernel(st Stub, img []byte) ([]byte, error) {
	entry, secs, err := peSections(img)
	if err != nil {
		return nil, err
	}
	if entry != st.Entry {
		return nil, errors.New("the stub starts elsewhere than the release's")
	}
	for name, want := range st.Sections {
		s := secs[name]
		if s == nil {
			return nil, fmt.Errorf("the stub has no %s", name)
		}
		got, err := sectionHash(s)
		if err != nil {
			return nil, err
		}
		if got != want {
			return nil, fmt.Errorf("the stub's %s is not the release's", name)
		}
	}
	for name := range secs {
		if st.Sections[name] == "" && !stubAdded[name] {
			return nil, fmt.Errorf("the stub has a section of its own, %s", name)
		}
	}
	for name := range stubAdded {
		if secs[name] == nil {
			return nil, fmt.Errorf("the stub has no %s", name)
		}
	}
	k, err := secs[".linuxh"].Data()
	if err != nil {
		return nil, err
	}
	if len(k) < sha256.Size {
		return nil, errors.New("the stub's kernel hash is short")
	}
	return k[:sha256.Size], nil
}
