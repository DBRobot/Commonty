// The fleet's network engine for the app: Tailscale's tsnet behind a C
// interface of a few calls. Nothing here knows what Commonty is; it takes
// a control server, a key and a state directory, joins, and hands back a
// loopback proxy that routes into the network and resolves its names.
// The Rust side does everything else through that proxy.
package main

/*
#include <stdlib.h>
*/
import "C"

import (
	"context"
	"encoding/json"
	"log"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"time"
	"unsafe"

	"github.com/tailscale/wireguard-go/tun"
	"tailscale.com/tsnet"

	"commonty.org/app/net/bridge"
)

var (
	mu      sync.Mutex
	srv     *tsnet.Server
	door    *bridge.Bridge
	proxy   *proxyServer
	browser *browserDoor
)

type started struct {
	Proxy      string `json:"proxy"`
	Credential string `json:"credential"`
	IP         string `json:"ip"`
	Error      string `json:"error,omitempty"`
}

type peer struct {
	Name   string `json:"name"`
	IP     string `json:"ip"`
	Online bool   `json:"online"`
}

type status struct {
	Running bool   `json:"running"`
	State   string `json:"state"`
	IP      string `json:"ip"`
	Name    string `json:"name"`
	Peers   []peer `json:"peers"`
	Error   string `json:"error,omitempty"`
}

func reply(v any) *C.char {
	b, _ := json.Marshal(v)
	return C.CString(string(b))
}

// commonty_net_start joins the network (or resumes from the state in dir
// when key is empty) and returns json: the SOCKS5 proxy's address and
// password, this node's address. tunfd, when not -1, is the phone's VPN
// interface: the whole device's traffic for the network comes through it.
// Idempotent while running.
//
//export commonty_net_start
func commonty_net_start(dir, control, key, hostname *C.char, tunfd C.int) *C.char {
	mu.Lock()
	defer mu.Unlock()
	if srv != nil {
		return reply(started{Proxy: proxy.addr(), Credential: proxy.cred, IP: ipOf(srv)})
	}
	// the engine's log policy wants a place of its own (a phone has no
	// home; Go reads its environment once, so this is set here, not by
	// the caller), and nothing of ours is sent to Tailscale
	state := C.GoString(dir)
	logs := filepath.Join(state, "logs")
	_ = os.MkdirAll(logs, 0o700)
	// the node's key lives here: nobody else on the machine lists it
	_ = os.Chmod(state, 0o700)
	_ = os.Setenv("TS_LOGS_DIR", logs)
	_ = os.Setenv("TS_NO_LOGS_NO_SUPPORT", "true")
	// the control server is reached through the bridge: the front door
	// carries websockets, not the engine's own upgrade
	b, err := bridge.New(C.GoString(control), "127.0.0.1:0")
	if err != nil {
		return reply(started{Error: err.Error()})
	}
	// quiet unless asked: the engine's log is a firehose
	logf := func(string, ...any) {}
	if os.Getenv("COMMONTY_NET_DEBUG") != "" || runtime.GOOS == "android" {
		logf = log.Printf
	}
	var dev tun.Device
	if tunfd >= 0 {
		d, err := tunFromFD(int(tunfd))
		if err != nil {
			b.Close()
			return reply(started{Error: err.Error()})
		}
		dev = d
	}
	s := &tsnet.Server{
		Tun:        dev,
		Dir:        state,
		ControlURL: b.URL(),
		AuthKey:    C.GoString(key),
		Hostname:   C.GoString(hostname),
		Ephemeral:  false,
		Logf:       logf,
		UserLogf:   logf,
	}
	ctx, cancel := context.WithTimeout(context.Background(), 60*time.Second)
	defer cancel()
	if _, err := s.Up(ctx); err != nil {
		s.Close()
		b.Close()
		return reply(started{Error: err.Error()})
	}
	p, err := newProxy(s)
	if err != nil {
		s.Close()
		b.Close()
		return reply(started{Error: err.Error()})
	}
	srv = s
	door = b
	proxy = p
	return reply(started{Proxy: p.addr(), Credential: p.cred, IP: ipOf(s)})
}

func ipOf(s *tsnet.Server) string {
	v4, _ := s.TailscaleIPs()
	if v4.IsValid() {
		return v4.String()
	}
	return ""
}

// commonty_net_status returns json: whether the node is up, its address
// and name, the peers it can see.
//
//export commonty_net_status
func commonty_net_status() *C.char {
	mu.Lock()
	defer mu.Unlock()
	if srv == nil {
		return reply(status{State: "stopped"})
	}
	lc, err := srv.LocalClient()
	if err != nil {
		return reply(status{Error: err.Error()})
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	st, err := lc.Status(ctx)
	if err != nil {
		return reply(status{Error: err.Error()})
	}
	out := status{Running: st.BackendState == "Running", State: st.BackendState, IP: ipOf(srv)}
	if st.Self != nil {
		out.Name = st.Self.HostName
	}
	for _, p := range st.Peer {
		ip := ""
		if len(p.TailscaleIPs) > 0 {
			ip = p.TailscaleIPs[0].String()
		}
		out.Peers = append(out.Peers, peer{Name: p.HostName, IP: ip, Online: p.Online})
	}
	return reply(out)
}

// commonty_net_browser opens the browser's way in (browser.go) for the
// fleet's names under domain and returns json: the proxy rules' url, and
// the rules themselves for a system that should read them from a file.
// Idempotent while running.
//
//export commonty_net_browser
func commonty_net_browser(domain *C.char) *C.char {
	mu.Lock()
	defer mu.Unlock()
	type out struct {
		Pac   string `json:"pac"`
		Rules string `json:"rules"`
		Error string `json:"error,omitempty"`
	}
	if srv == nil {
		return reply(out{Error: "not on the network"})
	}
	if browser == nil {
		b, err := newBrowserDoor(srv, C.GoString(domain))
		if err != nil {
			return reply(out{Error: err.Error()})
		}
		browser = b
	}
	return reply(out{Pac: browser.url(), Rules: pacFile(browser.domain)})
}

// commonty_net_stop leaves the network for this run; the state stays for
// the next start.
//
//export commonty_net_stop
func commonty_net_stop() {
	mu.Lock()
	defer mu.Unlock()
	if browser != nil {
		browser.close()
		browser = nil
	}
	if proxy != nil {
		proxy.close()
		proxy = nil
	}
	if srv != nil {
		srv.Close()
		srv = nil
	}
	if door != nil {
		door.Close()
		door = nil
	}
}

// commonty_net_free releases a string this library returned.
//
//export commonty_net_free
func commonty_net_free(p *C.char) {
	C.free(unsafe.Pointer(p))
}

func main() {}
