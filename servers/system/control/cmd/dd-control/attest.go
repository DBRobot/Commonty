package main

import (
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
// once a majority of box voters have challenged it.
type attesting struct {
	box    string
	voters []string
	tpm    *tpm.Box
	state  func() *state.State
	submit func(kind string, body []byte) (uint64, error)
	// what was sent and not yet agreed, so it is not sent again: a box
	// challenged, by its Opened; the challenges answered, by their commits
	sent map[string]int64
	said string
}

func (a *attesting) loop(every time.Duration) {
	for {
		a.once()
		time.Sleep(every)
	}
}

func (a *attesting) once() {
	s := a.state()
	if s.Attest == nil {
		return
	}
	voter := false
	for _, v := range a.voters {
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
	if p == nil || len(p.Challenges) < attest.Need(a.voters) {
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

func challenged(p *attest.Pending, by string) bool {
	for _, c := range p.Challenges {
		if c.Validator == by {
			return true
		}
	}
	return false
}
