package main

// The plan, carried out on this box: a placed service's units start only
// where /run/dd-placed/<svc> exists (nix/modules/box/placed.nix makes it
// their condition), and this is what puts the flag there and takes it
// away, by the plan every box computes from the agreed state. Only this
// box's own services, and only from a plan made from agreement that is
// fresh: with none, what runs keeps running.

import (
	"encoding/json"
	"fmt"
	"log"
	"os"
	"os/exec"
	"path/filepath"
	"sort"

	"commonty/control/place"
)

type placer struct {
	box string
	// where the flags are (/run/dd-placed): a tmpfs, so nothing placed
	// starts at boot before the plan says so
	dir string
	// service -> its units on this box, the services this box carries
	carries map[string][]string
	// systemctl, or a test's stand-in
	run func(args ...string) error
}

func loadCarries(path string) (map[string][]string, error) {
	out := map[string][]string{}
	raw, err := os.ReadFile(path)
	if os.IsNotExist(err) {
		return out, nil
	}
	if err != nil {
		return nil, err
	}
	return out, json.Unmarshal(raw, &out)
}

func systemctl(args ...string) error {
	out, err := exec.Command("systemctl", args...).CombinedOutput()
	if err != nil {
		return fmt.Errorf("systemctl %v: %v: %s", args, err, out)
	}
	return nil
}

// facts this box reports about its placed services: what it carries, and
// what it runs (a flag up)
func (p *placer) facts() map[string]float64 {
	f := map[string]float64{}
	for svc := range p.carries {
		f["carries/"+svc] = 1
		if p.up(svc) {
			f["runs/"+svc] = 1
		}
	}
	return f
}

func (p *placer) up(svc string) bool {
	_, err := os.Stat(filepath.Join(p.dir, svc))
	return err == nil
}

// reconcile makes this box run what the plan gives it of what it carries,
// and nothing else of it. A plan this box is not in (it does not count
// itself alive) changes nothing: it is the plan's view that is behind.
func (p *placer) reconcile(plan place.Plan) {
	mine, in := plan.Boxes[p.box]
	if !in {
		return
	}
	want := map[string]bool{}
	for _, d := range mine {
		want[d.Service] = true
	}
	names := make([]string, 0, len(p.carries))
	for svc := range p.carries {
		names = append(names, svc)
	}
	sort.Strings(names)
	for _, svc := range names {
		units := p.carries[svc]
		switch {
		case want[svc] && !p.up(svc):
			if err := os.MkdirAll(p.dir, 0o755); err != nil {
				log.Printf("placed: %v", err)
				continue
			}
			if err := os.WriteFile(filepath.Join(p.dir, svc), nil, 0o644); err != nil {
				log.Printf("placed: %s: %v", svc, err)
				continue
			}
			if err := p.run(append([]string{"start", "--no-block"}, units...)...); err != nil {
				log.Printf("placed: starting %s: %v", svc, err)
			} else {
				log.Printf("placed: %s runs here now", svc)
			}
		case !want[svc] && p.up(svc):
			if err := p.run(append([]string{"stop", "--no-block"}, units...)...); err != nil {
				log.Printf("placed: stopping %s: %v", svc, err)
				continue
			}
			_ = os.Remove(filepath.Join(p.dir, svc))
			log.Printf("placed: %s no longer runs here", svc)
		}
	}
}
