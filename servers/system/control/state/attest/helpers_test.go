package attest

import (
	"strings"
	"testing"
)

func wantErr(t *testing.T, err error, part string) {
	t.Helper()
	if err == nil || !strings.Contains(err.Error(), part) {
		t.Fatalf("want an error with %q, got %v", part, err)
	}
}
