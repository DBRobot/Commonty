package main

import (
	"encoding/hex"
	"encoding/json"
	"log"
	"strings"
	"time"

	"commonty/control/state"
	"commonty/control/state/attest"
	"commonty/control/tpm"
)

// attesting: this box's part in attestation (docs/dev/attestation.md). A
// box voter challenges each box attesting, once; a box attesting answers
// once a majority of box voters have challenged it; a joined box begins
// again each day, and when it finds itself in a boot other than the one it
// proved. A box of the release does none of the last two, and its TPM is
// not touched here.
type attesting struct {
	box string
	// the box voters, from the config in force
	voters func(*state.State) []string
	tpm    *tpm.Box
	state  func() *state.State
	submit func(kind string, body []byte) (uint64, error)
	// what was sent and not yet agreed, so it is not sent again: a box
	// challenged, by its Opened; the challenges answered, by their commits
	sent map[string]int64
	said string
	// the last Begin sent, at this agreed time; the PCRs of this boot, read
	// once: they do not move after it
	began int64
	pcr4  string
	pcr7  string
}

func (a *attesting) loop(every time.Duration) {
	for {
		a.once()
		time.Sleep(every)
	}
}

func (a *attesting) once() {
	s := a.state()
	if s == nil || s.Version() < 3 {
		return
	}
	a.again(s)
	if s.Attest == nil {
		return
	}
	voters := a.voters(s)
	voter := false
	for _, v := range voters {
		voter = voter || v == a.box
	}
	for box, p := range s.Attest.Pending {
		if !voter || box == a.box || a.sent[box] == p.Opened || challenged(p, a.box) {
			continue
		}
		c, err := tpm.MakeChallenge(p.Begin, box, a.box)
		if err != nil {
			log.Printf("attest: challenging %s: %v", box, err)
			continue
		}
		body, _ := json.Marshal(c)
		if _, err := a.submit("attest-challenge", body); err != nil {
			log.Printf("attest: challenging %s: %v", box, err)
			continue
		}
		a.sent[box] = p.Opened
	}
	p := s.Attest.Pending[a.box]
	if p == nil || len(p.Challenges) < attest.Need(voters) {
		return
	}
	var commits []string
	for _, c := range p.Challenges {
		commits = append(commits, c.Commit)
	}
	if said := strings.Join(commits, ","); said != a.said {
		r, err := a.tpm.Respond(a.box, p.Challenges)
		if err != nil {
			log.Printf("attest: answering: %v", err)
			return
		}
		body, _ := json.Marshal(r)
		if _, err := a.submit("attest-response", body); err != nil {
			log.Printf("attest: answering: %v", err)
			return
		}
		a.said = said
	}
}

// again: a joined box proves itself again when its proof is due - none yet,
// old, or not of this boot - and it has not just tried. The judge holds the
// box to the answer; here it only decides when to ask
func (a *attesting) again(s *state.State) {
	j := s.Joined[a.box]
	if j == nil || j.Unattested {
		return
	}
	if s.Attest != nil && s.Attest.Pending[a.box] != nil {
		return
	}
	rec := s.Attested(a.box)
	if s.Time-a.began < attest.RetryEvery || (rec != nil && s.Time-rec.Tried < attest.RetryEvery) {
		return
	}
	if a.pcr4 == "" {
		p4, p7, err := a.tpm.BootPCRs()
		if err != nil {
			log.Printf("attest: reading this boot's PCRs: %v", err)
			return
		}
		a.pcr4, a.pcr7 = hex.EncodeToString(p4[:]), hex.EncodeToString(p7[:])
	}
	if !proofDue(rec, a.pcr4, a.pcr7, s.Time) {
		return
	}
	b, err := a.tpm.Begin()
	if err != nil {
		log.Printf("attest: reading its TPM: %v", err)
		return
	}
	body, _ := json.Marshal(b)
	if _, err := a.submit("attest-begin", body); err != nil {
		log.Printf("attest: proving itself again: %v", err)
		return
	}
	a.began = s.Time
}

// proofDue: rec no longer says what this boot is, or it is a day old or more
func proofDue(rec *attest.Record, pcr4, pcr7 string, now int64) bool {
	return rec == nil || rec.Failed != "" || rec.PCRs["4"] != pcr4 || rec.PCRs["7"] != pcr7 || now-rec.At >= attest.ProveEvery
}

func challenged(p *attest.Pending, by string) bool {
	for _, c := range p.Challenges {
		if c.Validator == by {
			return true
		}
	}
	return false
}
