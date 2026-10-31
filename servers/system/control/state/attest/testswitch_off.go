//go:build !testswitches

package attest

// TestSwitches: whether this build reads Config.Test. A build for a box
// never does: the switches exist only in builds with the testswitches tag
// (dd-control-test, attest-check), whatever a release's config says.
const TestSwitches = false
