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
