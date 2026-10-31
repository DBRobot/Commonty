//go:build linux && !android

package main

import (
	"context"
	"encoding/json"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"tailscale.com/ipn"
)

// A real Headscale (on PATH: net-check puts it there) deletes this engine's
// node, as it lost the owner's laptop on 2026-10-26: the engine says it is
// gone and not running, a resume from its state says so at once rather
// than waiting out its minute, and a fresh key on a cleared state joins
// again.
func TestControlForgetsTheNode(t *testing.T) {
	if _, err := exec.LookPath("headscale"); err != nil {
		t.Skip("no headscale on PATH")
	}
	hs := startHeadscale(t)
	state := filepath.Join(t.TempDir(), "net")

	st := start(state, hs.url, hs.key(t), "laptop", -1)
	if st.Error != "" {
		t.Fatalf("join: %s", st.Error)
	}
	t.Cleanup(stop)
	if s := currentStatus(); !s.Running || s.Gone {
		t.Fatalf("after joining: %+v", s)
	}

	hs.run(t, "nodes", "delete", "--identifier", hs.nodeID(t, "laptop"), "--force")
	// Headscale keeps the open long poll going; what comes back refused is
	// the next update the engine sends, as a device does when its network
	// changes (the laptop's went every 30 s, retried). Here a new hostinfo.
	lc, err := srv.LocalClient()
	if err != nil {
		t.Fatal(err)
	}
	if _, err := lc.EditPrefs(context.Background(), &ipn.MaskedPrefs{HostnameSet: true, Prefs: ipn.Prefs{Hostname: "laptop-moved"}}); err != nil {
		t.Fatal(err)
	}
	deadline := time.Now().Add(60 * time.Second)
	for !currentStatus().Gone {
		if time.Now().After(deadline) {
			t.Fatalf("the engine never noticed its node was deleted: %+v", currentStatus())
		}
		time.Sleep(250 * time.Millisecond)
	}
	if s := currentStatus(); s.Running || s.Error == "" {
		t.Fatalf("a forgotten node reads as running: %+v", s)
	}
	stop()

	// resuming: the engine may come up on the netmap it cached and hear
	// from control after, or ask control first and be refused; either way
	// it says so, and soon
	t0 := time.Now()
	st = start(state, hs.url, "", "laptop", -1)
	for !st.Gone && !currentStatus().Gone {
		if time.Since(t0) > 30*time.Second {
			t.Fatalf("resuming a forgotten node: %+v, %+v", st, currentStatus())
		}
		time.Sleep(250 * time.Millisecond)
	}
	if s := currentStatus(); s.Running {
		t.Fatalf("a resumed forgotten node reads as running: %+v", s)
	}
	stop()

	// what the app does then: the state cleared, a new key from the gate
	if err := os.Remove(filepath.Join(state, "tailscaled.state")); err != nil {
		t.Fatal(err)
	}
	st = start(state, hs.url, hs.key(t), "laptop", -1)
	if st.Error != "" || st.Gone {
		t.Fatalf("joining again: %+v", st)
	}
	if s := currentStatus(); !s.Running || s.Gone {
		t.Fatalf("after joining again: %+v", s)
	}
}

type headscale struct {
	url, config, user string
}

func freePort(t *testing.T) int {
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	return l.Addr().(*net.TCPAddr).Port
}

func startHeadscale(t *testing.T) *headscale {
	dir := t.TempDir()
	port := freePort(t)
	h := &headscale{url: fmt.Sprintf("http://127.0.0.1:%d", port), config: filepath.Join(dir, "config.yaml"), user: "tom"}
	conf := fmt.Sprintf(`server_url: %s
listen_addr: 127.0.0.1:%d
metrics_listen_addr: 127.0.0.1:%d
grpc_listen_addr: 127.0.0.1:%d
grpc_allow_insecure: true
noise:
  private_key_path: %s/noise.key
prefixes:
  v4: 100.64.0.0/10
  v6: fd7a:115c:a1e0::/48
  allocation: sequential
derp:
  server:
    enabled: true
    region_id: 999
    region_code: test
    region_name: Test
    stun_listen_addr: 127.0.0.1:%d
    private_key_path: %s/derp.key
    automatically_add_embedded_derp_region: true
  urls: []
  paths: []
  auto_update_enabled: false
disable_check_updates: true
database:
  type: sqlite
  sqlite:
    path: %s/db.sqlite
dns:
  magic_dns: false
  override_local_dns: false
  base_domain: net.test.invalid
unix_socket: %s/headscale.sock
unix_socket_permission: "0770"
log:
  level: warn
`, h.url, port, freePort(t), freePort(t), dir, freePort(t), dir, dir, dir)
	if err := os.WriteFile(h.config, []byte(conf), 0o600); err != nil {
		t.Fatal(err)
	}
	cmd := exec.Command("headscale", "-c", h.config, "serve")
	cmd.Stdout, cmd.Stderr = os.Stderr, os.Stderr
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = cmd.Process.Kill(); _ = cmd.Wait() })
	for i := 0; ; i++ {
		if c, err := net.Dial("unix", filepath.Join(dir, "headscale.sock")); err == nil {
			c.Close()
			break
		}
		if i > 200 {
			t.Fatal("headscale never came up")
		}
		time.Sleep(100 * time.Millisecond)
	}
	h.run(t, "users", "create", h.user)
	return h
}

func (h *headscale) run(t *testing.T, args ...string) string {
	t.Helper()
	out, err := exec.Command("headscale", append([]string{"-c", h.config, "-o", "json"}, args...)...).CombinedOutput()
	if err != nil {
		t.Fatalf("headscale %s: %v: %s", strings.Join(args, " "), err, out)
	}
	return string(out)
}

func (h *headscale) key(t *testing.T) string {
	var users []struct {
		ID   json.Number `json:"id"`
		Name string      `json:"name"`
	}
	if err := json.Unmarshal([]byte(h.run(t, "users", "list")), &users); err != nil {
		t.Fatal(err)
	}
	for _, u := range users {
		if u.Name == h.user {
			var k struct {
				Key string `json:"key"`
			}
			out := h.run(t, "preauthkeys", "create", "--user", u.ID.String(), "--expiration", "10m")
			if err := json.Unmarshal([]byte(out), &k); err != nil {
				t.Fatalf("%v: %s", err, out)
			}
			return k.Key
		}
	}
	t.Fatal("no user")
	return ""
}

func (h *headscale) nodeID(t *testing.T, name string) string {
	var nodes []struct {
		ID   json.Number `json:"id"`
		Name string      `json:"name"`
	}
	if err := json.Unmarshal([]byte(h.run(t, "nodes", "list")), &nodes); err != nil {
		t.Fatal(err)
	}
	for _, n := range nodes {
		if n.Name == name {
			return n.ID.String()
		}
	}
	t.Fatalf("no node %s", name)
	return ""
}
