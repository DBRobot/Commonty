//go:build linux && !android

package main

import (
	"io"
	"net"
	"strings"
	"testing"
	"time"
)

// a /proc/net/tcp as the kernel writes it: the listener, the server's end
// of an accepted connection (uid of whoever runs the app) and the client's
// end (uid of whoever connected)
const table = `  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:A2E2 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 11 1 0 100 0 0 10 0
   1: 0100007F:A2E2 0100007F:C350 01 00000000:00000000 00:00000000 00000000  1000        0 12 1 0 20 4 30 10 -1
   2: 0100007F:C350 0100007F:A2E2 01 00000000:00000000 00:00000000 00000000  1001        0 13 1 0 20 4 30 10 -1
`

func TestOwnerIsTheClientsEnd(t *testing.T) {
	lo := net.ParseIP("127.0.0.1")
	client := &net.TCPAddr{IP: lo, Port: 0xC350}
	server := &net.TCPAddr{IP: lo, Port: 0xA2E2}
	uid, ok := ownerOf(table, client, server)
	if !ok || uid != 1001 {
		t.Fatalf("owner %d %v, want 1001 (the client's end, not the server's)", uid, ok)
	}
	if _, ok := ownerOf(table, &net.TCPAddr{IP: lo, Port: 1}, server); ok {
		t.Fatal("a connection that is not in the table has an owner")
	}
}

// the listener answers this account: a connection from this process gets
// through, and a byte sent comes back
func TestThisAccountGetsIn(t *testing.T) {
	l, err := listenOwn()
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	go func() {
		c, err := l.Accept()
		if err == nil {
			_, _ = io.Copy(c, c)
			c.Close()
		}
	}()
	c, err := net.Dial("tcp", l.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer c.Close()
	_ = c.SetDeadline(time.Now().Add(5 * time.Second))
	if _, err := c.Write([]byte("x")); err != nil {
		t.Fatal(err)
	}
	b := make([]byte, 1)
	if _, err := io.ReadFull(c, b); err != nil {
		t.Fatalf("this account was turned away: %v", err)
	}
}

func TestRulesNameThisRunsPort(t *testing.T) {
	r := pacFile("commonty.org", 51234)
	if !strings.Contains(r, "SOCKS5 127.0.0.1:51234") || !strings.Contains(r, `dnsDomainIs(host, ".commonty.org")`) {
		t.Fatal(r)
	}
}
