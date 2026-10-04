// Package wire is how everything crosses the relay: one frame, addressed to
// one node or to many, from one node. The relay reads only the address.
package wire

import "encoding/binary"

// Addresses: a validator's id, or one of these.
const (
	ToValidators uint64 = 1 << 62
	ToAll        uint64 = 1<<62 + 1
)

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
