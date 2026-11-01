package ident

import "testing"

func TestMinimumByInstalledMemoryFromVersion3(t *testing.T) {
	f := map[string]float64{
		"tpm_major": 2, "uefi": 1, "secure_boot_capable": 1, "arch_x86_64": 1,
		"threads": 8, "system_disk_bytes": 256e9, "nics": 1,
		"mem_total_bytes": 5.7 * (1 << 30), "mem_installed_bytes": 8 << 30,
	}
	if MinimumAt(f, 2) == "" {
		t.Fatal("version 2 judges MemTotal alone")
	}
	if why := MinimumAt(f, 3); why != "" {
		t.Fatalf("version 3, 8 GiB installed: %s", why)
	}
	delete(f, "mem_installed_bytes")
	if MinimumAt(f, 3) == "" {
		t.Fatal("no installed figure: MemTotal decides")
	}
	f["mem_installed_bytes"] = 4 << 30
	if MinimumAt(f, 3) == "" {
		t.Fatal("installed below MemTotal never helps")
	}
}
