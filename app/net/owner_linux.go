//go:build linux && !android

package main

import (
	"encoding/hex"
	"fmt"
	"net"
	"os"
	"strconv"
	"strings"
)

// sameAccount: the kernel's table of tcp connections names the account
// that opened each one; the client's end of this connection is the row
// whose local address is the client and whose remote is this listener
func sameAccount(client, server *net.TCPAddr) bool {
	table, err := os.ReadFile("/proc/net/tcp")
	if err != nil {
		return false
	}
	uid, ok := ownerOf(string(table), client, server)
	return ok && uid == os.Getuid()
}

// ownerOf finds the row for the connection from client to server in a
// /proc/net/tcp table and returns the uid that owns it
func ownerOf(table string, client, server *net.TCPAddr) (int, bool) {
	want := procAddr(client) + " " + procAddr(server)
	for _, line := range strings.Split(table, "\n")[1:] {
		f := strings.Fields(line)
		if len(f) < 8 || f[1]+" "+f[2] != want {
			continue
		}
		uid, err := strconv.Atoi(f[7])
		return uid, err == nil
	}
	return 0, false
}

// procAddr writes an ipv4 address as /proc/net/tcp does: the four bytes
// in the machine's order (reversed, on every machine this runs on), a
// colon, the port in hex
func procAddr(a *net.TCPAddr) string {
	ip := a.IP.To4()
	if ip == nil {
		return ""
	}
	b := []byte{ip[3], ip[2], ip[1], ip[0]}
	return strings.ToUpper(hex.EncodeToString(b)) + fmt.Sprintf(":%04X", a.Port)
}
