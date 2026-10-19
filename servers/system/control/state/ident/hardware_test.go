package ident

import (
	"strings"
	"testing"
)

func TestTheMinimum(t *testing.T) {
	if why := Minimum(minimumBox); why != "" {
		t.Fatalf("the minimum itself refused: %s", why)
	}
	if Minimum(with(minimumBox, "arch_x86_64", 0, "arch_aarch64", 1)) != "" {
		t.Fatal("an aarch64 box refused")
	}
	// each one short of the minimum refuses, and says what
	for what, f := range map[string]map[string]float64{
		"memory":       with(minimumBox, "mem_total_bytes", MinMemTotal-1),
		"threads":      with(minimumBox, "threads", MinThreads-1),
		"system disk":  with(minimumBox, "system_disk_bytes", MinSystemDisk-1),
		"TPM 2.0":      with(minimumBox, "tpm_major", 1),
		"UEFI":         with(minimumBox, "uefi", 0),
		"Secure Boot":  with(minimumBox, "secure_boot_capable", 0),
		"processor":    with(minimumBox, "arch_x86_64", 0),
		"network link": with(minimumBox, "nics", 0),
	} {
		why := Minimum(f)
		if why == "" || !strings.Contains(why, what) {
			t.Errorf("%s short: %q", what, why)
		}
	}
	if why := Minimum(nil); !strings.Contains(why, "TPM") || !strings.Contains(why, "network") {
		t.Fatalf("a box that said nothing: %q", why)
	}
}

func TestWhatQualifies(t *testing.T) {
	cases := []struct {
		what string
		f    map[string]float64
		want string
	}{
		{"just enough to run", minimumBox, ""},
		{"compute", with(minimumBox, "threads", ComputeThreads, "mem_total_bytes", ComputeMemTotal), "compute"},
		{"a thread short of compute", with(minimumBox, "threads", ComputeThreads-1, "mem_total_bytes", ComputeMemTotal), ""},
		{"memory short of compute", with(minimumBox, "threads", ComputeThreads, "mem_total_bytes", ComputeMemTotal-1), ""},
		{"a Raspberry Pi, however big", with(minimumBox, "threads", 16, "mem_total_bytes", 64.0*(1<<30), "raspberry_pi", 1), ""},
		{"storage", with(minimumBox, "store_size_bytes", StorageBytes), "storage"},
		{"a byte short of storage", with(minimumBox, "store_size_bytes", StorageBytes-1), ""},
		{"a Pi with a big disk", with(minimumBox, "raspberry_pi", 1, "store_size_bytes", StorageBytes), "storage"},
		{"a GPU", with(minimumBox, "gpu_vram_bytes", GPUVRAM), "gpu"},
		{"a small GPU", with(minimumBox, "gpu_vram_bytes", GPUVRAM-1), ""},
		{"an encoder only", with(minimumBox, "video_encoders", 1), "gpu"},
		{"compute below the minimum", with(computeBox, "system_disk_bytes", MinSystemDisk-1), ""},
		{"storage with no TPM", with(minimumBox, "store_size_bytes", StorageBytes, "tpm_major", 0), ""},
		{"a GPU with no Secure Boot", with(minimumBox, "gpu_vram_bytes", 24.0*(1<<30), "secure_boot_capable", 0), ""},
		{"nothing said", nil, ""},
	}
	for _, c := range cases {
		if got := Qualify(c.f); got != c.want {
			t.Errorf("%s: %q, want %q", c.what, got, c.want)
		}
	}
}
