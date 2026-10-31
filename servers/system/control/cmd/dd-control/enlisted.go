package main

import (
	"bytes"
	"encoding/json"
	"log"
	"os"
	"path/filepath"
	"regexp"

	"commonty/control/place"
	"commonty/control/state"
)

// writeEnlisted says whose this joined box is, as the agreed state has it, where
// its next build reads it (nix/lib/joined-own.nix): a gain is built for at the
// agent's next run, as a loss is, though the plan took that away already. Only
// ever what the box builds: nothing placed believes it, nothing deletes by it.
func writeEnlisted(path, owner string) {
	var v struct {
		Owner *string `json:"owner"`
	}
	if owner != "" {
		v.Owner = &owner
	}
	raw, _ := json.Marshal(v)
	old, err := os.ReadFile(path)
	if os.IsNotExist(err) && owner == "" {
		return
	}
	if err == nil && bytes.Equal(old, raw) {
		return
	}
	tmp := path + ".new"
	if err := os.WriteFile(tmp, raw, 0o644); err != nil {
		log.Printf("enlisted: %v", err)
		return
	}
	if err := os.Rename(tmp, path); err != nil {
		log.Printf("enlisted: %v", err)
		return
	}
	log.Printf("this box is now %s's, by the agreed state: its next build follows", owner)
}

// a member's name as a unit's instance and a file's name, as noteOwner in
// nix/modules/computers/computers.nix takes it
var memberName = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]{0,31}$`)

// sawOwner keeps, on this box's disk, each member the agreed state has said
// owns it: the only people whose things it would ever remove
func (p *placer) sawOwner(owner string) {
	if owner == "" || p.seen == "" || !memberName.MatchString(owner) {
		return
	}
	if err := os.MkdirAll(p.seen, 0o700); err == nil {
		_ = os.WriteFile(filepath.Join(p.seen, owner), nil, 0o600)
	}
}

// how often a removal that has not finished is tried again
const forgetEvery = 10 * 60 * 1000

// forget removes what is on this box's disk of a member the plan says it no
// longer keeps. A plan alone is not enough: this box must be one that joined,
// and by the same agreed state not that member's now, the word that ended it
// long enough ago, and that member someone it has itself known as its owner.
// Called only with a plan from fresh agreement. dd-forget-owner clears the
// member from what it has known once all is gone, so this is once for each.
func (p *placer) forget(plan place.Plan, s *state.State, owner string) {
	e, said := plan.Forget[p.box]
	if !said || !p.noLeases || p.seen == "" {
		return
	}
	if !memberName.MatchString(e.Member) || !place.MayForget(e, s.Time, owner) {
		log.Printf("placed: the plan says to remove a member's things here, and this box does not agree: not done")
		return
	}
	if _, err := os.Stat(filepath.Join(p.seen, e.Member)); err != nil {
		return
	}
	now := p.uptime()
	if at, ok := p.forgot[e.Member]; ok && now-at < forgetEvery {
		return
	}
	if p.forgot == nil {
		p.forgot = map[string]int64{}
	}
	p.forgot[e.Member] = now
	if err := p.run("start", "--no-block", "dd-forget-owner@"+e.Member+".service"); err != nil {
		log.Printf("placed: removing a former owner's things: %v", err)
	}
}
