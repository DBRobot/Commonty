// The browser's way in. The engine's own proxy wants a password, which no
// browser can be given; this one wants none, listens on loopback only, and
// reaches the fleet's names and nothing else, so it is no way out to the
// internet for anything on the machine. Beside it, the proxy rules a
// browser reads (a PAC file): the fleet's names to this proxy, everything
// else straight out as before. The app registers the rules with the system
// (app/src/browser.rs); while the app is closed nothing answers here and a
// browser goes direct.
package main

import (
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"strings"

	"tailscale.com/net/socks5"
	"tailscale.com/tsnet"
)

// fixed, so the rules the system holds stay right across restarts
const (
	pacPort   = 41650
	socksPort = 41651
)

type browserDoor struct {
	socks, pac net.Listener
	domain     string
}

func (d *browserDoor) url() string {
	return fmt.Sprintf("http://127.0.0.1:%d/proxy.pac", pacPort)
}

func (d *browserDoor) close() {
	d.socks.Close()
	d.pac.Close()
}

func ours(host, domain string) bool {
	host = strings.ToLower(strings.TrimSuffix(host, "."))
	return host == domain || strings.HasSuffix(host, "."+domain)
}

func pacFile(domain string) string {
	return fmt.Sprintf(`function FindProxyForURL(url, host) {
  host = host.toLowerCase();
  if (host == %q || dnsDomainIs(host, %q))
    return "SOCKS5 127.0.0.1:%d; SOCKS 127.0.0.1:%d";
  return "DIRECT";
}
`, domain, "."+domain, socksPort, socksPort)
}

func newBrowserDoor(s *tsnet.Server, domain string) (*browserDoor, error) {
	domain = strings.ToLower(strings.TrimSuffix(domain, "."))
	if domain == "" {
		return nil, errors.New("no domain")
	}
	lc, err := s.LocalClient()
	if err != nil {
		return nil, err
	}
	sl, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", socksPort))
	if err != nil {
		return nil, err
	}
	pl, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", pacPort))
	if err != nil {
		sl.Close()
		return nil, err
	}
	srv := &socks5.Server{
		Logf: func(string, ...any) {},
		Dialer: func(ctx context.Context, network, addr string) (net.Conn, error) {
			host, port, err := net.SplitHostPort(addr)
			if err != nil {
				return nil, err
			}
			// by name, and only the fleet's names: an address could be
			// anything, and this proxy goes nowhere but the network
			if net.ParseIP(host) != nil || !ours(host, domain) {
				return nil, fmt.Errorf("%s is not on the network", host)
			}
			ip, err := resolve(ctx, lc, host)
			if err != nil {
				return nil, err
			}
			return s.Dial(ctx, network, net.JoinHostPort(ip, port))
		},
	}
	go func() { _ = srv.Serve(sl) }()
	pac := pacFile(domain)
	go func() {
		_ = http.Serve(pl, http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			w.Header().Set("Content-Type", "application/x-ns-proxy-autoconfig")
			w.Header().Set("Cache-Control", "no-store")
			_, _ = w.Write([]byte(pac))
		}))
	}()
	return &browserDoor{socks: sl, pac: pl, domain: domain}, nil
}
