package ident

import (
	"encoding/base64"
	"encoding/json"
	"testing"
)

func madeOn(t *testing.T, origin string) string {
	t.Helper()
	client, _ := json.Marshal(map[string]string{"type": "webauthn.get", "challenge": "x", "origin": origin})
	raw, _ := json.Marshal(map[string]string{
		"authenticatorData": "AAAA",
		"clientDataJSON":    b64url.EncodeToString(client),
		"signature":         "AAAA",
	})
	return base64.StdEncoding.EncodeToString(raw)
}

func TestOnlyTheAccountsOwnPagesSpeakForIt(t *testing.T) {
	cfg := &Config{RootPages: []string{"https://home.example.org", "https://compute.example.org"}}
	item := func(shape, sig string) json.RawMessage {
		switch shape {
		case "v2":
			b, _ := json.Marshal(map[string]any{"v": 2, "kind": "entry", "body": "", "sig": sig})
			return b
		case "recovery":
			b, _ := json.Marshal(map[string]any{"v": 2, "kind": "entry", "body": "", "sig": "AAAA", "recovery_sig": sig})
			return b
		case "entry":
			b, _ := json.Marshal(map[string]any{"entry": map[string]string{"signature": sig}})
			return b
		default:
			b, _ := json.Marshal(map[string]any{"leave": map[string]string{"signature": sig}})
			return b
		}
	}
	for _, shape := range []string{"v2", "recovery", "entry", "leave"} {
		for _, ok := range []string{"https://home.example.org", "https://compute.example.org"} {
			if err := cfg.OnRootPage(item(shape, madeOn(t, ok))); err != nil {
				t.Errorf("%s on %s: %v", shape, ok, err)
			}
		}
		for _, no := range []string{"https://x.j0123456789.example.org", "https://git.example.org", "https://example.org", ""} {
			if cfg.OnRootPage(item(shape, madeOn(t, no))) == nil {
				t.Errorf("%s on %q was let through", shape, no)
			}
		}
		// a device key's signature is no assertion
		if err := cfg.OnRootPage(item(shape, base64.StdEncoding.EncodeToString(make([]byte, 64)))); err != nil {
			t.Errorf("%s by a device key: %v", shape, err)
		}
	}
	// a release that names no pages judges as before
	if err := (&Config{}).OnRootPage(item("v2", madeOn(t, "https://git.example.org"))); err != nil {
		t.Errorf("no pages named: %v", err)
	}
}
