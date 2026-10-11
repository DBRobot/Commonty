package main

import (
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"commonty/control/place"
)

func newPlacer(t *testing.T) (*placer, *[]string) {
	calls := []string{}
	p := &placer{
		box:     "node1",
		dir:     filepath.Join(t.TempDir(), "dd-placed"),
		carries: map[string][]string{"web": {"web.service"}, "photos": {"ente.service", "ente-web.service"}},
		run: func(args ...string) error {
			calls = append(calls, strings.Join(args, " "))
			return nil
		},
	}
	return p, &calls
}

func TestReconcileStartsAndStops(t *testing.T) {
	p, calls := newPlacer(t)
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: "photos"}, {Service: "games"}}}})
	if !p.up("photos") || p.up("web") {
		t.Fatalf("flags: photos %v web %v", p.up("photos"), p.up("web"))
	}
	if want := []string{"start --no-block ente.service ente-web.service"}; !reflect.DeepEqual(*calls, want) {
		t.Fatalf("calls %v", *calls)
	}
	// the same plan again does nothing more
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: "photos"}}}})
	if len(*calls) != 1 {
		t.Fatalf("calls %v", *calls)
	}
	if f := p.facts(); f["carries/photos"] != 1 || f["runs/photos"] != 1 || f["runs/web"] != 0 || f["carries/games"] != 0 {
		t.Fatalf("facts %v", f)
	}
	// moved away
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {}}})
	if p.up("photos") || (*calls)[1] != "stop --no-block ente.service ente-web.service" {
		t.Fatalf("not stopped: %v", *calls)
	}
}

func TestAPlanWithoutThisBoxChangesNothing(t *testing.T) {
	p, calls := newPlacer(t)
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node1": {{Service: "web"}}}})
	p.reconcile(place.Plan{Boxes: map[string][]place.Decision{"node2": {}}})
	if !p.up("web") || len(*calls) != 1 {
		t.Fatalf("a plan this box is not in stopped it: %v", *calls)
	}
}

func TestNoCarriesFileIsNone(t *testing.T) {
	c, err := loadCarries(filepath.Join(t.TempDir(), "none.json"))
	if err != nil || len(c) != 0 {
		t.Fatalf("%v %v", c, err)
	}
	path := filepath.Join(t.TempDir(), "c.json")
	_ = os.WriteFile(path, []byte(`{"web":["web.service"]}`), 0o644)
	if c, err := loadCarries(path); err != nil || c["web"][0] != "web.service" {
		t.Fatalf("%v %v", c, err)
	}
}
