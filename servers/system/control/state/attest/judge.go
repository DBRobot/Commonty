package attest

import (
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"sort"
	"time"
)

// State is the agreed state's part for attestation: boxes under way, what
// each one that finished proved, and which box each TPM is.
//
// The flow, each step an agreed item:
//
//	enlisting box   its request to enlist carries Begin; the enlistment's
//	                judge calls Open (an unknown or reused TPM stops there)
//	each box voter  attest-challenge: a fresh secret sealed to the EK for
//	                the AK, and its hash (Challenge)
//	enlisting box   attest-response, once a majority of box voters have
//	                challenged: the secrets, a quote and a certification
//	                over their nonce (Response) -> Done[box]
//
// One honest voter's secret is enough: the nonce holds every secret, so
// no answer could be made before that voter's challenge was.
type State struct {
	Pending map[string]*Pending `json:"pending,omitempty"`
	Done    map[string]*Record  `json:"done,omitempty"`
	EKs     map[string]string   `json:"eks,omitempty"` // EK hash -> the box it is
	DBs     map[string]string   `json:"dbs,omitempty"` // db certificate hash -> the box whose it is
}

type Pending struct {
	Begin  Begin  `json:"begin"`
	Key    string `json:"key"` // the box's join key, SPKI base64: what signs its response
	EK     string `json:"ek"`
	DB     string `json:"db,omitempty"`
	Opened int64  `json:"opened"` // agreed ms
	// at most one per box voter, in voter order
	Challenges []Challenge `json:"challenges,omitempty"`
}

const (
	// TTL: a box that has not answered in an hour starts again
	TTL = int64(time.Hour / time.Millisecond)
	// MaxBegin bounds Begin: certificates, three public areas, a log
	MaxBegin = 256 << 10
	// MaxChallenge bounds one challenge (an RSA-2048 EK's is under 1 KB)
	MaxChallenge = 4 << 10
	// MaxResponse bounds a response: secrets, two attestations, signatures
	MaxResponse = 32 << 10
)

func New() *State {
	return &State{Pending: map[string]*Pending{}, Done: map[string]*Record{}, EKs: map[string]string{}, DBs: map[string]string{}}
}

func (s *State) init() {
	if s.Pending == nil {
		s.Pending = map[string]*Pending{}
	}
	if s.Done == nil {
		s.Done = map[string]*Record{}
	}
	if s.EKs == nil {
		s.EKs = map[string]string{}
	}
	if s.DBs == nil {
		s.DBs = map[string]string{}
	}
}

func agreed(ms int64) time.Time { return time.UnixMilli(ms).UTC() }

// CheckOpen says whether box may start with b at now. It changes nothing.
func (s *State) CheckOpen(cfg *Config, box, key string, b Begin, now int64) (*Checked, error) {
	if raw, _ := json.Marshal(b); len(raw) > MaxBegin {
		return nil, fmt.Errorf("a Begin of %d bytes; at most %d", len(raw), MaxBegin)
	}
	c, err := VerifyBegin(cfg, b, key, agreed(now))
	if err != nil {
		return nil, err
	}
	if other, ok := s.EKs[c.EKHash]; ok && other != box {
		return nil, fmt.Errorf("this TPM is already %s", other)
	}
	if other, ok := s.DBs[c.DBHash]; ok && c.DBHash != "" && other != box {
		return nil, fmt.Errorf("this Secure Boot certificate is already %s's", other)
	}
	for other, p := range s.Pending {
		if other == box || now-p.Opened >= TTL {
			continue
		}
		if p.EK == c.EKHash {
			return nil, fmt.Errorf("this TPM is already enlisting as %s", other)
		}
		if c.DBHash != "" && p.DB == c.DBHash {
			return nil, fmt.Errorf("this Secure Boot certificate is already enlisting as %s", other)
		}
	}
	return c, nil
}

// Open starts box's attestation (again, if it had started); call only
// after CheckOpen passed at the same now.
func (s *State) Open(cfg *Config, box, key string, b Begin, now int64) {
	s.init()
	c, err := s.CheckOpen(cfg, box, key, b, now)
	if err != nil {
		return
	}
	s.Pending[box] = &Pending{Begin: b, Key: key, EK: c.EKHash, DB: c.DBHash, Opened: now}
}

func (s *State) live(box string, now int64) (*Pending, error) {
	p := s.Pending[box]
	if p == nil || now-p.Opened >= TTL {
		return nil, fmt.Errorf("%s is not attesting", box)
	}
	return p, nil
}

// Need: how many box voters must challenge before an answer counts - a
// majority, so the voters who challenged include an honest one wherever
// most are
func Need(voters []string) int { return len(voters)/2 + 1 }

// CheckChallenge: from (a box voter) challenges c.Box, once.
func (s *State) CheckChallenge(voters []string, from string, c Challenge, now int64) error {
	if !contains(voters, from) || c.Validator != from {
		return fmt.Errorf("%s is not a box voter challenging for itself", from)
	}
	p, err := s.live(c.Box, now)
	if err != nil {
		return err
	}
	for _, have := range p.Challenges {
		if have.Validator == from {
			return fmt.Errorf("%s has challenged %s already", from, c.Box)
		}
	}
	if h, err := hex.DecodeString(c.Commit); err != nil || len(h) != 32 {
		return errors.New("the commitment is a sha256, hex")
	}
	if _, err := b64(c.Cred); err != nil || c.Cred == "" {
		return errors.New("no credential")
	}
	if _, err := b64(c.Secret); err != nil || c.Secret == "" {
		return errors.New("no encrypted secret")
	}
	return nil
}

func (s *State) ApplyChallenge(c Challenge) {
	p := s.Pending[c.Box]
	p.Challenges = append(p.Challenges, c)
	sort.Slice(p.Challenges, func(i, j int) bool { return p.Challenges[i].Validator < p.Challenges[j].Validator })
}

// CheckResponse: box answers every challenge made of it, and enough were.
func (s *State) CheckResponse(cfg *Config, voters []string, box string, r Response, now int64) (*Record, error) {
	p, err := s.live(box, now)
	if err != nil {
		return nil, err
	}
	if r.Box != box {
		return nil, errors.New("a response for another box")
	}
	n := 0
	for _, c := range p.Challenges {
		if contains(voters, c.Validator) {
			n++
		}
	}
	if n < Need(voters) {
		return nil, fmt.Errorf("%d of %d box voters have challenged; %d needed", n, len(voters), Need(voters))
	}
	c, err := s.CheckOpen(cfg, box, p.Key, p.Begin, now)
	if err != nil {
		return nil, err
	}
	rec, err := VerifyResponse(cfg, c, p.Challenges, r)
	if err != nil {
		return nil, err
	}
	rec.At = now
	return rec, nil
}

// ApplyResponse records what box proved; call only after CheckResponse
// passed at the same now.
func (s *State) ApplyResponse(cfg *Config, voters []string, box string, r Response, now int64) {
	rec, err := s.CheckResponse(cfg, voters, box, r, now)
	if err != nil {
		return
	}
	s.init() // a state from before DBs
	if old := s.Done[box]; old != nil {
		delete(s.EKs, old.EK)
		delete(s.DBs, old.DB)
	}
	s.Done[box] = rec
	s.EKs[rec.EK] = box
	if rec.DB != "" {
		s.DBs[rec.DB] = box
	}
	delete(s.Pending, box)
}

// Expire drops attestations not answered within TTL.
func (s *State) Expire(now int64) {
	for box, p := range s.Pending {
		if now-p.Opened >= TTL {
			delete(s.Pending, box)
		}
	}
}

// Forget: box left the fleet; its TPM may enlist another.
func (s *State) Forget(box string) {
	if rec := s.Done[box]; rec != nil {
		delete(s.EKs, rec.EK)
		delete(s.DBs, rec.DB)
	}
	delete(s.Done, box)
	delete(s.Pending, box)
}

func contains(xs []string, x string) bool {
	for _, y := range xs {
		if y == x {
			return true
		}
	}
	return false
}
