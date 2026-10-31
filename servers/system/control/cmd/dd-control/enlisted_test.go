package main

import (
	"os"
	"path/filepath"
	"testing"
)

func TestEnlistedFollowsTheAgreedOwner(t *testing.T) {
	path := filepath.Join(t.TempDir(), "enlisted.json")
	writeEnlisted(path, "")
	if _, err := os.Stat(path); !os.IsNotExist(err) {
		t.Fatal("a box never enlisted changed what it builds from")
	}
	writeEnlisted(path, "ann")
	raw, _ := os.ReadFile(path)
	if string(raw) != `{"owner":"ann"}` {
		t.Fatalf("%s", raw)
	}
	before, _ := os.Stat(path)
	writeEnlisted(path, "ann")
	if after, _ := os.Stat(path); !os.SameFile(before, after) {
		t.Fatal("rewritten with nothing changed")
	}
	writeEnlisted(path, "")
	raw, _ = os.ReadFile(path)
	if string(raw) != `{"owner":null}` {
		t.Fatalf("%s", raw)
	}
}
