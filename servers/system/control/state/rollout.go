package state

// Staged rollout (docs/dev/rollout.md): which boxes take a release first,
// and the mark that lets the rest follow. A box's agent begins a rollout
// with the signed release file; the validators check it against the
// release key and fix the waves from the agreed state at that moment. Each
// box of an admitted wave says how its release went (took, good, bad), and
// every batch judges the newest wave from those words and the agreed time.
// No box, and not the release key, can pass a release on its own.

import (
	"crypto/ed25519"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"math"
	"sort"
	"strings"

	"commonty/control/state/ident"
)

// RolloutRules come with the release (fleet/control.json); absent, the
// defaults below
type RolloutRules struct {
	// the share of the live boxes each wave brings the total to, the last
	// wave being everyone else: 5%, then 25%, then all
	Waves []float64 `json:"waves,omitempty"`
	// how long each wave soaks, ms; the last given counts for the rest
	SoakMs []int64 `json:"soakMs,omitempty"`
	// how long an admitted wave's boxes have to switch and say so
	TookMs int64 `json:"tookMs,omitempty"`
	// past the soak, how long before a wave not proven stops the rollout
	GraceMs int64 `json:"graceMs,omitempty"`
}

func (r *RolloutRules) withDefaults() RolloutRules {
	out := RolloutRules{Waves: []float64{0.05, 0.25}, SoakMs: []int64{30 * 60_000, 10 * 60_000, 0}, TookMs: 40 * 60_000, GraceMs: 30 * 60_000}
	if r == nil {
		return out
	}
	if len(r.Waves) > 0 {
		out.Waves = r.Waves
	}
	if len(r.SoakMs) > 0 {
		out.SoakMs = r.SoakMs
	}
	if r.TookMs > 0 {
		out.TookMs = r.TookMs
	}
	if r.GraceMs > 0 {
		out.GraceMs = r.GraceMs
	}
	return out
}

// Soak is how long wave k (from 0) soaks
func (r RolloutRules) Soak(k int) int64 {
	if k < len(r.SoakMs) {
		return r.SoakMs[k]
	}
	return r.SoakMs[len(r.SoakMs)-1]
}

// RolloutOp is the body of a "rollout" request
type RolloutOp struct {
	Op string `json:"op"` // begin | verdict
	// begin: the release file exactly as published (current.json)
	Release string `json:"release,omitempty"`
	// verdict: about this release, from the box that sends it
	Counter uint64 `json:"counter,omitempty"`
	Verdict string `json:"verdict,omitempty"` // took | good | bad
	Reason  string `json:"reason,omitempty"`
}

// MaxRollout bounds one request: a release file with every box's
// attestations
const MaxRollout = 128 << 10

type Report struct {
	Took   int64  `json:"took,omitempty"` // agreed ms of its took
	Good   int64  `json:"good,omitempty"` // of its latest good
	Bad    int64  `json:"bad,omitempty"`  // of its bad, which sticks
	Reason string `json:"reason,omitempty"`
}

// Rollout is the newest release's: there is only ever one
type Rollout struct {
	Counter uint64 `json:"counter"`
	// sha256 hex of the release's canonical payload and signature (Digest):
	// what an agent checks it fetched
	Digest string     `json:"digest"`
	Began  int64      `json:"began"`
	Waves  [][]string `json:"waves"`
	// how many waves may take it; the newest admitted is judged
	Admitted int `json:"admitted"`
	// agreed ms the newest wave was admitted, moved on by any gap in
	// agreement, so a deadline never falls in an outage
	Since int64 `json:"since"`
	// the end of the last gap in agreement: a soak counts from here
	GapEnd  int64             `json:"gapEnd,omitempty"`
	Paused  int64             `json:"paused,omitempty"`
	Reports map[string]Report `json:"reports"`
	// soaking | good | done | bad
	Status string `json:"status"`
	Why    string `json:"why,omitempty"`
	// boxes of a later wave that were away all through it: they take the
	// release when they are back
	Excused []string `json:"excused,omitempty"`
}

// WaveOf is the index of box's wave, or -1 when it is in none
func (r *Rollout) WaveOf(box string) int {
	for i, w := range r.Waves {
		if contains(w, box) {
			return i
		}
	}
	return -1
}

// May says whether box may run this release now: its wave admitted and the
// rollout not stopped; a box in no wave, once every wave has passed
func (r *Rollout) May(box string) bool {
	if r.Status == "bad" {
		return false
	}
	k := r.WaveOf(box)
	if k < 0 {
		return r.Status == "done"
	}
	return k < r.Admitted
}

// Proven: box's wave has passed, so a stop does not roll it back
func (r *Rollout) Proven(box string) bool {
	k := r.WaveOf(box)
	if r.Status == "done" {
		return true
	}
	return k >= 0 && k < r.Admitted-1 || (k == 0 && r.Status == "good")
}

type releasePayload struct {
	Counter uint64                     `json:"counter"`
	Boxes   map[string]json.RawMessage `json:"boxes"`
}

// readRelease checks a release file against the release key, as the
// agent's verify_json does in its v2 form (the payload as published, keys
// sorted at every level), and gives its counter, boxes and digest. The file
// is exactly payload, signer and signature: any other key is refused.
func readRelease(cfg *Config, raw string) (*releasePayload, string, error) {
	if cfg.Identity == nil || cfg.Identity.Release == "" {
		return nil, "", errors.New("this fleet's config names no release key")
	}
	// a key twice anywhere would let what is read here differ from what
	// the signature covers
	if err := ident.NoDuplicates([]byte(raw)); err != nil {
		return nil, "", fmt.Errorf("not a release: %v", err)
	}
	top, err := releaseFields(raw)
	if err != nil {
		return nil, "", fmt.Errorf("not a release: %v", err)
	}
	var signer, signature string
	if json.Unmarshal(top["signer"], &signer) != nil || json.Unmarshal(top["signature"], &signature) != nil {
		return nil, "", errors.New("not a release: signer and signature are strings")
	}
	payload := top["payload"]
	if signer != cfg.Identity.Release {
		return nil, "", errors.New("not signed by the release key")
	}
	pub, err := ident.DecodePublic(cfg.Identity.Release)
	if err != nil {
		return nil, "", err
	}
	sig, err := base64.StdEncoding.Strict().DecodeString(signature)
	if err != nil || len(sig) != ed25519.SignatureSize {
		return nil, "", errors.New("not a signature")
	}
	msg, err := ident.CanonicalValue(payload)
	if err != nil {
		return nil, "", err
	}
	if !ed25519.Verify(pub, msg, sig) {
		return nil, "", errors.New("not signed by the release key")
	}
	// read from the bytes the signature covers, not the bytes handed in
	var p releasePayload
	if err := json.Unmarshal(msg, &p); err != nil {
		return nil, "", err
	}
	return &p, Digest(msg, sig), nil
}

// releaseFields reads the top of a release file: payload, signer and
// signature, each once and named exactly so, and nothing after
func releaseFields(raw string) (map[string]json.RawMessage, error) {
	d := json.NewDecoder(strings.NewReader(raw))
	if t, err := d.Token(); err != nil || t != json.Delim('{') {
		return nil, errors.New("an object")
	}
	top := map[string]json.RawMessage{}
	for d.More() {
		t, err := d.Token()
		if err != nil {
			return nil, err
		}
		k, _ := t.(string)
		if _, seen := top[k]; seen || k != "payload" && k != "signer" && k != "signature" {
			return nil, fmt.Errorf("a field %q", k)
		}
		var v json.RawMessage
		if err := d.Decode(&v); err != nil {
			return nil, err
		}
		top[k] = v
	}
	if _, err := d.Token(); err != nil {
		return nil, err
	}
	if _, err := d.Token(); err != io.EOF {
		return nil, errors.New("something after the release")
	}
	if len(top) != 3 {
		return nil, errors.New("payload, signer and signature, each once")
	}
	return top, nil
}

// Digest names a release by what its signature covers: sha256 hex of the
// canonical payload, then the signature. However a box re-writes the file
// it hands in, the digest is the one every agent computes from the file it
// fetched (release::rollout::digest in Rust).
func Digest(canonical, sig []byte) string {
	h := sha256.New()
	h.Write(canonical)
	h.Write(sig)
	return hex.EncodeToString(h.Sum(nil))
}

func (s *State) checkRollout(cfg *Config, box string, body json.RawMessage, now int64) error {
	if len(body) > MaxRollout {
		return fmt.Errorf("a rollout request of %d bytes; at most %d", len(body), MaxRollout)
	}
	var op RolloutOp
	if err := json.Unmarshal(body, &op); err != nil {
		return err
	}
	r := s.Rollout
	switch op.Op {
	case "begin":
		p, digest, err := readRelease(cfg, op.Release)
		if err != nil {
			return err
		}
		// only a box the release is for begins it: a box outside it, a
		// joined one building its own, takes it once every wave has
		if _, ok := p.Boxes[box]; !ok {
			return fmt.Errorf("release %d is not for %s", p.Counter, box)
		}
		if r != nil && p.Counter == r.Counter {
			if digest != r.Digest {
				return fmt.Errorf("release %d began as another file", p.Counter)
			}
			return nil // begun already: nothing to do
		}
		if r != nil && p.Counter < r.Counter {
			return fmt.Errorf("release %d is not newer than the rollout of %d", p.Counter, r.Counter)
		}
		return nil
	case "verdict":
		if r == nil || r.Counter != op.Counter {
			return fmt.Errorf("no rollout of release %d", op.Counter)
		}
		if len(op.Reason) > 1024 {
			return errors.New("a reason of at most 1K")
		}
		k := r.WaveOf(box)
		if k < 0 || k >= r.Admitted {
			return fmt.Errorf("%s's wave is not admitted", box)
		}
		if r.Status == "bad" || r.Status == "done" || k < r.Admitted-1 {
			return fmt.Errorf("%s's wave is judged already", box)
		}
		switch op.Verdict {
		case "took", "bad":
			return nil
		case "good":
			if r.Reports[box].Took == 0 {
				return fmt.Errorf("%s has not said it took release %d", box, op.Counter)
			}
			return nil
		}
		return fmt.Errorf("no such verdict: %q", op.Verdict)
	}
	return fmt.Errorf("no such rollout operation: %q", op.Op)
}

func (s *State) applyRollout(cfg *Config, box string, body json.RawMessage, now int64) {
	var op RolloutOp
	_ = json.Unmarshal(body, &op)
	switch op.Op {
	case "begin":
		p, digest, _ := readRelease(cfg, op.Release)
		if r := s.Rollout; r != nil && r.Counter == p.Counter {
			return
		}
		s.Rollout = &Rollout{
			Counter: p.Counter, Digest: digest, Began: now,
			Waves: s.waves(cfg, p, now), Admitted: 1, Since: now,
			Reports: map[string]Report{}, Status: "soaking",
		}
	case "verdict":
		r := s.Rollout
		rep := r.Reports[box]
		switch op.Verdict {
		case "took":
			if rep.Took == 0 {
				rep.Took = now
			}
		case "good":
			rep.Good = now
		case "bad":
			if rep.Bad == 0 {
				rep.Bad, rep.Reason = now, op.Reason
			}
		}
		r.Reports[box] = rep
	}
}

// waves fixes who goes when, from the agreed state now: the release's boxes
// this config knows. The live ones are ranked; the rest go last.
func (s *State) waves(cfg *Config, p *releasePayload, now int64) [][]string {
	rules := cfg.Rules.Rollout.withDefaults()
	var live, away []string
	for name := range p.Boxes {
		if _, ok := s.BoxKey(cfg, name); !ok {
			continue // not this fleet's: it takes the release once all have
		}
		if s.Alive(cfg, name, now) {
			live = append(live, name)
		} else {
			away = append(away, name)
		}
	}
	sort.Strings(away)
	order := s.rank(cfg, live, p.Counter, now)
	n := len(order)
	var out [][]string
	at := 0
	for _, share := range rules.Waves {
		upto := int(math.Ceil(share * float64(n)))
		if upto < 1 {
			upto = 1
		}
		if n >= 2 && upto >= n {
			upto = n - 1 // someone always waits for the first to prove it
		}
		if upto > at && upto < n {
			out = append(out, order[at:upto])
			at = upto
		}
	}
	last := append(append([]string{}, order[at:]...), away...)
	if len(last) > 0 {
		out = append(out, last)
	}
	return out
}

// rank orders the live boxes for the waves: the ones holding no writer
// first, so the first wave needs no handover with nowhere proven to go;
// then, one at a time, the box adding most that the boxes before it have
// not covered (a house, an architecture, a class of hardware, the services
// it runs); then by a hash of the counter and its name, so another box
// goes first next time
func (s *State) rank(cfg *Config, live []string, counter uint64, now int64) []string {
	writers := map[string]int{}
	for _, l := range s.Leases {
		if l.Expires > now {
			writers[l.Holder]++
		}
	}
	traits := map[string][]string{}
	for _, name := range live {
		b := s.Boxes[name]
		house := cfg.Boxes[name].House
		if j := s.Joined[name]; j != nil {
			house = j.House // a box joined on its own key
		}
		t := []string{"house:" + house}
		arch := "other"
		if b.Facts["x86_64"] > 0 {
			arch = "x86_64"
		}
		t = append(t, "arch:"+arch)
		ram := 0
		if v := b.Facts["ram_bytes"]; v > 0 {
			ram = int(math.Round(math.Log2(v / (1 << 30))))
		}
		cores := 0
		if v := b.Facts["cores"]; v > 0 {
			cores = int(math.Round(math.Log2(v)))
		}
		t = append(t, fmt.Sprintf("hw:%d/%d", ram, cores))
		for k, v := range b.Facts {
			if svc, ok := strings.CutPrefix(k, "runs/"); ok && v > 0 {
				t = append(t, "svc:"+svc)
			}
		}
		for _, r := range b.Roles {
			t = append(t, "role:"+r)
		}
		traits[name] = t
	}
	tie := func(name string) string {
		h := sha256.Sum256([]byte(fmt.Sprintf("%d\x00%s", counter, name)))
		return hex.EncodeToString(h[:])
	}
	covered := map[string]bool{}
	left := append([]string{}, live...)
	var out []string
	for len(left) > 0 {
		best := -1
		var bw, bn int
		var bt string
		for i, name := range left {
			w := min(writers[name], 1)
			n := 0
			for _, t := range traits[name] {
				if !covered[t] {
					n++
				}
			}
			t := tie(name)
			if best < 0 || w < bw || w == bw && (n > bn || n == bn && t < bt) {
				best, bw, bn, bt = i, w, n, t
			}
		}
		name := left[best]
		out = append(out, name)
		for _, t := range traits[name] {
			covered[t] = true
		}
		left = append(left[:best], left[best+1:]...)
	}
	return out
}

// judge moves the rollout on at agreed time now; prev is the time of the
// batch before, for gaps in agreement
func (s *State) judge(cfg *Config, prev, now int64) {
	r := s.Rollout
	if r == nil || r.Status == "bad" || r.Status == "done" {
		return
	}
	// an outage of the control plane is nobody's fault: deadlines move on
	// by it, and a soak is watched agreed time after it
	if prev > 0 && now-prev > cfg.Rules.GoneAfter {
		gap := now - prev
		r.Paused += gap
		r.Since += gap
		r.GapEnd = now
	}
	rules := cfg.Rules.Rollout.withDefaults()
	for r.Status != "done" && r.Status != "bad" {
		k := r.Admitted - 1
		soak := rules.Soak(k)
		passed, waiting := true, []string{}
		for _, name := range r.Waves[k] {
			rep := r.Reports[name]
			if rep.Bad > 0 {
				r.Status = "bad"
				r.Why = fmt.Sprintf("%s: %s", name, rep.Reason)
				return
			}
			if contains(r.Excused, name) {
				continue
			}
			if rep.Took == 0 || rep.Good < max(rep.Took, r.GapEnd)+soak {
				passed = false
				waiting = append(waiting, name)
			}
		}
		if !passed {
			if now <= r.Since+rules.TookMs+soak+rules.GraceMs {
				return
			}
			// past the deadline: a later wave's box away all through it
			// is excused and takes it when back; any other is not proven
			var stuck []string
			for _, name := range waiting {
				if k > 0 && r.Reports[name].Took == 0 && !s.Alive(cfg, name, now) {
					r.Excused = append(r.Excused, name)
				} else {
					stuck = append(stuck, name)
				}
			}
			if len(stuck) > 0 {
				r.Status = "bad"
				r.Why = fmt.Sprintf("%s: not proven by %d", strings.Join(stuck, ", "), now)
				return
			}
			continue // only the excused were waited for: the wave passes
		}
		if k == 0 {
			r.Status = "good"
		}
		if r.Admitted == len(r.Waves) {
			r.Status = "done"
			return
		}
		r.Admitted++
		r.Since = now
	}
}
