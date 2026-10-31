package main

import (
	"bytes"
	"encoding/json"
	"log"
	"os"
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
