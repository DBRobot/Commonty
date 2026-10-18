package ident

import (
	"encoding/json"
	"net/http"
	"testing"
)

func TestIdentityAnswers(t *testing.T) {
	l := newLog(t)
	ann := &person{name: "ann", root: key(1), recovery: key(2), device: key(3)}
	var items []Item
	add := func(raw json.RawMessage, box string) {
		l.at++
		items = append(items, Item{Time: int64(l.at) * 1000, Box: box, Item: raw})
	}
	first := ann.entry(t, 1, ann.root, nil)
	add(first, "node1")
	for _, v := range voters {
		add(jsonBody(t, map[string]string{"ack": LogName(first)}), v)
	}
	cfg := &Config{Release: pub(key(99))}
	id := MemberID(pub(ann.root))
	cfg.Members.Members = []string{id}
	cfg.Members.Removed = map[string]uint64{id: 1_791_000_000}
	sv := NewServer(cfg, []string{"node2", "node1"})
	get := func(n int, height uint64, name string, since uint64) (int, map[string]any) {
		code, b := sv.Answer(n, func(i int) Item { return items[i] }, height, items[n-1].Time, nil, name, since)
		var m map[string]any
		_ = json.Unmarshal(b, &m)
		return code, m
	}
	code, all := get(len(items), 10, "", 0)
	if code != http.StatusOK {
		t.Fatal(code)
	}
	m := all["members"].(map[string]any)["ann"].(map[string]any)
	if m["status"] != "member" || m["removed"].(float64) != 1_791_000_000_000 || m["agreed"].(float64) < 1e12 {
		t.Fatalf("%v", m)
	}
	changed := uint64(all["changed"].(float64))
	// nothing new: 304, however high the state went
	if code, _ := get(len(items), 20, "", changed); code != http.StatusNotModified {
		t.Fatal(code)
	}
	// one member
	if code, one := get(len(items), 20, "ann", 0); code != http.StatusOK || one["member"].(map[string]any)["id"] != id {
		t.Fatal(code, one)
	}
	if code, _ := get(len(items), 20, "nobody", 0); code != http.StatusNotFound {
		t.Fatal(code)
	}
	// she leaves: changed moves on, and left is in ms
	add(seal(t, "leave", jsonBody(t, Leave{Name: "ann", Root: pub(ann.root), At: l.at}), ann.root, nil), "node1")
	code, all = get(len(items), 30, "", changed)
	if code != http.StatusOK || uint64(all["changed"].(float64)) != 30 {
		t.Fatal(code, all["changed"])
	}
	m = all["members"].(map[string]any)["ann"].(map[string]any)
	if m["status"] != "left" || m["left"].(float64) != float64(items[len(items)-1].Time/1000*1000) {
		t.Fatalf("%v", m)
	}
}

// Requests come at once (the gate, the Workers' relay, placement): judging
// and reading under one lock, or the race detector stops the run.
func TestIdentityAnswersAtOnce(t *testing.T) {
	l := newLog(t)
	ann := &person{name: "ann", root: key(1), recovery: key(2), device: key(3)}
	var items []Item
	add := func(raw json.RawMessage, box string) {
		l.at++
		items = append(items, Item{Time: int64(l.at) * 1000, Box: box, Item: raw})
	}
	first := ann.entry(t, 1, ann.root, nil)
	add(first, "node1")
	for _, v := range voters {
		add(jsonBody(t, map[string]string{"ack": LogName(first)}), v)
	}
	cfg := &Config{Release: pub(key(99))}
	cfg.Members.Members = []string{MemberID(pub(ann.root))}
	sv := NewServer(cfg, []string{"node2", "node1"})
	done := make(chan struct{})
	for g := 0; g < 8; g++ {
		go func(g int) {
			defer func() { done <- struct{}{} }()
			for n := 1; n <= len(items); n++ {
				sv.Answer(n, func(i int) Item { return items[i] }, uint64(n), items[n-1].Time, nil, "", 0)
			}
		}(g)
	}
	for g := 0; g < 8; g++ {
		<-done
	}
}
