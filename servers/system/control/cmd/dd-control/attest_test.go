package main

import (
	"encoding/json"
	"os"
	"testing"

	"commonty/control/state"
	"commonty/control/state/attest"
)

func TestAVoterChallengesEachBoxAttestingOnce(t *testing.T) {
	raw, err := os.ReadFile("../../state/attest/testdata/swtpm-vector.json")
	if err != nil {
		t.Fatal(err)
	}
	var v struct {
		Begin attest.Begin `json:"begin"`
	}
	_ = json.Unmarshal(raw, &v)
	s := state.New()
	s.Attest = attest.New()
	s.Attest.Pending["jbox"] = &attest.Pending{Begin: v.Begin, Opened: 5}
	var sent []string
	a := &attesting{
		box: "node1", voters: []string{"node1", "node2"}, sent: map[string]int64{},
		state: func() *state.State { return s },
		submit: func(kind string, body []byte) (uint64, error) {
			var c attest.Challenge
			_ = json.Unmarshal(body, &c)
			sent = append(sent, kind+" "+c.Box+" "+c.Validator)
			return 1, nil
		},
	}
	a.once()
	a.once() // not agreed yet: not sent again
	if len(sent) != 1 || sent[0] != "attest-challenge jbox node1" {
		t.Fatalf("sent %v", sent)
	}
	// a box that does not vote challenges no one
	a.box, sent = "node3", nil
	a.once()
	if len(sent) != 0 {
		t.Fatalf("sent %v", sent)
	}
}
