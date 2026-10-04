//go:build windows

package main

import (
	"encoding/binary"
	"net"
	"unsafe"

	"golang.org/x/sys/windows"
)

var getExtendedTcpTable = windows.NewLazySystemDLL("iphlpapi.dll").NewProc("GetExtendedTcpTable")

const tcpTableOwnerPidConnections = 4 // TCP_TABLE_OWNER_PID_CONNECTIONS

// sameAccount: Windows' table of tcp connections names the process at
// each end; the client's process is asked who it runs as, and that is
// compared with who this one runs as
func sameAccount(client, server *net.TCPAddr) bool {
	pid, ok := ownerPid(client, server)
	if !ok {
		return false
	}
	h, err := windows.OpenProcess(windows.PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
	if err != nil {
		return false // another account's process is not ours to open
	}
	defer windows.CloseHandle(h)
	var tok windows.Token
	if err := windows.OpenProcessToken(h, windows.TOKEN_QUERY, &tok); err != nil {
		return false
	}
	defer tok.Close()
	theirs, err := tok.GetTokenUser()
	if err != nil {
		return false
	}
	ours, err := windows.GetCurrentProcessToken().GetTokenUser()
	if err != nil {
		return false
	}
	return windows.EqualSid(theirs.User.Sid, ours.User.Sid)
}

// ownerPid finds the row whose local end is the client and whose remote
// end is this listener, and returns the process that owns it
func ownerPid(client, server *net.TCPAddr) (uint32, bool) {
	var size uint32
	_, _, _ = getExtendedTcpTable.Call(0, uintptr(unsafe.Pointer(&size)), 0, windows.AF_INET, tcpTableOwnerPidConnections, 0)
	if size == 0 {
		return 0, false
	}
	buf := make([]byte, size)
	r, _, _ := getExtendedTcpTable.Call(uintptr(unsafe.Pointer(&buf[0])), uintptr(unsafe.Pointer(&size)), 0, windows.AF_INET, tcpTableOwnerPidConnections, 0)
	if r != 0 || len(buf) < 4 {
		return 0, false
	}
	// MIB_TCPTABLE_OWNER_PID: a count, then rows of six uint32s - state,
	// local address, local port, remote address, remote port, owning pid;
	// the ports in network order in the low two bytes
	n := binary.LittleEndian.Uint32(buf[0:4])
	port := func(v uint32) int { return int(v&0xff)<<8 | int(v>>8&0xff) }
	for i := uint32(0); i < n; i++ {
		off := 4 + i*24
		if int(off+24) > len(buf) {
			break
		}
		row := buf[off : off+24]
		lport := port(binary.LittleEndian.Uint32(row[8:12]))
		rport := port(binary.LittleEndian.Uint32(row[16:20]))
		if lport == client.Port && rport == server.Port {
			return binary.LittleEndian.Uint32(row[20:24]), true
		}
	}
	return 0, false
}
