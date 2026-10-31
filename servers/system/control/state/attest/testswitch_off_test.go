//go:build !testswitches

package attest

import "testing"

// a box's build reads no test switch, whatever a release's config says
func TestABoxsBuildReadsNoTestSwitch(t *testing.T) {
	if TestSwitches {
		t.Fatal("the switches are on in a build without the tag")
	}
	c := &Config{Test: &Test{SoftwareTPM: true, NoSecureBoot: true, UnattestedJoin: true}}
	p, err := c.policy()
	if err != nil {
		t.Fatal(err)
	}
	if p.test != (Test{}) {
		t.Fatalf("%+v", p.test)
	}
}
