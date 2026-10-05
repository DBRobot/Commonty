//go:build !(linux && !android) && !windows

package main

import "net"

// sameAccount where there is no account table to read: a phone, where the
// browser's way in is never opened (the phone has its VPN), and systems
// the app is not built for. Closed, so a build there cannot open it by
// mistake.
func sameAccount(client, server *net.TCPAddr) bool { return false }
