package attest

import (
	"bytes"
	"crypto/sha256"
	"encoding/binary"
	"encoding/hex"
	"errors"
	"fmt"
	"regexp"
	"slices"
	"sort"
	"strings"
)

// peImage is a PE image as both the Authenticode digest and the stub check
// read it - one parse, so the two cannot see different images.
type peImage struct {
	img      []byte
	peOff    int
	opt      int // the optional header's offset
	optSize  int
	csum     int // the checksum's offset
	certDir  int // the certificate table's data directory entry
	headers  int // SizeOfHeaders
	sections []peSection
	symbols  bool // a COFF symbol table named (a kernel's PE header may)
}

type peSection struct {
	name            string
	vsize, va       uint32
	rawSize, rawPtr uint32
	chars           uint32
}

const (
	scnCode    = 0x00000020
	scnExecute = 0x20000000
	scnWrite   = 0x80000000
)

func parsePE(img []byte) (*peImage, error) {
	if len(img) < 0x40 || !bytes.HasPrefix(img, []byte("MZ")) {
		return nil, errors.New("not a PE image")
	}
	p := &peImage{img: img, peOff: int(binary.LittleEndian.Uint32(img[0x3c:]))}
	if p.peOff < 0x40 || p.peOff+24 > len(img) || !bytes.Equal(img[p.peOff:p.peOff+4], []byte("PE\x00\x00")) {
		return nil, errors.New("not a PE image")
	}
	coff := img[p.peOff+4:]
	nsec := int(binary.LittleEndian.Uint16(coff[2:]))
	p.symbols = binary.LittleEndian.Uint32(coff[8:]) != 0 || binary.LittleEndian.Uint32(coff[12:]) != 0
	p.optSize = int(binary.LittleEndian.Uint16(coff[16:]))
	p.opt = p.peOff + 24
	if p.opt+p.optSize > len(img) || p.optSize < 112 {
		return nil, errors.New("a PE optional header cut short")
	}
	if binary.LittleEndian.Uint16(img[p.opt:]) != 0x20b {
		return nil, errors.New("not PE32+")
	}
	// the certificate table is directory 4: a kernel's header lists six
	if dirs := binary.LittleEndian.Uint32(img[p.opt+108:]); dirs < 5 || dirs > 16 || 112+8*int(dirs) > p.optSize {
		return nil, errors.New("PE data directories out of place")
	}
	p.csum = p.opt + 64
	p.certDir = p.opt + 112 + 4*8
	p.headers = int(binary.LittleEndian.Uint32(img[p.opt+60:]))
	tbl := p.opt + p.optSize
	if nsec == 0 || nsec > 96 || tbl+40*nsec > p.headers || p.headers > len(img) {
		return nil, errors.New("PE headers out of place")
	}
	for i := range nsec {
		h := img[tbl+40*i:]
		s := peSection{
			name:  strings.TrimRight(string(h[:8]), "\x00"),
			vsize: binary.LittleEndian.Uint32(h[8:]), va: binary.LittleEndian.Uint32(h[12:]),
			rawSize: binary.LittleEndian.Uint32(h[16:]), rawPtr: binary.LittleEndian.Uint32(h[20:]),
			chars: binary.LittleEndian.Uint32(h[36:]),
		}
		if s.rawSize > 0 && (int(s.rawPtr) < p.headers || uint64(s.rawPtr)+uint64(s.rawSize) > uint64(len(img))) {
			return nil, fmt.Errorf("PE section %s outside the file", s.name)
		}
		p.sections = append(p.sections, s)
	}
	// no two sections share bytes of the file
	byRaw := slices.Clone(p.sections)
	sort.Slice(byRaw, func(i, j int) bool { return byRaw[i].rawPtr < byRaw[j].rawPtr })
	end := uint32(p.headers)
	for _, s := range byRaw {
		if s.rawSize == 0 {
			continue
		}
		if s.rawPtr < end {
			return nil, errors.New("PE sections overlap in the file")
		}
		end = s.rawPtr + s.rawSize
	}
	return p, nil
}

func (p *peImage) u32(off int) uint32 { return binary.LittleEndian.Uint32(p.img[off:]) }

func (p *peImage) data(s peSection) []byte {
	n := min(s.vsize, s.rawSize)
	return p.img[s.rawPtr : s.rawPtr+n]
}

// Authenticode is a PE image's SHA-256 Authenticode digest: what UEFI
// firmware extends into PCR 4 for each boot application it starts. It
// leaves out the checksum, the certificate table and its directory entry,
// so it is the same whichever key signed the image.
func Authenticode(img []byte) ([]byte, error) {
	p, err := parsePE(img)
	if err != nil {
		return nil, err
	}
	return p.authenticode()
}

func (p *peImage) authenticode() ([]byte, error) {
	img := p.img
	certOff, certSize := int(p.u32(p.certDir)), int(p.u32(p.certDir+4))
	h := sha256.New()
	h.Write(img[:p.csum])
	h.Write(img[p.csum+4 : p.certDir])
	h.Write(img[p.certDir+8 : p.headers])
	byRaw := slices.Clone(p.sections)
	sort.Slice(byRaw, func(i, j int) bool { return byRaw[i].rawPtr < byRaw[j].rawPtr })
	sum := p.headers
	for _, s := range byRaw {
		if s.rawSize == 0 {
			continue
		}
		h.Write(img[s.rawPtr : s.rawPtr+s.rawSize])
		sum += int(s.rawSize)
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

// headerHash: the DOS header and stub, the COFF header and the whole
// optional header, data directories included, but for what lzbt's
// objcopy changes: the section count, the time stamp (objcopy's own hash),
// SizeOfInitializedData and the checksum; and the certificate table's
// entry, which signing sets
func (p *peImage) headerHash() string {
	b := slices.Clone(p.img[:p.opt+p.optSize])
	clear(b[p.peOff+6 : p.peOff+8])   // NumberOfSections
	clear(b[p.peOff+8 : p.peOff+12])  // TimeDateStamp
	clear(b[p.opt+8 : p.opt+12])      // SizeOfInitializedData
	clear(b[p.csum : p.csum+4])       // CheckSum
	clear(b[p.certDir : p.certDir+8]) // the certificate table
	h := sha256.Sum256(b)
	return hex.EncodeToString(h[:])
}

// Stub is a lanzaboote stub as the release built it, before the box's
// install added its own sections: its headers, and each section in order.
type Stub struct {
	Header   string        `json:"header"` // headerHash, hex
	Sections []StubSection `json:"sections"`
}

type StubSection struct {
	Name  string `json:"name"`
	VA    uint32 `json:"va"`
	Size  uint32 `json:"size"` // VirtualSize
	Chars uint32 `json:"chars"`
	Hash  string `json:"hash"` // sha256 of its contents, hex
}

// the sections lzbt adds to the stub for each generation, in its order
var stubAdded = []string{".osrel", ".cmdline", ".initrd", ".linux", ".initrdh", ".linuxh"}

// StubOf describes the release's own stub (lanzaboote_stub.efi).
func StubOf(img []byte) (Stub, error) {
	p, err := parsePE(img)
	if err != nil {
		return Stub{}, err
	}
	st := Stub{Header: p.headerHash()}
	for _, s := range p.sections {
		h := sha256.Sum256(p.data(s))
		st.Sections = append(st.Sections, StubSection{Name: s.name, VA: s.va, Size: s.vsize, Chars: s.chars, Hash: hex.EncodeToString(h[:])})
	}
	return st, nil
}

// stubImage: what checkBoot learns from a box's lanzaboote image
type stubImage struct {
	digest  []byte
	kernel  []byte // .linuxh
	initrd  []byte // .initrdh
	cmdline string
}

// readStub: img is the release's stub st, its headers and every section
// as built, with only lzbt's six sections after them: data, never code,
// each at its own addresses past the stub's and inside the image the
// firmware loads, and the entry point in the stub's own code
func readStub(st Stub, img []byte) (*stubImage, error) {
	p, err := parsePE(img)
	if err != nil {
		return nil, err
	}
	// what a string or symbol table could name differently to two readers
	if p.symbols {
		return nil, errors.New("the stub names a symbol table")
	}
	for _, s := range p.sections {
		if strings.HasPrefix(s.name, "/") {
			return nil, errors.New("a PE section with a long name")
		}
	}
	if p.headerHash() != st.Header {
		return nil, errors.New("the stub's headers are not the release's")
	}
	n := len(st.Sections)
	if len(p.sections) != n+len(stubAdded) {
		return nil, fmt.Errorf("the stub has %d sections; the release's %d and lzbt's %d", len(p.sections), n, len(stubAdded))
	}
	entry := p.u32(p.opt + 16)
	sizeOfImage := p.u32(p.opt + 56)
	inCode, top := false, uint64(0)
	for i, want := range st.Sections {
		s := p.sections[i]
		h := sha256.Sum256(p.data(s))
		if s.name != want.Name || s.va != want.VA || s.vsize != want.Size || s.chars != want.Chars || hex.EncodeToString(h[:]) != want.Hash {
			return nil, fmt.Errorf("the stub's section %d (%s) is not the release's", i, s.name)
		}
		if s.chars&scnExecute != 0 && entry >= s.va && entry < s.va+s.vsize {
			inCode = true
		}
		top = max(top, uint64(s.va)+uint64(s.vsize))
	}
	if !inCode {
		return nil, errors.New("the stub's entry point is not in its own code")
	}
	out := &stubImage{}
	if out.digest, err = p.authenticode(); err != nil {
		return nil, err
	}
	for i, name := range stubAdded {
		s := p.sections[n+i]
		switch {
		case s.name != name:
			return nil, fmt.Errorf("the stub's section %d is %s, not lzbt's %s", n+i, s.name, name)
		case s.chars&(scnCode|scnExecute|scnWrite) != 0:
			return nil, fmt.Errorf("the stub's %s is code or writable", name)
		case uint64(s.va) < top || uint64(s.va)+uint64(s.vsize) > uint64(sizeOfImage) || s.vsize > s.rawSize:
			return nil, fmt.Errorf("the stub's %s is not past the stub's own, inside the image", name)
		}
		top = uint64(s.va) + uint64(s.vsize)
	}
	sec := func(i int) []byte { return p.data(p.sections[n+i]) }
	out.cmdline, out.initrd, out.kernel = string(sec(1)), sec(4), sec(5)
	if len(out.initrd) != sha256.Size || len(out.kernel) != sha256.Size {
		return nil, errors.New("the stub's kernel or initrd hash is not a sha256")
	}
	return out, nil
}

// a system's init, as lzbt writes it first on the command line
var initPath = regexp.MustCompile(`^init=/nix/store/[0-9a-df-np-sv-z]{32}-nixos-system-[A-Za-z0-9._+-]+/init$`)

// cmdlineOK: "init=<a NixOS system's init> <one of the release's kernel
// parameter lines>"
func cmdlineOK(b *Boot, cmdline string) bool {
	first, rest, _ := strings.Cut(cmdline, " ")
	return initPath.MatchString(first) && slices.Contains(b.Params, rest)
}
