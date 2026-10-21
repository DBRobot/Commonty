package ident

// What a box's hardware must be (docs/dev/joining.md, "Hardware"): every
// number David tunes is here. A box below the minimum can't join at all;
// one at the minimum joins as a stranger that holds copies; one that also
// brings compute, storage or a GPU qualifies, and gives its member access.
// Read from the box's own report as its heartbeat carries it (dd-control
// facts), which is its word until TPM attestation is asked for.

import (
	"fmt"
	"strings"
)

const (
	// the minimum: what building and running its own system needs (boxes
	// don't share builds; rustc peaks near 1 GB a job; the closure is 5-11
	// GB, with generations and its share of the store beside it)
	MinThreads    = 4
	MinMemTotal   = 7 << 30 // "8 GiB": MemTotal leaves out what firmware and graphics keep
	MinSystemDisk = 128e9   // the disk the system is on, bytes

	// compute: enough to host a service or a member's computer, which are
	// capped at 8 threads and 16 GiB
	ComputeThreads  = 8
	ComputeMemTotal = 14.5 * (1 << 30) // "16 GiB"
	// storage: usable store, bytes; copies are kept twice
	StorageBytes = 4e12
	// a GPU: VRAM on an nvidia or amdgpu card, bytes
	GPUVRAM = 8 << 30
)

// Minimum: why a box can't join, or "" when it can. Each reason says what
// is missing in words the person setting the box up can act on.
func Minimum(f map[string]float64) string {
	why := Missing(f)
	if len(why) == 0 {
		return ""
	}
	return "needs " + strings.Join(why, ", ")
}

// Missing: what the box lacks of the minimum, one item each
func Missing(f map[string]float64) []string {
	var why []string
	if f["tpm_major"] != 2 {
		why = append(why, "a TPM 2.0")
	}
	if f["uefi"] != 1 {
		why = append(why, "UEFI firmware")
	}
	if f["secure_boot_capable"] != 1 {
		why = append(why, "firmware that can do Secure Boot")
	}
	if f["arch_x86_64"] != 1 && f["arch_aarch64"] != 1 {
		why = append(why, "an x86_64 or aarch64 processor")
	}
	if f["threads"] < MinThreads {
		why = append(why, fmt.Sprintf("%d processor threads (it has %.0f)", MinThreads, f["threads"]))
	}
	if f["mem_total_bytes"] < MinMemTotal {
		why = append(why, fmt.Sprintf("8 GiB of memory (it has %.1f GiB)", f["mem_total_bytes"]/(1<<30)))
	}
	if f["system_disk_bytes"] < MinSystemDisk {
		why = append(why, fmt.Sprintf("a %d GB system disk (it has %.0f GB)", int(MinSystemDisk/1e9), f["system_disk_bytes"]/1e9))
	}
	if f["nics"] < 1 {
		why = append(why, "a network link")
	}
	return why
}

// Qualify: what a box brings beyond the minimum, "" for nothing (it joins,
// as a stranger, and gives no access): compute, storage or gpu.
func Qualify(f map[string]float64) string {
	if Minimum(f) != "" {
		return ""
	}
	switch {
	// a Raspberry Pi never qualifies by its compute
	case f["threads"] >= ComputeThreads && f["mem_total_bytes"] >= ComputeMemTotal && f["raspberry_pi"] == 0:
		return "compute"
	case f["store_size_bytes"] >= StorageBytes:
		return "storage"
	// integrated graphics count by their encoder alone, never their VRAM
	case f["gpu_vram_bytes"] >= GPUVRAM || f["video_encoders"] >= 1:
		return "gpu"
	}
	return ""
}
