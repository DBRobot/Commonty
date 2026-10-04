// Package wire is how everything crosses the relay: one frame, addressed to
// one node or to many, from one node. The relay reads only the address.
package wire

import (
	"encoding/binary"
	"fmt"
)

// Addresses: a validator's or box's id, or one of these. Kept under 2^53,
// where a JavaScript number (the relay) still holds them exactly.
const (
	ToValidators uint64 = 1 << 50
	ToAll        uint64 = 1<<50 + 1
)

// Hello is what a connection signs to say who it is: its address and the
// time, so a recorded one is soon worthless.
func Hello(id uint64, at int64) []byte {
	return []byte(fmt.Sprintf("commonty relay v1\x00%d\x00%d", id, at))
}

// Kinds of frame.
const (
	Consensus byte = 1 // a SmartBFT message, validator to validator
	Forward   byte = 2 // a request a validator passes to the leader
	Submit    byte = 3 // a box's signed request, to the validators
	SyncAsk   byte = 4 // "send me your latest", from anyone
	SyncState byte = 5 // the answer: a state and the decision after it
)

const header = 17

func Frame(to, from uint64, kind byte, payload []byte) []byte {
	b := make([]byte, header+len(payload))
	binary.BigEndian.PutUint64(b[0:], to)
	binary.BigEndian.PutUint64(b[8:], from)
	b[16] = kind
	copy(b[header:], payload)
	return b
}

func Parse(b []byte) (to, from uint64, kind byte, payload []byte, ok bool) {
	if len(b) < header {
		return 0, 0, 0, nil, false
	}
	return binary.BigEndian.Uint64(b[0:]), binary.BigEndian.Uint64(b[8:]), b[16], b[header:], true
}
