package attest

import (
	"bytes"
	"crypto/ecdsa"
	"crypto/sha256"
	"encoding/binary"
	"errors"
	"fmt"

	"github.com/google/go-tpm/tpm2"
)

// The join key's policy (docs/dev/attestation.md, "The join key across
// updates"): TPM2_PolicyAuthorize by the box's approver A, a P-256 key
// whose private half is sealed in the TPM under that same policy. A
// approves one policy per boot it may unseal under: PCRs 4 and 7 as that
// boot measures them, and the box's epoch counter equal to the epoch the
// approval was made for. Raising the counter retires every older approval.

// ApprovalRef is the policyRef of every approval.
var ApprovalRef = []byte("commonty boot v1")

// EpochIndex is the NV counter that holds the box's epoch.
const EpochIndex = 0x01500DD0

const (
	ccPolicyPCR       = 0x0000017F
	ccPolicyNV        = 0x00000149
	ccPolicyAuthorize = 0x0000016A
)

// PolicyPCR47: TPM2_PolicyPCR over SHA-256 PCRs 4 and 7 at these values,
// from an empty policy
func PolicyPCR47(pcr4, pcr7 [32]byte) []byte {
	sel := []byte{0, 0, 0, 1, 0x00, 0x0B, 3, 0x90, 0, 0}
	d := sha256.New()
	d.Write(pcr4[:])
	d.Write(pcr7[:])
	h := sha256.New()
	h.Write(make([]byte, 32))
	h.Write(u32(ccPolicyPCR))
	h.Write(sel)
	h.Write(d.Sum(nil))
	return h.Sum(nil)
}

// PolicyNVEq extends prev with TPM2_PolicyNV: the index named nvName
// holds operand at offset 0
func PolicyNVEq(prev, nvName, operand []byte) []byte {
	args := sha256.New()
	args.Write(operand)
	args.Write([]byte{0, 0}) // offset
	args.Write([]byte{0, 0}) // TPM_EO_EQ
	h := sha256.New()
	h.Write(prev)
	h.Write(u32(ccPolicyNV))
	h.Write(args.Sum(nil))
	h.Write(nvName)
	return h.Sum(nil)
}

// Approved is the policy an approval names: this boot's PCRs 4 and 7, at
// this epoch of the counter named epochName.
func Approved(pcr4, pcr7 [32]byte, epochName []byte, epoch uint64) []byte {
	return PolicyNVEq(PolicyPCR47(pcr4, pcr7), epochName, binary.BigEndian.AppendUint64(nil, epoch))
}

// PolicyAuthorize: the join key's policy, from the approver's name
func PolicyAuthorize(approverName []byte) []byte {
	h := sha256.New()
	h.Write(make([]byte, 32))
	h.Write(u32(ccPolicyAuthorize))
	h.Write(approverName)
	first := h.Sum(nil)
	h = sha256.New()
	h.Write(first)
	h.Write(ApprovalRef)
	return h.Sum(nil)
}

// ApprovalDigest is what the approver signs for approved: aHash of
// TPM2_PolicyAuthorize.
func ApprovalDigest(approved []byte) []byte {
	h := sha256.New()
	h.Write(approved)
	h.Write(ApprovalRef)
	return h.Sum(nil)
}

func u32(v uint32) []byte { return binary.BigEndian.AppendUint32(nil, v) }

// EpochPublic is the epoch counter's public area as the box defines it:
// a counter the owner reads and raises, written once at least.
func EpochPublic() tpm2.TPMSNVPublic {
	return tpm2.TPMSNVPublic{
		NVIndex: EpochIndex,
		NameAlg: tpm2.TPMAlgSHA256,
		Attributes: tpm2.TPMANV{
			OwnerWrite: true, OwnerRead: true, NoDA: true, Written: true,
			NT: tpm2.TPMNTCounter,
		},
		DataSize: 8,
	}
}

// epochName: the counter's name, from its public area, which must be a
// counter (a counter is never set back: one defined again starts at the
// highest any counter in the TPM reached)
func epochName(raw []byte) ([]byte, error) {
	p2b, err := tpm2.Unmarshal[tpm2.TPM2BNVPublic](raw)
	if err != nil {
		return nil, err
	}
	pub, err := p2b.Contents()
	if err != nil {
		return nil, err
	}
	if pub.Attributes.NT != tpm2.TPMNTCounter || !pub.Attributes.Written || pub.NameAlg != tpm2.TPMAlgSHA256 || pub.DataSize != 8 {
		return nil, errors.New("the epoch index is not a written SHA-256 counter")
	}
	name, err := tpm2.NVName(pub)
	if err != nil {
		return nil, err
	}
	return name.Buffer, nil
}

// approverKey: the approver's public area - a P-256 key - and its name
func approverKey(raw []byte) (*ecdsa.PublicKey, []byte, error) {
	p2b, err := tpm2.Unmarshal[tpm2.TPM2BPublic](raw)
	if err != nil {
		return nil, nil, err
	}
	pub, err := p2b.Contents()
	if err != nil {
		return nil, nil, err
	}
	if pub.Type != tpm2.TPMAlgECC || pub.NameAlg != tpm2.TPMAlgSHA256 {
		return nil, nil, errors.New("the approver is not a SHA-256-named ECC key")
	}
	k, err := tpm2.Pub(*pub)
	if err != nil {
		return nil, nil, err
	}
	ek, ok := k.(*ecdsa.PublicKey)
	if !ok || ek.Curve.Params().Name != "P-256" {
		return nil, nil, errors.New("the approver is not a P-256 key")
	}
	name, err := tpm2.ObjectName(pub)
	if err != nil {
		return nil, nil, err
	}
	return ek, name.Buffer, nil
}

// PredictPCR4: PCR 4 as the log gives it with its boot applications'
// digests replaced, in order, by apps - what a boot of those images, on
// the same firmware, will measure
func PredictPCR4(logRaw []byte, apps [][]byte) ([32]byte, error) {
	log, err := parseLog(logRaw)
	if err != nil {
		return [32]byte{}, err
	}
	n := 0
	for i, e := range log {
		if e.pcr == 4 && e.typ == evEFIBootApplication {
			if n >= len(apps) {
				return [32]byte{}, fmt.Errorf("the log ran %d boot applications or more; %d given", n+1, len(apps))
			}
			log[i].sha256 = apps[n]
			n++
		}
	}
	if n != len(apps) {
		return [32]byte{}, fmt.Errorf("the log ran %d boot applications; %d given", n, len(apps))
	}
	pcrs, err := replay(log)
	if err != nil {
		return [32]byte{}, err
	}
	return pcrs[4], nil
}

// BootApps: the boot applications' digests the log says ran, in order.
func BootApps(logRaw []byte) ([][]byte, error) {
	log, err := parseLog(logRaw)
	if err != nil {
		return nil, err
	}
	var apps [][]byte
	for _, e := range log {
		if e.pcr == 4 && e.typ == evEFIBootApplication {
			apps = append(apps, bytes.Clone(e.sha256))
		}
	}
	return apps, nil
}

// Replay47: PCRs 4 and 7 as the log gives them.
func Replay47(logRaw []byte) (pcr4, pcr7 [32]byte, err error) {
	log, err := parseLog(logRaw)
	if err != nil {
		return pcr4, pcr7, err
	}
	pcrs, err := replay(log)
	if err != nil {
		return pcr4, pcr7, err
	}
	return pcrs[4], pcrs[7], nil
}
