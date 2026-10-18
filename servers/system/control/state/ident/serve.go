package ident

// /identity, the same on every box (dd-control) and at the relay (the
// witness): the view at the agreed state's time, one member or all, and
// 304 for a reader whose copy is still current.

import (
	"crypto/sha256"
	"encoding/json"
	"net/http"
	"sync"
)

// Server answers /identity from a replayer over the agreed log
type Server struct {
	cfg  *Config
	r    *Replayer
	mu   sync.Mutex
	last [32]byte
	// the height the answer last changed at
	changed uint64
}

func NewServer(cfg *Config, voters []string) *Server {
	return &Server{cfg: cfg, r: NewReplayer(cfg, voters)}
}

// Answer: the log is n items, item(i) each; height and time (ms) the agreed
// state's. name, if not empty, asks for that member alone; since, if not 0,
// is the `changed` the reader last saw.
func (sv *Server) Answer(n int, item func(int) Item, height uint64, timeMs int64, name string, since uint64) (int, []byte) {
	// one lock over judging and reading: CatchN moves the replayer's state,
	// and a second request judging while this one reads it was a data race
	sv.mu.Lock()
	defer sv.mu.Unlock()
	s := sv.r.CatchN(n, item)
	v := s.View(sv.cfg, height, timeMs, nil)
	// what changed is everything but where and when it was asked
	v.Height, v.Time = 0, 0
	b, _ := json.Marshal(v)
	if h := sha256.Sum256(b); h != sv.last || sv.changed == 0 {
		sv.last, sv.changed = h, height
	}
	v.Height, v.Time, v.Changed = height, timeMs, sv.changed
	if since != 0 && since >= sv.changed {
		return http.StatusNotModified, nil
	}
	if name != "" {
		m := v.Members[name]
		if m == nil {
			return http.StatusNotFound, []byte(`{"error":"no such name"}`)
		}
		b, _ = json.Marshal(map[string]any{"height": height, "time": timeMs, "changed": sv.changed, "name": name, "member": m})
		return http.StatusOK, b
	}
	b, _ = json.Marshal(v)
	return http.StatusOK, b
}
