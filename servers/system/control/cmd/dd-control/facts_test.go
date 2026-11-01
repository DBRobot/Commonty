package main

import (
	"os"
	"path/filepath"
	"testing"
)

func TestTheHeartbeatCarriesWhatTheFleetPagesShow(t *testing.T) {
	d := t.TempDir()
	agent := filepath.Join(d, "dd_agent.prom")
	backup := filepath.Join(d, "backup.prom")
	_ = os.WriteFile(agent, []byte("# HELP dd_agent_counter x\n# TYPE dd_agent_counter gauge\ndd_agent_counter{box=\"node1\"} 200\ndd_agent_info{box=\"node1\",path=\"/nix/store/x\",result=\"refused\"} 1\n"), 0o644)
	_ = os.WriteFile(backup, []byte("dd_backup_last_success_seconds{box=\"node1\"} 1791100523\n"), 0o644)
	f := promFacts(agent, backup, filepath.Join(d, "missing.prom"))
	if f["release"] != 200 || f["agent_ok"] != 0 || f["backup_last"] != 1791100523 {
		t.Fatalf("facts: %v", f)
	}
}

func TestUsageFactsAreNamedAndBounded(t *testing.T) {
	d := t.TempDir()
	u := filepath.Join(d, "usage-code.facts")
	_ = os.WriteFile(u, []byte("# x\nusage/code/tom 2048\nusage/code/bad name 1\nusage/../x/tom 1\nusage/code/a/b 1\nother/code/tom 1\n"), 0o644)
	f := usageFacts(u)
	if f["usage/code/tom"] != 2048 || len(f) != 1 {
		t.Fatalf("facts: %v", f)
	}
}

func TestUsageIsNotAMetric(t *testing.T) {
	d := t.TempDir()
	old := filepath.Join(d, "usage-code.prom")
	_ = os.WriteFile(old, []byte("dd_usage{service=\"code\",member=\"tom\"} 2048\n"), 0o644)
	if f := promFacts(old); len(f) != 0 {
		t.Fatalf("a metric line became a fact: %v", f)
	}
}

func TestTheHeartbeatCarriesTheLine(t *testing.T) {
	d := t.TempDir()
	n := filepath.Join(d, "net.prom")
	_ = os.WriteFile(n, []byte("dd_net_metered 0\ndd_net_down_mbps 119.5\ndd_net_up_mbps 58.4\ndd_net_rtt_ms 4.3\n"), 0o644)
	f := promFacts(n)
	if f["net_metered"] != 0 || f["net_down_mbps"] != 119.5 || f["net_up_mbps"] != 58.4 || f["net_rtt_ms"] != 4.3 {
		t.Fatalf("facts: %v", f)
	}
}

func TestTheHeartbeatCarriesWhatEachServiceHolds(t *testing.T) {
	d := t.TempDir()
	n := filepath.Join(d, "services.prom")
	_ = os.WriteFile(n, []byte("dd_service_memory_bytes{service=\"forgejo\"} 524288000\ndd_service_memory_bytes{service=\"bad name\"} 1\ndd_service_memory_bytes{service=\"x/y\"} 1\n"), 0o644)
	f := promFacts(n)
	if f["svc/forgejo/mem"] != 524288000 || len(f) != 1 {
		t.Fatalf("facts: %v", f)
	}
}

func TestTheStoreSaysItsRoomAndWhatItKeeps(t *testing.T) {
	d := t.TempDir()
	st := filepath.Join(d, "store.prom")
	_ = os.WriteFile(st, []byte("dd_store_size_bytes 1000\ndd_store_free_bytes 400\ndd_store_bucket_bytes{bucket=\"ente\"} 300\ndd_store_bucket_bytes{bucket=\"../x\"} 1\ndd_store_lost{bucket=\"ente\",box=\"node2\"} 1\ndd_store_lost{bucket=\"ente\",box=\"a/b\"} 1\n"), 0o644)
	f := promFacts(st)
	if f["store_size_bytes"] != 1000 || f["store_free_bytes"] != 400 || f["store/ente/bytes"] != 300 || f["store/ente/lost/node2"] != 1 || len(f) != 4 {
		t.Fatalf("facts: %v", f)
	}
}

func TestTheHeartbeatCarriesWhatTheBoxCanRunAndWhetherItDrains(t *testing.T) {
	d := t.TempDir()
	c := filepath.Join(d, "can.prom")
	dr := filepath.Join(d, "draining.prom")
	other := filepath.Join(d, "computer.prom")
	_ = os.WriteFile(c, []byte("dd_can_live 1\ndd_can{service=\"photos\"} 1.25\ndd_can{service=\"bad name\"} 1\n"), 0o644)
	_ = os.WriteFile(dr, []byte("dd_box_draining 1\n"), 0o644)
	_ = os.WriteFile(other, []byte("dd_box_draining 0\n"), 0o644)
	f := promFacts(c, dr, other)
	if f["can_live"] != 1 || f["can/photos"] != 1.25 || f["draining"] != 1 || len(f) != 3 {
		t.Fatalf("facts: %v", f)
	}
}

func TestTheHeartbeatCarriesTheRoomForComputers(t *testing.T) {
	d := t.TempDir()
	c := filepath.Join(d, "computers.prom")
	_ = os.WriteFile(c, []byte("dd_computer_max_mem_bytes 8589934592\ndd_computer_max_threads 7\ndd_computer_max_disk_bytes 107374182400\ndd_computer_used_bytes{computer=\"0123456789abcdef\"} 4096\ndd_computer_used_bytes{computer=\"../x\"} 1\n"), 0o644)
	f := promFacts(c)
	if f["computer_max_mem_bytes"] != 8589934592 || f["computer_max_threads"] != 7 || f["computer_max_disk_bytes"] != 107374182400 || f["computer/0123456789abcdef/used_bytes"] != 4096 || len(f) != 4 {
		t.Fatalf("facts: %v", f)
	}
}

func TestOnlyJoinedTaggedNodesGiveAddresses(t *testing.T) {
	// headscale 0.28's `nodes list -o json`, trimmed: a tagged node's user
	// is tagged-devices; who it was minted for is its pre-auth key's user
	list := []byte(`[
	  {"id": 3, "name": "x", "user": {"id": 2147455555, "name": "tagged-devices"}, "pre_auth_key": {"user": {"name": "joined-j0123456789"}, "acl_tags": ["tag:joined"]},
	   "ip_addresses": ["100.64.0.9", "fd7a::9"], "tags": ["tag:joined"]},
	  {"id": 4, "name": "j9999999999", "user": {"name": "tagged-devices"}, "pre_auth_key": {"user": {"name": "joined-j9999999999"}}, "ip_addresses": ["100.64.0.10"], "tags": ["tag:box"]},
	  {"id": 5, "name": "j0123456789", "user": {"name": "tagged-devices"}, "pre_auth_key": {"user": {"name": "boxes"}}, "ip_addresses": ["100.64.0.11"], "tags": ["tag:joined"]},
	  {"id": 6, "name": "j1111111111", "user": {"name": "tagged-devices"}, "ip_addresses": ["100.64.0.13"], "tags": ["tag:joined"]},
	  {"id": 7, "name": "y", "user": {"name": "tagged-devices"}, "pre_auth_key": {"user": {"name": "joined-J01234567"}}, "ip_addresses": ["100.64.0.12"], "tags": ["tag:joined"]},
	  {"id": 8, "name": "z", "user": {"name": "tagged-devices"}, "pre_auth_key": {"user": {"name": "joined-jaaaaaaaaaa"}}, "ip_addresses": ["100.64.0.20"], "tags": ["tag:joined"]},
	  {"id": 2, "name": "z", "user": {"name": "tagged-devices"}, "pre_auth_key": {"user": {"name": "joined-jaaaaaaaaaa"}}, "ip_addresses": ["100.64.0.2"], "tags": ["tag:joined"]}
	]`)
	got := joinedAddrs(list)
	if len(got) != 2 || got["j0123456789"] != 100<<24|64<<16|9 || got["jaaaaaaaaaa"] != 100<<24|64<<16|20 {
		t.Fatalf("%v", got)
	}
	// headscale before 0.28: the node's own user, camelCase
	old := []byte(`[{"user": {"name": "joined-j0123456789"}, "ipAddresses": ["100.64.0.9"], "validTags": ["tag:joined"]}]`)
	if got := joinedAddrs(old); got["j0123456789"] != 100<<24|64<<16|9 {
		t.Fatalf("%v", got)
	}
}

func TestWhatABoxQualifiesByComesFromItsOwnReport(t *testing.T) {
	d := t.TempDir()
	r, m := filepath.Join(d, "hardware.json"), filepath.Join(d, "model")
	_ = os.WriteFile(r, []byte(`{"firmware":{"uefi":true,"secureBootCapable":true,"tpm":{"version":2}},"cpu":{"threads":12},"memoryBytes":16000000000,
	  "bootDisk":"/dev/disk/by-id/nvme-a","disks":[{"id":"/dev/disk/by-id/ata-b","bytes":4e12},{"id":"/dev/disk/by-id/nvme-a","bytes":256e9}],
	  "nics":[{"name":"eno1"},{"name":"wlan0"}],
	  "gpus":[{"driver":"i915","encode":["H264Main","HEVCMain"]},{"driver":"nvidia","vram":12884901888,"encode":[]},{"driver":"amdgpu","vram":4294967296}]}`), 0o644)
	f := reportFacts(r, m)
	if f["uefi"] != 1 || f["secure_boot_capable"] != 1 || f["tpm_major"] != 2 || f["gpus"] != 2 || f["raspberry_pi"] != 0 ||
		f["system_disk_bytes"] != 256e9 || f["threads"] != 12 || f["mem_total_bytes"] != 16e9 || f["nics"] != 2 || f["gpu_vram_bytes"] != 12884901888 || f["video_encoders"] != 1 {
		t.Fatalf("facts: %v", f)
	}
	// a report from before secureBootCapable: Secure Boot on, or in setup mode, can do it
	_ = os.WriteFile(r, []byte(`{"firmware":{"uefi":true,"secureBoot":false,"setupMode":true}}`), 0o644)
	if f := reportFacts(r, m); f["secure_boot_capable"] != 1 || f["system_disk_bytes"] != 0 {
		t.Fatalf("facts: %v", f)
	}
	_ = os.WriteFile(r, []byte(`{"firmware":{"uefi":false,"tpm":null},"gpus":[{"driver":"v3d"}]}`), 0o644)
	_ = os.WriteFile(m, []byte("Raspberry Pi 5 Model B Rev 1.0\x00"), 0o644)
	f = reportFacts(r, m)
	if f["uefi"] != 0 || f["secure_boot_capable"] != 0 || f["tpm_major"] != 0 || f["gpus"] != 0 || f["video_encoders"] != 0 || f["raspberry_pi"] != 1 {
		t.Fatalf("facts: %v", f)
	}
	if f := reportFacts(filepath.Join(d, "none"), filepath.Join(d, "none")); len(f) != 0 {
		t.Fatalf("facts: %v", f)
	}
}

func TestOnlyAReleaseThatFailedIsAgentOkZero(t *testing.T) {
	d := t.TempDir()
	agent := filepath.Join(d, "dd_agent.prom")
	for result, want := range map[string]float64{"ok": 1, "refused": 0, "rollback": 0, "held": -1, "none": -1, "absent": -1, "": -1} {
		_ = os.WriteFile(agent, []byte("dd_agent_info{box=\"j0123456789\",path=\"/nix/store/x\",result=\""+result+"\"} 1\n"), 0o644)
		v, said := promFacts(agent)["agent_ok"]
		// -1: no word on how a release went, which is never "our fault"
		if want < 0 && said || want >= 0 && (!said || v != want) {
			t.Fatalf("result %q: agent_ok %v (said %v), want %v", result, v, said, want)
		}
	}
}

func TestAGuardedCopyHeldBackIsAFact(t *testing.T) {
	f := filepath.Join(t.TempDir(), "store-copy.prom")
	_ = os.WriteFile(f, []byte("dd_store_copy_skipped{bucket=\"backups-node1\",box=\"node2\"} 1\n"), 0o644)
	if got := promFacts(f); got["store/backups-node1/skipped/node2"] != 1 {
		t.Fatalf("facts: %v", got)
	}
}
