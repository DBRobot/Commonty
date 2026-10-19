package state

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"testing"

	"commonty/control/state/computers"
	"commonty/control/state/ident"
)

// A remote computer's lease, judged from the log: only while it is up, on
// the boxes placement may give it (the release's hosts; for "my boxes
// only", the box.json owner's), kept only while that holds.
// node1 is a host with no owner, node2 is ann's, node3 is no host: ann
// enlists node3, which counts for nothing here.
func computerFleet(t *testing.T, consent string) (*fleet, ed25519.PrivateKey, string) {
	f := newFleet(t)
	f.c.Rules.ComputerHosts = map[string]string{"node1": "", "node2": "ann"}
	seed := make([]byte, ed25519.SeedSize)
	seed[0] = 7
	root := ed25519.NewKeyFromSeed(seed)
	pub := base64.StdEncoding.EncodeToString(root.Public().(ed25519.PublicKey))
	f.c.Validators = []Validator{{ID: 1, Name: "node1"}, {ID: 2, Name: "node2"}}
	f.c.Identity = &ident.Config{}
	f.c.Identity.Members.Members = []string{ident.MemberID(pub)}
	f.c.Computers = &computers.Limits{Memory: 16384, Threads: 8, Disk: 200, PortBase: 27200, PortCount: 8}
	seal := func(kind string, v any) json.RawMessage {
		b, err := json.Marshal(v)
		if err != nil {
			t.Fatal(err)
		}
		if raw, ok := v.([]byte); ok {
			b = raw
		}
		env, _ := json.Marshal(ident.Envelope{V: 2, Kind: kind, Body: base64.StdEncoding.EncodeToString(b), Sig: base64.StdEncoding.EncodeToString(ed25519.Sign(root, ident.Message(kind, b)))})
		return env
	}
	seed[0] = 8
	device := base64.StdEncoding.EncodeToString(ed25519.NewKeyFromSeed(seed).Public().(ed25519.PublicKey))
	e := ident.Entry{Name: "ann", Root: pub, Recovery: pub, Version: 1, Updated: 1,
		Devices: []ident.Device{{Fingerprint: "f", PublicKey: device, Added: 1}}}
	eb, err := e.Canonical()
	if err != nil {
		t.Fatal(err)
	}
	now := int64(1_800_000_000_000)
	dir := func(box string, item json.RawMessage) {
		now += 1000
		if f.block(now, f.req(box, "directory", json.RawMessage(item))) != 1 {
			t.Fatalf("directory item refused")
		}
	}
	entry := seal("entry", eb)
	dir("node1", entry)
	for _, v := range []string{"node1", "node2"} {
		ack, _ := json.Marshal(map[string]string{"ack": ident.LogName(entry)})
		dir(v, ack)
	}
	{
		at := uint64(now / 1000)
		dir("node1", seal("enlist", []byte(fmt.Sprintf(`{"member":%q,"box":"node3","key":"k","ticket":"00","expires":%d,"at":%d}`, ident.MemberID(pub), at+3600, at))))
	}
	id := "0123456789abcdef"
	dir("node1", seal("computer", computers.Computer{Owner: "ann", ID: id, Name: "dev", Config: "default", Memory: 2048, Threads: 2, Disk: 32,
		Ports: []uint64{}, Players: []string{}, Consent: consent, Anywhere: consent == "any", Version: 1}))
	for _, b := range []string{"node1", "node2", "node3"} {
		f.kvm(b, now)
	}
	if _, _, err := f.s.computerAt(f.c, id, now); err != nil {
		t.Fatalf("not judged: %v", err)
	}
	return f, root, id
}

func TestAComputersLeaseIsTheLogs(t *testing.T) {
	f, _, id := computerFleet(t, "any")
	role := "svc/computer-" + id
	now := f.s.Time
	if f.block(now+1, f.req("node1", "lease", lease("svc/computer-fedcba9876543210", "acquire", 0))) != 0 {
		t.Fatal("a computer that is not in the log was leased")
	}
	if f.block(now+2, f.req("node1", "lease", lease(role, "acquire", 0))) != 1 {
		t.Fatal("an up computer's lease refused to a live box")
	}
	if f.block(now+3, f.req("node3", "lease", lease(role, "acquire", 0))) != 0 {
		t.Fatal("a held computer's lease taken by another box")
	}
	if f.block(now+4, f.req("node1", "lease", lease(role, "renew", f.s.Leases[role].Epoch))) != 1 {
		t.Fatal("its holder could not renew")
	}
}

func TestAComputerStoppedKeepsNoLease(t *testing.T) {
	f, root, id := computerFleet(t, "any")
	role := "svc/computer-" + id
	now := f.s.Time
	if f.block(now+1, f.req("node1", "lease", lease(role, "acquire", 0))) != 1 {
		t.Fatal("refused")
	}
	b, _ := json.Marshal(computers.Power{ID: id, Up: false, At: uint64((now + 2000) / 1000)})
	env, _ := json.Marshal(ident.Envelope{V: 2, Kind: "computer-power", Body: base64.StdEncoding.EncodeToString(b), Sig: base64.StdEncoding.EncodeToString(ed25519.Sign(root, ident.Message("computer-power", b)))})
	if f.block(now+2000, f.req("node1", "directory", json.RawMessage(env))) != 1 {
		t.Fatal("power item refused")
	}
	epoch := f.s.Leases[role].Epoch
	if f.block(now+3000, f.req("node1", "lease", lease(role, "renew", epoch))) != 0 {
		t.Fatal("a stopped computer's lease was renewed")
	}
	if f.block(now+3001, f.req("node1", "lease", lease(role, "release", epoch))) != 1 {
		t.Fatal("its holder could not give it back")
	}
	if f.block(now+3002, f.req("node1", "lease", lease(role, "acquire", 0))) != 0 {
		t.Fatal("a stopped computer was leased")
	}
}

func (f *fleet) kvm(box string, now int64) {
	if f.block(now, f.req(box, "heartbeat", Heartbeat{Facts: map[string]float64{"cores": 8, "kvm": 1, "x86_64": 1}})) != 1 {
		f.t.Fatalf("%s heartbeat refused", box)
	}
}

func TestMyBoxesOnlyIsLeasedToTheirBoxesAlone(t *testing.T) {
	f, _, id := computerFleet(t, "own")
	role := "svc/computer-" + id
	if f.block(f.s.Time+1, f.req("node1", "lease", lease(role, "acquire", 0))) != 0 {
		t.Fatal("a host not the owner's took a my-boxes-only computer")
	}
	// enlisted by ann, but neither a host nor hers by the release
	if f.block(f.s.Time+2, f.req("node3", "lease", lease(role, "acquire", 0))) != 0 {
		t.Fatal("an enlisted box the release does not give to ann took her computer")
	}
	if f.block(f.s.Time+3, f.req("node2", "lease", lease(role, "acquire", 0))) != 1 {
		t.Fatal("the owner's own host was refused it")
	}
}

func TestOnlyAHostTheReleaseNamesHoldsOne(t *testing.T) {
	f, _, id := computerFleet(t, "any")
	role := "svc/computer-" + id
	if f.block(f.s.Time+1, f.req("node3", "lease", lease(role, "acquire", 0))) != 0 {
		t.Fatal("a box the release does not let host computers took one")
	}
	// a host that no longer says it has KVM: none, and its holder renews none
	if f.block(f.s.Time+2, f.req("node1", "lease", lease(role, "acquire", 0))) != 1 {
		t.Fatal("refused to a host")
	}
	f.beat("node1", f.s.Time+3)
	if f.block(f.s.Time+4, f.req("node1", "lease", lease(role, "renew", f.s.Leases[role].Epoch))) != 0 {
		t.Fatal("renewed on a box with no KVM")
	}
}

func TestAReleaseWithoutComputersLeasesNone(t *testing.T) {
	f := newFleet(t)
	f.beat("node1", 1000)
	if f.block(2000, f.req("node1", "lease", lease("svc/computer-0123456789abcdef", "acquire", 0))) != 0 {
		t.Fatal("leased with no computers in the release")
	}
}
