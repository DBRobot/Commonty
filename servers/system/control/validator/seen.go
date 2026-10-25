package validator

import (
	"sync"

	bft "github.com/hyperledger/SmartBFT/pkg/types"
)

// when this validator first saw each request, unix ms: a box sends every
// request to all the validators, so this is soon after it was sent, and
// never before. Two generations, swapped every seenFor, so an entry lives
// seenFor to twice that; past maxSeen nothing more is kept, and a request
// not kept counts as first seen when it is looked up
type seenAt struct {
	mu        sync.Mutex
	cur, old  map[bft.RequestInfo]int64
	startedAt int64
}

const (
	seenFor = 120_000
	maxSeen = 1 << 16
)

func (s *seenAt) rotate(now int64) {
	if s.cur == nil {
		s.cur, s.startedAt = map[bft.RequestInfo]int64{}, now
	}
	if now-s.startedAt >= seenFor {
		s.old, s.cur, s.startedAt = s.cur, map[bft.RequestInfo]int64{}, now
	}
}

func (s *seenAt) saw(id bft.RequestInfo, now int64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.rotate(now)
	if _, ok := s.cur[id]; ok {
		return
	}
	if t, ok := s.old[id]; ok {
		s.cur[id] = t
		return
	}
	if len(s.cur) < maxSeen {
		s.cur[id] = now
	}
}

func (s *seenAt) first(id bft.RequestInfo, now int64) int64 {
	s.mu.Lock()
	defer s.mu.Unlock()
	if t, ok := s.cur[id]; ok {
		return t
	}
	if t, ok := s.old[id]; ok {
		return t
	}
	return now
}
