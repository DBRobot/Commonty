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
	u := filepath.Join(d, "usage-code.prom")
	_ = os.WriteFile(u, []byte("dd_usage{service=\"code\",member=\"tom\"} 2048\ndd_usage{service=\"code\",member=\"bad name\"} 1\ndd_usage{service=\"../x\",member=\"tom\"} 1\n"), 0o644)
	f := promFacts(u)
	if f["usage/code/tom"] != 2048 || len(f) != 1 {
		t.Fatalf("facts: %v", f)
	}
}
