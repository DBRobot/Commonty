package computers

// Who may say what about a remote computer, and what the judge gives it:
// members only, their root for what it is and a device of theirs to start
// or stop it, their computers within the totals together, ports handed once
// and the same on every box, an old item sent again changing nothing, and a
// member who leaves having theirs stopped, not deleted.

import (
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"testing"

	"commonty/control/state/ident"
)

var b64 = base64.StdEncoding

func key(seed byte) ed25519.PrivateKey {
	s := make([]byte, ed25519.SeedSize)
	s[0] = seed
	return ed25519.NewKeyFromSeed(s)
}

func pub(k ed25519.PrivateKey) string { return b64.EncodeToString(k.Public().(ed25519.PublicKey)) }

func seal(t *testing.T, kind string, body []byte, k ed25519.PrivateKey) json.RawMessage {
	t.Helper()
	env := ident.Envelope{V: 2, Kind: kind, Body: b64.EncodeToString(body), Sig: b64.EncodeToString(ed25519.Sign(k, ident.Message(kind, body)))}
	b, err := json.Marshal(env)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

func body(t *testing.T, v any) []byte {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	return b
}

type person struct {
	name         string
	root, device ed25519.PrivateKey
	recovery     ed25519.PrivateKey
}

func who(name string, seed byte) person {
	return person{name: name, root: key(seed), device: key(seed + 1), recovery: key(seed + 2)}
}

func (p person) entry(t *testing.T) json.RawMessage {
	return p.entryAt(t, 1, []ident.Device{{Fingerprint: "f", PublicKey: pub(p.device), Added: 1}})
}

// entryAt: their entry at version v, with these devices
func (p person) entryAt(t *testing.T, v uint64, devices []ident.Device) json.RawMessage {
	e := ident.Entry{Name: p.name, Root: pub(p.root), Recovery: pub(p.recovery),
		Devices: devices, Version: v, Updated: v}
	b, err := e.Canonical()
	if err != nil {
		t.Fatal(err)
	}
	return seal(t, "entry", b, p.root)
}

var voters = []string{"node1", "node2"}

type fleet struct {
	t   *testing.T
	cfg *ident.Config
	lim *Limits
	log []ident.Item
	r   *Replayer
	at  int64
}

func newFleet(t *testing.T, members ...person) *fleet {
	cfg := &ident.Config{}
	for _, m := range members {
		cfg.Members.Members = append(cfg.Members.Members, ident.MemberID(pub(m.root)))
	}
	lim := &Limits{Memory: 16384, Threads: 8, Disk: 200, PortBase: 27200, PortCount: 4}
	f := &fleet{t: t, cfg: cfg, lim: lim, r: NewReplayer(cfg, lim, voters), at: 1_800_000_000_000}
	return f
}

// claim: a person's first entry, acknowledged by every voter
func (f *fleet) claim(p person) {
	raw := p.entry(f.t)
	f.add(raw)
	for _, v := range voters {
		f.at += 1000
		f.log = append(f.log, ident.Item{Time: f.at, Box: v, Item: body(f.t, map[string]string{"ack": ident.LogName(raw)})})
	}
}

func (f *fleet) add(raw json.RawMessage) string {
	f.at += 1000
	f.log = append(f.log, ident.Item{Time: f.at, Box: "node1", Item: raw})
	_, s := f.r.CatchN(len(f.log), func(i int) ident.Item { return f.log[i] })
	return s.Outcomes[ident.ItemHash(raw)]
}

func (f *fleet) now() uint64 { return uint64(f.at / 1000) }

func (f *fleet) view() *View {
	id, s := f.r.CatchN(len(f.log), func(i int) ident.Item { return f.log[i] })
	return s.View(id, f.cfg, 1, f.at)
}

func make1(owner, id, name string, mem, thr, version uint64) Computer {
	return Computer{Owner: owner, ID: id, Name: name, Config: "default", Memory: mem, Threads: thr, Disk: 32,
		Nix: "", Ports: []uint64{}, Players: []string{}, Consent: "any", Anywhere: true, Version: version}
}

func TestOnlyMembersByTheirRoot(t *testing.T) {
	tom, ann, eve := who("tom", 1), who("ann", 10), who("eve", 20)
	f := newFleet(t, tom, ann)
	f.claim(tom)
	f.claim(ann)
	f.claim(eve) // signed up, not let in
	c := make1("tom", "00000000000000a1", "dev", 4096, 2, 1)
	if got := f.add(seal(t, "computer", body(t, c), ann.root)); got == "ok" {
		t.Fatal("ann signed tom's computer")
	}
	if got := f.add(seal(t, "computer", body(t, c), tom.device)); got == "ok" {
		t.Fatal("a device made a computer: that takes the root")
	}
	if got := f.add(seal(t, "computer", body(t, c), tom.root)); got != "ok" {
		t.Fatal(got)
	}
	e := make1("eve", "00000000000000e1", "dev", 1024, 1, 1)
	if got := f.add(seal(t, "computer", body(t, e), eve.root)); got == "ok" {
		t.Fatal("not a member, and made a computer")
	}
	// the same item again is older than what is on file
	if got := f.add(seal(t, "computer", body(t, c), tom.root)); got == "ok" {
		t.Fatal("an old item counted twice")
	}
	// says more than its kind: refused
	extra := map[string]any{}
	_ = json.Unmarshal(body(t, c), &extra)
	extra["version"] = 2
	extra["root"] = true
	if got := f.add(seal(t, "computer", body(t, extra), tom.root)); got == "ok" {
		t.Fatal("an unknown field passed")
	}
}

func TestTotalsPortsAndConsent(t *testing.T) {
	tom, ann := who("tom", 1), who("ann", 10)
	f := newFleet(t, tom, ann)
	f.claim(tom)
	f.claim(ann)
	a := make1("tom", "00000000000000a1", "a", 12288, 6, 1)
	a.Ports = []uint64{8080}
	if got := f.add(seal(t, "computer", body(t, a), tom.root)); got != "ok" {
		t.Fatal(got)
	}
	// stopped still counts
	p := Power{ID: a.ID, Up: false, At: f.now()}
	if got := f.add(seal(t, "computer-power", body(t, p), tom.device)); got != "ok" {
		t.Fatal(got)
	}
	b := make1("tom", "00000000000000b1", "b", 8192, 1, 1)
	if got := f.add(seal(t, "computer", body(t, b), tom.root)); got == "ok" {
		t.Fatal("over the total")
	}
	// anyone's box without the yes
	n := make1("ann", "00000000000000c1", "c", 1024, 1, 1)
	n.Anywhere = false
	if got := f.add(seal(t, "computer", body(t, n), ann.root)); got == "ok" {
		t.Fatal("anyone's box without the yes")
	}
	n.Anywhere = true
	if got := f.add(seal(t, "computer", body(t, n), ann.root)); got != "ok" {
		t.Fatal(got)
	}
	v := f.view()
	ta, ac := v.Computers[a.ID], v.Computers[n.ID]
	if ta.Port != 27200 || len(ta.Forwards) != 1 || ta.Forwards[0].Host != 27201 || ac.Port != 27202 {
		t.Fatalf("ports: %+v %+v", ta, ac)
	}
	if ta.Up || !ac.Up {
		t.Fatal("up as asked")
	}
	// an update keeps its ports; a new forward gets the next free
	a.Version, a.Ports = 2, []uint64{8080, 3000}
	if got := f.add(seal(t, "computer", body(t, a), tom.root)); got != "ok" {
		t.Fatal(got)
	}
	ta = f.view().Computers[a.ID]
	if ta.Port != 27200 || ta.Forwards[0].Host != 27201 || ta.Forwards[1].Host != 27203 {
		t.Fatalf("kept and handed: %+v", ta.Forwards)
	}
	// the pool (4) is full
	d := make1("ann", "00000000000000d1", "d", 1024, 1, 1)
	if got := f.add(seal(t, "computer", body(t, d), ann.root)); got == "ok" {
		t.Fatal("past the pool")
	}
	// a delete frees them
	del := Delete{Owner: "tom", ID: a.ID, At: f.now()}
	if got := f.add(seal(t, "computer-delete", body(t, del), tom.device)); got == "ok" {
		t.Fatal("a device deleted a computer: that takes the root")
	}
	if got := f.add(seal(t, "computer-delete", body(t, del), tom.root)); got != "ok" {
		t.Fatal(got)
	}
	// the deleted one, its old item sent again: it stays gone
	a.Version = 9
	if got := f.add(seal(t, "computer", body(t, a), tom.root)); got == "ok" {
		t.Fatal("a deleted computer came back")
	}
	if got := f.add(seal(t, "computer", body(t, d), ann.root)); got != "ok" {
		t.Fatal(got)
	}
	if f.view().Computers[d.ID].Port != 27200 {
		t.Fatal("the lowest free port")
	}
}

func TestPowerFromADeviceOnlyForward(t *testing.T) {
	tom, ann := who("tom", 1), who("ann", 10)
	f := newFleet(t, tom, ann)
	f.claim(tom)
	f.claim(ann)
	c := make1("tom", "00000000000000a1", "dev", 2048, 1, 1)
	f.add(seal(t, "computer", body(t, c), tom.root))
	stop := Power{ID: c.ID, Up: false, At: f.now()}
	if got := f.add(seal(t, "computer-power", body(t, stop), ann.device)); got == "ok" {
		t.Fatal("ann stopped tom's")
	}
	if got := f.add(seal(t, "computer-power", body(t, stop), tom.device)); got != "ok" {
		t.Fatal(got)
	}
	// the same stop again, and an older start: nothing
	if got := f.add(seal(t, "computer-power", body(t, stop), tom.device)); got == "ok" {
		t.Fatal("replayed")
	}
	start := Power{ID: c.ID, Up: true, At: stop.At - 1}
	if got := f.add(seal(t, "computer-power", body(t, start), tom.device)); got == "ok" {
		t.Fatal("older than the last")
	}
	start.At = f.now() + 3600
	if got := f.add(seal(t, "computer-power", body(t, start), tom.device)); got == "ok" {
		t.Fatal("dated ahead")
	}
	start.At = f.now()
	if got := f.add(seal(t, "computer-power", body(t, start), tom.device)); got != "ok" {
		t.Fatal(got)
	}
	if !f.view().Computers[c.ID].Up {
		t.Fatal("started")
	}
}

func TestALeaverIsStoppedNotDeleted(t *testing.T) {
	tom := who("tom", 1)
	f := newFleet(t, tom)
	f.claim(tom)
	c := make1("tom", "00000000000000a1", "dev", 2048, 1, 1)
	f.add(seal(t, "computer", body(t, c), tom.root))
	// the release takes tom off the list
	f.cfg.Members.Members = nil
	v := f.view()
	got := v.Computers[c.ID]
	if got == nil || got.Up || !got.Asked {
		t.Fatalf("kept, stopped, as asked: %+v", got)
	}
}

func TestConfigs(t *testing.T) {
	tom, ann := who("tom", 1), who("ann", 10)
	f := newFleet(t, tom, ann)
	f.claim(tom)
	f.claim(ann)
	c := Config{Owner: "tom", Name: "big", Memory: 8192, Threads: 4, Disk: 64, Nix: "{ }", Ports: []uint64{}, Players: []string{"ann"}, Consent: "any", Version: 1}
	if got := f.add(seal(t, "computer-config", body(t, c), tom.root)); got != "ok" {
		t.Fatal(got)
	}
	c.Players = []string{"nobody"}
	c.Version = 2
	if got := f.add(seal(t, "computer-config", body(t, c), tom.root)); got == "ok" {
		t.Fatal("let in someone who isn't a member")
	}
	gone := Config{Owner: "tom", Name: "big", Ports: []uint64{}, Players: []string{}, Version: 3, Gone: true}
	if got := f.add(seal(t, "computer-config", body(t, gone), tom.root)); got != "ok" {
		t.Fatal(got)
	}
	if _, there := f.view().Configs["tom"]["big"]; there {
		t.Fatal("a deleted config is shown")
	}
}

// the identity judge leaves these kinds alone
func TestIdentityLeavesThemAlone(t *testing.T) {
	tom := who("tom", 1)
	f := newFleet(t, tom)
	f.claim(tom)
	raw := seal(t, "computer", body(t, make1("tom", "00000000000000a1", "dev", 2048, 1, 1)), tom.root)
	f.add(raw)
	id, _ := f.r.CatchN(len(f.log), func(i int) ident.Item { return f.log[i] })
	if _, judged := id.Outcomes[ident.ItemHash(raw)]; judged {
		t.Fatal("identity judged a computer")
	}
}

// /computers: one member's own, and 304 while nothing changed
func TestServe(t *testing.T) {
	tom, ann := who("tom", 1), who("ann", 10)
	f := newFleet(t, tom, ann)
	f.claim(tom)
	f.claim(ann)
	f.add(seal(t, "computer", body(t, make1("tom", "00000000000000a1", "dev", 2048, 1, 1)), tom.root))
	f.add(seal(t, "computer", body(t, make1("ann", "00000000000000b1", "lab", 2048, 1, 1)), ann.root))
	sv := NewServer(f.cfg, f.lim, voters)
	item := func(i int) ident.Item { return f.log[i] }
	code, b := sv.Answer(len(f.log), item, 7, f.at, "tom", 0)
	var mine struct {
		Changed   uint64             `json:"changed"`
		Computers map[string]*Listed `json:"computers"`
	}
	if code != 200 || json.Unmarshal(b, &mine) != nil || len(mine.Computers) != 1 || mine.Computers["00000000000000a1"] == nil {
		t.Fatalf("%d %s", code, b)
	}
	if code, _ := sv.Answer(len(f.log), item, 8, f.at, "", mine.Changed); code != 304 {
		t.Fatalf("unchanged: %d", code)
	}
}

// many readers at once while the log grows: one answer at a time, no race
// (go test -race)
func TestServeAtOnce(t *testing.T) {
	tom := who("tom", 1)
	f := newFleet(t, tom)
	f.claim(tom)
	for i := 0; i < 20; i++ {
		c := make1("tom", fmt.Sprintf("%016x", i+1), fmt.Sprintf("c%d", i), 512, 0, 1)
		c.Threads = 1
		f.log = append(f.log, ident.Item{Time: f.at + int64(i+1)*1000, Box: "node1", Item: seal(t, "computer", body(t, c), tom.root)})
	}
	sv := NewServer(f.cfg, f.lim, voters)
	done := make(chan struct{})
	for g := 0; g < 8; g++ {
		go func(g int) {
			defer func() { done <- struct{}{} }()
			for n := len(f.log) - 20; n <= len(f.log); n++ {
				sv.Answer(n, func(i int) ident.Item { return f.log[i] }, uint64(n), f.at, "", 0)
			}
		}(g)
	}
	for g := 0; g < 8; g++ {
		<-done
	}
}

func TestTheReviewsCases(t *testing.T) {
	tom, ann := who("tom", 1), who("ann", 10)
	f := newFleet(t, tom, ann)
	f.claim(tom)
	f.claim(ann)

	// a device taken off their entry starts and stops nothing
	c := make1("tom", "00000000000000a1", "dev", 2048, 1, 1)
	if got := f.add(seal(t, "computer", body(t, c), tom.root)); got != "ok" {
		t.Fatal(got)
	}
	other := key(77)
	raw := tom.entryAt(t, 2, []ident.Device{{Fingerprint: "g", PublicKey: pub(other), Added: 2}})
	f.add(raw)
	if id, _ := f.r.CatchN(len(f.log), func(i int) ident.Item { return f.log[i] }); id.Outcomes[ident.ItemHash(raw)] != "ok" {
		t.Fatal(id.Outcomes[ident.ItemHash(raw)])
	}
	stop := Power{ID: c.ID, Up: false, At: f.now()}
	if got := f.add(seal(t, "computer-power", body(t, stop), tom.device)); got == "ok" {
		t.Fatal("a removed device stopped it")
	}
	if got := f.add(seal(t, "computer-power", body(t, stop), other)); got != "ok" {
		t.Fatal(got)
	}

	// the same id, someone else's
	steal := make1("ann", c.ID, "mine", 1024, 1, 9)
	if got := f.add(seal(t, "computer", body(t, steal), ann.root)); got == "ok" {
		t.Fatal("ann took tom's id")
	}

	// my boxes only, with no box: refused
	own := make1("ann", "00000000000000b1", "own", 1024, 1, 1)
	own.Consent, own.Anywhere = "own", false
	if got := f.add(seal(t, "computer", body(t, own), ann.root)); got == "ok" {
		t.Fatal("own, with no box")
	}

	// disk is a total too: 200 across them
	big := make1("ann", "00000000000000c1", "big", 1024, 1, 1)
	big.Disk = 150
	if got := f.add(seal(t, "computer", body(t, big), ann.root)); got != "ok" {
		t.Fatal(got)
	}
	big2 := make1("ann", "00000000000000c2", "big2", 1024, 1, 1)
	big2.Disk = 60
	if got := f.add(seal(t, "computer", body(t, big2), ann.root)); got == "ok" {
		t.Fatal("over 200 GiB together")
	}

	// players: no more than 32, none twice
	two := make1("ann", "00000000000000c3", "two", 1024, 1, 1)
	two.Players = []string{"tom", "tom"}
	if got := f.add(seal(t, "computer", body(t, two), ann.root)); got == "ok" {
		t.Fatal("a player twice")
	}
	many := make([]string, MaxPlayers+1)
	for i := range many {
		many[i] = fmt.Sprintf("p%d", i)
	}
	two.Players = many
	if got := f.add(seal(t, "computer", body(t, two), ann.root)); got == "ok" {
		t.Fatal("too many players")
	}

	// a config's name in a computer is a name
	odd := make1("ann", "00000000000000c4", "odd", 1024, 1, 1)
	odd.Config = "../x"
	if got := f.add(seal(t, "computer", body(t, odd), ann.root)); got == "ok" {
		t.Fatal("a config name that isn't one")
	}
}

func TestEachCap(t *testing.T) {
	tom := who("tom", 1)
	f := newFleet(t, tom)
	f.lim.PortCount = 100
	f.claim(tom)
	// configs: 32, deleted ones counted
	for i := 0; i < MaxConfigs; i++ {
		c := Config{Owner: "tom", Name: fmt.Sprintf("c%d", i), Memory: 512, Threads: 1, Disk: 8, Ports: []uint64{}, Players: []string{}, Consent: "any", Version: 1}
		if got := f.add(seal(t, "computer-config", body(t, c), tom.root)); got != "ok" {
			t.Fatal(i, got)
		}
	}
	gone := Config{Owner: "tom", Name: "c0", Ports: []uint64{}, Players: []string{}, Version: 2, Gone: true}
	f.add(seal(t, "computer-config", body(t, gone), tom.root))
	one := Config{Owner: "tom", Name: "more", Memory: 512, Threads: 1, Disk: 8, Ports: []uint64{}, Players: []string{}, Consent: "any", Version: 1}
	if got := f.add(seal(t, "computer-config", body(t, one), tom.root)); got == "ok" {
		t.Fatal("past the configs' cap")
	}
	// ports: 16 across their computers, SSH counted
	c := make1("tom", "00000000000000a1", "a", 512, 1, 1)
	c.Disk = 8
	c.Ports = []uint64{1, 2, 3, 4, 5, 6, 7, 8}
	if got := f.add(seal(t, "computer", body(t, c), tom.root)); got != "ok" {
		t.Fatal(got)
	}
	d := make1("tom", "00000000000000a2", "b", 512, 1, 1)
	d.Disk = 8
	d.Ports = []uint64{1, 2, 3, 4, 5, 6} // 9 + 7: 16
	if got := f.add(seal(t, "computer", body(t, d), tom.root)); got != "ok" {
		t.Fatal(got)
	}
	e := make1("tom", "00000000000000a3", "c", 512, 1, 1)
	e.Disk = 8
	if got := f.add(seal(t, "computer", body(t, e), tom.root)); got == "ok" {
		t.Fatal("past 16 ports")
	}
}

func TestASuspendedOwner(t *testing.T) {
	tom := who("tom", 1)
	f := newFleet(t, tom)
	f.claim(tom)
	c := make1("tom", "00000000000000a1", "dev", 2048, 1, 1)
	f.add(seal(t, "computer", body(t, c), tom.root))
	id := ident.MemberID(pub(tom.root))
	f.cfg.Members.Revoked = []string{id}
	f.cfg.Members.Suspended = map[string]string{id: "abuse"}
	f.cfg.Members.Removed = map[string]uint64{id: f.now()}
	if v := f.view(); v.Computers[c.ID].Up {
		t.Fatal("suspended, still up")
	}
	c.Version = 2
	if got := f.add(seal(t, "computer", body(t, c), tom.root)); got == "ok" {
		t.Fatal("suspended, still changing it")
	}
	stop := Power{ID: c.ID, Up: true, At: f.now()}
	if got := f.add(seal(t, "computer-power", body(t, stop), tom.device)); got == "ok" {
		t.Fatal("suspended, still starting it")
	}
}
