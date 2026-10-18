// dd-control: a box's part in the control plane. Every box heartbeats -
// what it has and what it runs, signed by its TPM - and keeps the agreed
// state, checked against the validators' signatures, at
// http://127.0.0.1:4195/state for the box's own services. A box named as
// its house's validator also votes. It dials out to the relay and nothing
// dials in.
package main

import (
	"commonty/control/state/ident"
	"context"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/binary"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"os/user"
	"path/filepath"
	"regexp"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

	"commonty/control/place"
	"commonty/control/state"
	"commonty/control/validator"
	"commonty/control/wire"

	"github.com/coder/websocket"
	"github.com/hyperledger/SmartBFT/pkg/metrics/disabled"
	"github.com/hyperledger/SmartBFT/pkg/wal"
)

type logger struct{}

func (logger) Debugf(string, ...any)     {}
func (logger) Infof(string, ...any)      {}
func (logger) Warnf(f string, a ...any)  { log.Printf("warn: "+f, a...) }
func (logger) Errorf(f string, a ...any) { log.Printf("error: "+f, a...) }
func (logger) Panicf(f string, a ...any) { log.Panicf(f, a...) }

// relay: one WebSocket out, redialled when it drops. Frames sent while it
// is down are lost; consensus does not need them all.
type relay struct {
	url  string
	self uint64
	// signs the hello that says who this connection is
	hello func(msg []byte) (string, error)
	// a box the release does not list: its key, which the relay checks its
	// address against (state.JoinName)
	key  string
	mu   sync.Mutex
	conn *websocket.Conn
}

func (r *relay) Send(to uint64, kind byte, p []byte) {
	r.mu.Lock()
	c := r.conn
	r.mu.Unlock()
	if c == nil {
		return
	}
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	_ = c.Write(ctx, websocket.MessageBinary, wire.Frame(to, r.self, kind, p))
}

func (r *relay) run(receive func(from uint64, kind byte, p []byte), connected func()) {
	wait := time.Second
	for {
		at := time.Now().Unix()
		sig, err := r.hello(wire.Hello(r.self, at))
		if err != nil {
			log.Printf("relay: signing the hello: %v; again in %s", err, wait)
			time.Sleep(wait)
			wait = min(wait*2, 30*time.Second)
			continue
		}
		u := fmt.Sprintf("%s?id=%d&at=%d&sig=%s", r.url, r.self, at, sig)
		if r.key != "" {
			u += "&key=" + url.QueryEscape(r.key)
		}
		c, _, err := websocket.Dial(context.Background(), u, nil)
		if err != nil {
			log.Printf("relay: %v; again in %s", err, wait)
			time.Sleep(wait)
			wait = min(wait*2, 30*time.Second)
			continue
		}
		wait = time.Second
		c.SetReadLimit(32 << 20)
		r.mu.Lock()
		r.conn = c
		r.mu.Unlock()
		log.Printf("relay: connected")
		connected()
		for {
			_, b, err := c.Read(context.Background())
			if err != nil {
				log.Printf("relay: %v", err)
				break
			}
			if _, from, kind, p, ok := wire.Parse(b); ok {
				receive(from, kind, p)
			}
		}
		r.mu.Lock()
		r.conn = nil
		r.mu.Unlock()
		_ = c.CloseNow()
	}
}

// tpmSign signs with the box's TPM-held key, under its signed boot (PCR 7),
// the key dd-unlock-enrol made on the boot partition.
func tpmSign(keys string, msg []byte) (string, error) {
	dir, err := os.MkdirTemp("", "dd-control-sign")
	if err != nil {
		return "", err
	}
	defer os.RemoveAll(dir)
	p := func(n string) string { return filepath.Join(dir, n) }
	if err := os.WriteFile(p("msg"), msg, 0o600); err != nil {
		return "", err
	}
	steps := [][]string{
		{"tpm2_createprimary", "-Q", "-C", "o", "-G", "ecc256", "-c", p("srk.ctx")},
		{"tpm2_load", "-Q", "-C", p("srk.ctx"), "-u", filepath.Join(keys, "sig.pub"), "-r", filepath.Join(keys, "sig.priv"), "-c", p("sig.ctx")},
		{"tpm2_startauthsession", "-Q", "--policy-session", "-S", p("s.ctx")},
		{"tpm2_policypcr", "-Q", "-S", p("s.ctx"), "-l", "sha256:7"},
		{"tpm2_sign", "-Q", "-c", p("sig.ctx"), "-g", "sha256", "-s", "ecdsa", "-f", "plain", "-p", "session:" + p("s.ctx"), "-o", p("sig.bin"), p("msg")},
	}
	var failed error
	for _, s := range steps {
		cmd := exec.Command(s[0], s[1:]...)
		cmd.Env = append(os.Environ(), "TPM2TOOLS_TCTI=device:/dev/tpmrm0")
		if out, err := cmd.CombinedOutput(); err != nil {
			failed = fmt.Errorf("%s: %v: %s", s[0], err, strings.TrimSpace(string(out)))
			break
		}
	}
	_ = exec.Command("tpm2_flushcontext", p("s.ctx")).Run()
	_ = exec.Command("tpm2_flushcontext", "-t").Run()
	if failed != nil {
		return "", failed
	}
	sig, err := os.ReadFile(p("sig.bin"))
	if err != nil {
		return "", err
	}
	return base64.RawURLEncoding.EncodeToString(sig), nil
}

// reportMessage is what a box signs over its own report: the files' hashes
// under a label no heartbeat starts with, so neither signature passes for
// the other (nix/checks/box-reports.py checks it the same way)
func reportMessage(box, dir string) ([]byte, error) {
	m := "commonty box report v1\x00" + box
	for _, f := range []string{"hardware.json", "hardware-configuration.nix"} {
		b, err := os.ReadFile(filepath.Join(dir, f))
		if err != nil {
			return nil, err
		}
		m += fmt.Sprintf("\x00%x", sha256.Sum256(b))
	}
	return []byte(m), nil
}

// joinMessage is what a box signs to ask its gate for a key to the fleet's
// network; the gate checks the same bytes (servers/gate/src/network.rs)
func joinMessage(box string, now int64) []byte {
	return []byte(fmt.Sprintf("commonty box join v1\x00%s\x00%d", box, now))
}

// how old agreement may be and still switch anything here
const freshFor = 30 * time.Second

// how often the plan is carried out: a third of a lease, so a writer's
// lease is renewed twice before its margin
const placeEvery = 5 * time.Second

// memFree is MemAvailable in bytes; 0 when unknown
func memFree() float64 {
	b, err := os.ReadFile("/proc/meminfo")
	if err != nil {
		return 0
	}
	for _, l := range strings.Split(string(b), "\n") {
		if fs := strings.Fields(l); len(fs) >= 2 && fs[0] == "MemAvailable:" {
			kb, _ := strconv.ParseFloat(fs[1], 64)
			return kb * 1024
		}
	}
	return 0
}

// facts: what this box has, as the placement rules will read them
func facts() map[string]float64 {
	f := map[string]float64{"cores": float64(runtime.NumCPU())}
	if b, err := os.ReadFile("/proc/meminfo"); err == nil {
		for _, l := range strings.Split(string(b), "\n") {
			fs := strings.Fields(l)
			if len(fs) >= 2 && (fs[0] == "MemTotal:" || fs[0] == "MemAvailable:") {
				kb, _ := strconv.ParseFloat(fs[1], 64)
				f[map[string]string{"MemTotal:": "ram_bytes", "MemAvailable:": "ram_free_bytes"}[fs[0]]] = kb * 1024
			}
		}
	}
	if b, err := os.ReadFile("/proc/loadavg"); err == nil {
		if fs := strings.Fields(string(b)); len(fs) > 0 {
			f["load"], _ = strconv.ParseFloat(fs[0], 64)
		}
	}
	var st syscall.Statfs_t
	if syscall.Statfs("/", &st) == nil {
		f["disk_free_bytes"] = float64(st.Bavail) * float64(st.Bsize)
	}
	hottest := 0.0
	zones, _ := filepath.Glob("/sys/class/thermal/thermal_zone*/temp")
	for _, z := range zones {
		if b, err := os.ReadFile(z); err == nil {
			if v, err := strconv.ParseFloat(strings.TrimSpace(string(b)), 64); err == nil && v/1000 > hottest {
				hottest = v / 1000
			}
		}
	}
	if hottest > 0 {
		f["temp_c"] = hottest
	}
	// whether builds that boot a VM can run here (a build order's needs)
	if _, err := os.Stat("/dev/kvm"); err == nil {
		f["kvm"] = 1
	}
	// what the fleet pages show of a box, from what the box's own units
	// write (/var/lib/dd-facts): the release it runs, how its agent's last
	// run ended, its last good backup. Here, a box's word reaches the
	// fleet without any box reading another (servers/gate/src/fleet.rs).
	// and its line, measured daily (nix/modules/box/netspeed.nix): what
	// placement weighs a service's traffic against; and what each service
	// holds in memory (nix/modules/box/service-memory.nix)
	// and its own store (nix/modules/storage/store.nix): its size, what is
	// free, and what it keeps of each bucket, for where the copies go
	for name, v := range promFacts("/var/lib/dd-facts/dd_agent.prom", "/var/lib/dd-facts/backup.prom", "/var/lib/dd-facts/net.prom", "/var/lib/dd-facts/services.prom", "/var/lib/dd-facts/store.prom", "/var/lib/dd-facts/store-copy.prom") {
		f[name] = v
	}
	for name, v := range reportFacts("/var/lib/dd/report/hardware.json", "/sys/firmware/devicetree/base/model") {
		f[name] = v
	}
	// what members keep here: not .prom, so none of it reaches Prometheus
	usage, _ := filepath.Glob("/var/lib/dd-facts/usage-*.facts")
	for name, v := range usageFacts(usage...) {
		f[name] = v
	}
	return f
}

// reportFacts: what a member's box qualifies by (ident.Qualify), from the
// box's own signed report (nix/modules/box/report.nix, detect.sh): its TPM's
// version, UEFI, its GPUs that are cards of their own (nvidia, amdgpu), and
// whether the board is a Raspberry Pi (its device tree's model)
func reportFacts(report, model string) map[string]float64 {
	out := map[string]float64{}
	if b, err := os.ReadFile(report); err == nil {
		var r struct {
			Firmware struct {
				UEFI bool `json:"uefi"`
				TPM  *struct {
					Version float64 `json:"version"`
				} `json:"tpm"`
			} `json:"firmware"`
			GPUs []struct {
				Driver string `json:"driver"`
			} `json:"gpus"`
		}
		if json.Unmarshal(b, &r) == nil {
			if r.Firmware.UEFI {
				out["uefi"] = 1
			}
			if r.Firmware.TPM != nil {
				out["tpm_major"] = r.Firmware.TPM.Version
			}
			n := 0
			for _, g := range r.GPUs {
				if g.Driver == "nvidia" || g.Driver == "amdgpu" {
					n++
				}
			}
			out["gpus"] = float64(n)
		}
	}
	if b, err := os.ReadFile(model); err == nil && strings.Contains(string(b), "Raspberry Pi") {
		out["raspberry_pi"] = 1
	}
	return out
}

// promFacts reads the few samples the heartbeat carries from text-format
// metric files: release (dd_agent_counter), agent_ok (dd_agent_info's
// result: 1 ok, 0 refused) and backup_last (dd_backup_last_success_seconds)
// maxFacts bounds what one heartbeat carries, members and services and all
const maxFacts = 512

// usageName: a service's or member's name in a usage fact, of letters,
// digits, '.', '_' and '-' only
func usageName(s string) bool {
	if s == "" || len(s) > 64 {
		return false
	}
	for _, c := range s {
		if !(c >= 'a' && c <= 'z' || c >= 'A' && c <= 'Z' || c >= '0' && c <= '9' || c == '.' || c == '_' || c == '-') {
			return false
		}
	}
	return true
}

// usageFacts reads "usage/<service>/<member> <bytes>" lines (storage/usage.nix,
// dd storage-ledger); every box's gate adds them up for the allowance
func usageFacts(files ...string) map[string]float64 {
	out := map[string]float64{}
	for _, file := range files {
		b, err := os.ReadFile(file)
		if err != nil {
			continue
		}
		for _, l := range strings.Split(string(b), "\n") {
			fs := strings.Fields(l)
			if len(fs) != 2 || strings.HasPrefix(l, "#") {
				continue
			}
			parts := strings.Split(fs[0], "/")
			if len(parts) != 3 || parts[0] != "usage" || !usageName(parts[1]) || !usageName(parts[2]) {
				continue
			}
			v, err := strconv.ParseFloat(fs[1], 64)
			if err != nil || len(out) >= maxFacts {
				continue
			}
			out[fs[0]] = v
		}
	}
	return out
}

var storeLost = regexp.MustCompile(`^dd_store_lost\{bucket="([^"]*)",box="([^"]*)"\}$`)

func promFacts(files ...string) map[string]float64 {
	out := map[string]float64{}
	for _, file := range files {
		b, err := os.ReadFile(file)
		if err != nil {
			continue
		}
		for _, l := range strings.Split(string(b), "\n") {
			if strings.HasPrefix(l, "#") {
				continue
			}
			fs := strings.Fields(l)
			if len(fs) < 2 {
				continue
			}
			v, err := strconv.ParseFloat(fs[len(fs)-1], 64)
			if err != nil {
				continue
			}
			switch name := strings.SplitN(fs[0], "{", 2)[0]; name {
			case "dd_agent_counter":
				out["release"] = v
			case "dd_backup_last_success_seconds":
				out["backup_last"] = v
			case "dd_net_metered":
				out["net_metered"] = v
			case "dd_net_down_mbps":
				out["net_down_mbps"] = v
			case "dd_net_up_mbps":
				out["net_up_mbps"] = v
			case "dd_net_rtt_ms":
				out["net_rtt_ms"] = v
			case "dd_store_size_bytes":
				out["store_size_bytes"] = v
			case "dd_store_free_bytes":
				out["store_free_bytes"] = v
			case "dd_store_bucket_bytes":
				// dd_store_bucket_bytes{bucket="ente"} as store/ente/bytes
				b, ok := strings.CutPrefix(fs[0], `dd_store_bucket_bytes{bucket="`)
				b, ok2 := strings.CutSuffix(b, `"}`)
				if ok && ok2 && len(fs) == 2 && usageName(b) && len(out) < maxFacts {
					out["store/"+b+"/bytes"] = v
				}
			case "dd_store_lost":
				// dd_store_lost{bucket="ente",box="node2"}: the copy read
				// back from node2 was not what was sent
				m := storeLost.FindStringSubmatch(fs[0])
				if m != nil && len(fs) == 2 && usageName(m[1]) && usageName(m[2]) && len(out) < maxFacts {
					out["store/"+m[1]+"/lost/"+m[2]] = v
				}
			case "dd_service_memory_bytes":
				// dd_service_memory_bytes{service="forgejo"} as svc/forgejo/mem
				svc, ok := strings.CutPrefix(fs[0], `dd_service_memory_bytes{service="`)
				svc, ok2 := strings.CutSuffix(svc, `"}`)
				if ok && ok2 && len(fs) == 2 && usageName(svc) && len(out) < maxFacts {
					out["svc/"+svc+"/mem"] = v
				}
			case "dd_agent_info":
				if strings.Contains(l, `result="ok"`) {
					out["agent_ok"] = 1
				} else {
					out["agent_ok"] = 0
				}
			}
		}
	}
	return out
}

// the validator's own key for its votes: made once, kept on the encrypted disk
func validatorKey(path string) (ed25519.PrivateKey, error) {
	if b, err := os.ReadFile(path); err == nil && len(b) == ed25519.SeedSize {
		return ed25519.NewKeyFromSeed(b), nil
	}
	_, k, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		return nil, err
	}
	if err := os.WriteFile(path, k.Seed(), 0o600); err != nil {
		return nil, err
	}
	return k, nil
}

type node interface {
	Receive(from uint64, kind byte, p []byte)
	State() *state.State
}

func main() {
	configPath := flag.String("config", "/etc/dd-control/config.json", "the signed fleet config")
	box := flag.String("box", "", "this box's name")
	relayURL := flag.String("relay", "", "wss://.../relay")
	dir := flag.String("state", "/var/lib/dd-control", "where the validator keeps its key, log and state")
	tpmKeys := flag.String("tpm-keys", "/boot/dd/unlock", "the TPM-held key's files")
	listen := flag.String("listen", "127.0.0.1:4195", "the agreed state, for this box")
	roles := flag.String("roles", "", "roles this box runs, comma separated")
	placement := flag.String("placement", "", "what each box can run and what the fleet wants (the release's placement-inputs.json); empty: no /plan")
	every := flag.Duration("heartbeat", 30*time.Second, "how often to heartbeat")
	placedCarries := flag.String("placed-carries", "", "the placed services this box carries and their units (nix/modules/box/placed.nix); empty: none, and nothing is switched")
	placedDir := flag.String("placed-dir", "/run/dd-placed", "where a placed service's flag is, the condition its units start on")
	joinKey := flag.String("join-key", "", "a box the release does not list: a file with its TPM-held key (SPKI, base64), with which it joins the agreed state under the name the key gives (state/join.go); -box must be that name")
	joinRecipient := flag.String("join-recipient", "", "with -join-key: its secrets key's age recipient, what copies' keys are sealed to")
	joinRecipientFile := flag.String("join-recipient-file", "", "with -join-key: a file holding -join-recipient")
	joinHouse := flag.String("join-house", "", "with -join-key: its house, as it found it (dd-locate)")
	joinTicket := flag.String("join-ticket-file", "", "with -join-key: a file with the member's join ticket the box was set up with (a base64 biscuit); absent or empty, it joins as a stranger")
	placedCopies := flag.String("placed-copies", "", "where to write the plan's copies, which boxes' stores keep each bucket (nix/modules/storage/store.nix); empty: nowhere")
	printKey := flag.Bool("print-validator-key", false, "make the validator key if missing, print its public half, exit")
	softKey := flag.String("box-key", "", "tests only: sign as the box with this PKCS#8 P-256 key file instead of the TPM")
	submitSock := flag.String("submit", "", "a unix socket where this box's gate hands in directory items (POST /submit); empty: none")
	submitGroup := flag.String("submit-group", "", "the group that may open -submit (the gate's)")
	builtSock := flag.String("builds", "", "a unix socket, root's alone, where dd-attest hands in build results (POST /built); empty: none")
	report := flag.String("sign-report", "", "sign the box's report in this directory (hardware.json, hardware-configuration.nix) as -box, into report.sig, and exit")
	join := flag.Bool("sign-join", false, "print a request for a join key to the fleet's network, signed as -box now, and exit")
	flag.Parse()
	log.SetFlags(0)

	if *printKey {
		k, err := validatorKey(filepath.Join(*dir, "validator.key"))
		if err != nil {
			log.Fatal(err)
		}
		fmt.Println(base64.StdEncoding.EncodeToString(k.Public().(ed25519.PublicKey)))
		return
	}

	if *join {
		now := time.Now().Unix()
		sign := func(m []byte) (string, error) { return tpmSign(*tpmKeys, m) }
		if *softKey != "" {
			sign = softSigner(*softKey)
		}
		sig, err := sign(joinMessage(*box, now))
		if err != nil {
			log.Fatal(err)
		}
		_ = json.NewEncoder(os.Stdout).Encode(map[string]any{"box": *box, "time": now, "sig": sig})
		return
	}

	if *report != "" {
		msg, err := reportMessage(*box, *report)
		if err != nil {
			log.Fatal(err)
		}
		sign := func(m []byte) (string, error) { return tpmSign(*tpmKeys, m) }
		if *softKey != "" {
			sign = softSigner(*softKey)
		}
		sig, err := sign(msg)
		if err != nil {
			log.Fatal(err)
		}
		if err := os.WriteFile(filepath.Join(*report, "report.sig"), []byte(sig+"\n"), 0o644); err != nil {
			log.Fatal(err)
		}
		return
	}

	raw, err := os.ReadFile(*configPath)
	if err != nil {
		log.Fatal(err)
	}
	cfg := &state.Config{}
	if err := json.Unmarshal(raw, cfg); err != nil {
		log.Fatal(err)
	}
	me, ok := cfg.Boxes[*box]
	// a box the release does not list joins on its own key
	var joining *state.JoinReq
	if !ok {
		if *joinKey == "" {
			log.Fatalf("%s is not in the config, and has no key to join with (-join-key)", *box)
		}
		k, err := os.ReadFile(*joinKey)
		if err != nil {
			log.Fatal(err)
		}
		spki := strings.TrimSpace(string(k))
		name, id, err := state.JoinName(spki)
		if err != nil {
			log.Fatal(err)
		}
		if name != *box {
			log.Fatalf("this box's key joins as %s, not %s", name, *box)
		}
		if *joinRecipientFile != "" {
			b, err := os.ReadFile(*joinRecipientFile)
			if err != nil {
				log.Fatal(err)
			}
			*joinRecipient = strings.TrimSpace(string(b))
		}
		me = state.Box{ID: id, House: *joinHouse, Key: spki}
		joining = &state.JoinReq{Key: spki, Recipient: *joinRecipient, House: *joinHouse}
		if *joinTicket != "" {
			if t, err := os.ReadFile(*joinTicket); err == nil {
				joining.Ticket = strings.TrimSpace(string(t))
			}
		}
	}
	var mine *state.Validator
	for i := range cfg.Validators {
		if cfg.Validators[i].Name == *box {
			mine = &cfg.Validators[i]
		}
	}
	self := me.ID
	if mine != nil {
		self = mine.ID // a voting box is addressed as its validator
	}
	sign := func(m []byte) (string, error) { return tpmSign(*tpmKeys, m) }
	if *softKey != "" {
		sign = softSigner(*softKey)
	}
	r := &relay{url: *relayURL, self: self, hello: sign}
	if joining != nil {
		r.key = joining.Key
	}

	var n node
	var ask func()
	if mine != nil {
		key, err := validatorKey(filepath.Join(*dir, "validator.key"))
		if err != nil {
			log.Fatal(err)
		}
		if base64.StdEncoding.EncodeToString(key.Public().(ed25519.PublicKey)) != mine.Key {
			log.Fatalf("this box's validator key is not the one the config names")
		}
		// made the first time, reopened and replayed after a restart
		w, walEntries, err := wal.InitializeAndReadAll(logger{}, filepath.Join(*dir, "wal"), &wal.Options{Metrics: wal.NewMetrics(&disabled.Provider{}, "control")})
		if err != nil {
			log.Fatal(err)
		}
		var saved *validator.Saved
		if b, err := os.ReadFile(filepath.Join(*dir, "saved.json")); err == nil {
			saved = &validator.Saved{}
			if json.Unmarshal(b, saved) != nil {
				saved = nil
			}
		}
		v, err := validator.New(validator.Options{
			Config: cfg, ID: mine.ID, Key: key, Transport: r, Logger: logger{}, WAL: w, WALEntries: walEntries, Saved: saved,
			Persist: func(s validator.Saved) {
				b, _ := json.Marshal(s)
				tmp := filepath.Join(*dir, "saved.json.tmp")
				if os.WriteFile(tmp, b, 0o600) == nil {
					_ = os.Rename(tmp, filepath.Join(*dir, "saved.json"))
				}
			},
		})
		if err != nil {
			log.Fatal(err)
		}
		n = v
		ask = func() {}
		r.hello = func(m []byte) (string, error) {
			return base64.RawURLEncoding.EncodeToString(ed25519.Sign(key, m)), nil
		}
		go func() {
			// start once the relay is up, so the first catch-up is heard
			for {
				r.mu.Lock()
				up := r.conn != nil
				r.mu.Unlock()
				if up {
					break
				}
				time.Sleep(200 * time.Millisecond)
			}
			if err := v.Start(); err != nil {
				log.Fatal(err)
			}
			log.Printf("validator %d voting", mine.ID)
		}()
	} else {
		rd, err := validator.NewReader(cfg, r)
		if err != nil {
			log.Fatal(err)
		}
		n = rd
		ask = rd.Ask
	}
	go r.run(n.Receive, ask)

	// Everything this box asks the validators, signed by the TPM, under one
	// rising sequence: a heartbeat and a directory item never share a number
	var seqMu sync.Mutex
	seq := uint64(time.Now().UnixMilli()) // above any earlier run's
	submit := func(kind string, body []byte) (uint64, error) {
		seqMu.Lock()
		seq++
		q := state.Request{Kind: kind, Box: *box, Seq: seq, Body: body}
		seqMu.Unlock()
		sig, err := sign(q.Bytes())
		if err != nil {
			return 0, err
		}
		q.Sig = sig
		b, _ := json.Marshal(q)
		r.Send(wire.ToValidators, wire.Submit, b)
		if mine != nil {
			n.Receive(self, wire.Submit, b) // our own validator too
		}
		return q.Seq, nil
	}

	// on the box running the network's control server: where each box that
	// joined on its own key is on the network, as headscale gave it, into
	// the agreed state - its store's name points there, never at what the
	// box says of itself
	if joining == nil {
		go func() {
			for {
				time.Sleep(time.Minute)
				out, err := exec.Command("headscale", "nodes", "list", "-o", "json").Output()
				if err != nil {
					continue // no network server here
				}
				s := n.State()
				if s == nil {
					continue
				}
				for name, ip := range joinedAddrs(out) {
					if j := s.Joined[name]; j != nil && j.IPv4 != ip {
						body, _ := json.Marshal(state.NetAddr{Name: name, IPv4: ip})
						if _, err := submit("netaddr", body); err != nil {
							log.Printf("netaddr: %v", err)
						}
					}
				}
			}
		}()
	}

	// a box the release does not list asks to join until the agreed state
	// has it; its heartbeats count from then
	if joining != nil {
		body, _ := json.Marshal(joining)
		go func() {
			for {
				if s := n.State(); s != nil && s.Joined[*box] != nil {
					log.Printf("joined the fleet as %s", *box)
					return
				}
				if _, err := submit("join", body); err != nil {
					log.Printf("join: %v", err)
				}
				time.Sleep(time.Minute)
			}
		}()
	}

	// the placed services this box carries, switched by the plan below
	carried, err := loadCarries(*placedCarries)
	if err != nil {
		log.Fatalf("%s: %v", *placedCarries, err)
	}
	placed := &placer{
		box: *box, dir: *placedDir, carries: carried, leased: map[string]bool{},
		run: systemctl, uptime: uptime, ttl: time.Duration(cfg.Rules.LeaseTTL) * time.Millisecond,
		since: map[string]int64{}, sent: map[string]map[uint64]int64{},
		memory: map[string]float64{}, free: memFree, refused: map[string]int64{},
		copies: *placedCopies,
		ask: func(op state.LeaseOp) (uint64, error) {
			body, _ := json.Marshal(op)
			return submit("lease", body)
		},
	}

	// heartbeats; a reader also asks for the latest each time
	go func() {
		for {
			f := facts()
			for k, v := range placed.facts() {
				f[k] = v
			}
			body, _ := json.Marshal(state.Heartbeat{Facts: f, Roles: splitRoles(*roles)})
			if _, err := submit("heartbeat", body); err != nil {
				log.Printf("heartbeat: %v", err)
			}
			ask()
			time.Sleep(*every)
		}
	}()

	// The gate's way in: a directory item, sent as this box's request. Only
	// the gate may open the socket; the validators bound what any box adds
	// (state.DirPerMinute), and the gates judge what it says.
	if *submitSock != "" {
		_ = os.Remove(*submitSock)
		l, err := net.Listen("unix", *submitSock)
		if err != nil {
			log.Fatal(err)
		}
		g, err := user.LookupGroup(*submitGroup)
		if err != nil {
			log.Fatal(err)
		}
		gid, _ := strconv.Atoi(g.Gid)
		// already ours (a test running as the gate's group): nothing to change
		if gid != os.Getgid() {
			if err := os.Chown(*submitSock, -1, gid); err != nil {
				log.Fatal(err)
			}
		}
		if err := os.Chmod(*submitSock, 0o660); err != nil {
			log.Fatal(err)
		}
		mux := http.NewServeMux()
		mux.HandleFunc("POST /submit", func(w http.ResponseWriter, req *http.Request) {
			body, err := io.ReadAll(io.LimitReader(req.Body, state.MaxDirItem+1))
			if err != nil || len(body) > state.MaxDirItem || !json.Valid(body) {
				http.Error(w, "a directory item: json, at most 64K", http.StatusBadRequest)
				return
			}
			if _, err := submit("directory", body); err != nil {
				http.Error(w, err.Error(), http.StatusServiceUnavailable)
				return
			}
			w.WriteHeader(http.StatusAccepted)
		})
		go func() {
			s := &http.Server{Handler: mux, ReadHeaderTimeout: 5 * time.Second}
			if err := s.Serve(l); err != nil && !errors.Is(err, http.ErrServerClosed) {
				log.Fatal(err)
			}
		}()
	}

	// dd-attest's way in: what this box built for a build order, signed by
	// its host key inside, sent as this box's request. Root's alone: only
	// what runs as root here holds the host key anyway.
	if *builtSock != "" {
		_ = os.Remove(*builtSock)
		l, err := net.Listen("unix", *builtSock)
		if err != nil {
			log.Fatal(err)
		}
		if err := os.Chmod(*builtSock, 0o600); err != nil {
			log.Fatal(err)
		}
		mux := http.NewServeMux()
		mux.HandleFunc("POST /built", func(w http.ResponseWriter, req *http.Request) {
			body, err := io.ReadAll(io.LimitReader(req.Body, state.MaxBuilt+1))
			if err != nil || len(body) > state.MaxBuilt || !json.Valid(body) {
				http.Error(w, "a build result: json, at most 16K", http.StatusBadRequest)
				return
			}
			if _, err := submit("built", body); err != nil {
				http.Error(w, err.Error(), http.StatusServiceUnavailable)
				return
			}
			w.WriteHeader(http.StatusAccepted)
		})
		go func() {
			s := &http.Server{Handler: mux, ReadHeaderTimeout: 5 * time.Second}
			if err := s.Serve(l); err != nil && !errors.Is(err, http.ErrServerClosed) {
				log.Fatal(err)
			}
		}()
	}

	http.HandleFunc("/state", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(n.State())
	})
	var ids *ident.Server
	if cfg.Identity != nil {
		ids = ident.NewServer(cfg.Identity, cfg.BoxVoters())
	}
	// what the fleet would run, from the agreed state now; a dry run - no
	// box acts on it yet
	if *placement != "" {
		raw, err := os.ReadFile(*placement)
		if err != nil {
			log.Fatal(err)
		}
		in := place.Inputs{}
		if err := json.Unmarshal(raw, &in); err != nil {
			log.Fatalf("%s: %v", *placement, err)
		}
		http.HandleFunc("/plan", func(w http.ResponseWriter, _ *http.Request) {
			w.Header().Set("content-type", "application/json")
			s := n.State()
			_ = json.NewEncoder(w).Encode(place.Decide(in, s, cfg.Rules.GoneAfter, memberBoxes(ids, s)))
		})
		// and carried out here, for what this box carries: only from a plan
		// made of agreement still fresh. Without it, what runs keeps running,
		// but a writer only while its lease may still be this box's
		for svc := range carried {
			placed.leased[svc] = in.Services[svc].Lease
			placed.memory[svc] = max(in.Services[svc].MemoryGiB, in.Services[svc].ReserveGiB)
		}
		if len(carried) > 0 || *placedCopies != "" {
			go func() {
				for {
					time.Sleep(placeEvery)
					placed.fence()
					s := n.State()
					if time.Now().UnixMilli()-s.Time > freshFor.Milliseconds() {
						continue
					}
					placed.reconcile(place.Decide(in, s, cfg.Rules.GoneAfter, memberBoxes(ids, s)), s)
				}
			}()
		}
	}
	// the directory log from item `since` on, and where the next will be
	http.HandleFunc("/directory", func(w http.ResponseWriter, req *http.Request) {
		since, _ := strconv.Atoi(req.URL.Query().Get("since"))
		s := n.State()
		items := []state.DirItem{}
		if since >= 0 && since < len(s.Directory) {
			items = s.Directory[since:]
		}
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"items": items, "next": len(s.Directory), "height": s.Height})
	})
	// the agreed identity state (docs/dev/identity-state.md): judged here
	// from the directory log, so every box serves the same
	if ids != nil {
		http.HandleFunc("/identity", func(w http.ResponseWriter, req *http.Request) {
			q := req.URL.Query()
			since, _ := strconv.ParseUint(q.Get("since"), 10, 64)
			s := n.State()
			code, b := ids.Answer(len(s.Directory), func(i int) ident.Item {
				d := s.Directory[i]
				return ident.Item{Time: d.Time, Box: d.Box, Item: d.Item}
			}, s.Height, s.Time, s.IdentBoxes(), q.Get("name"), since)
			w.Header().Set("content-type", "application/json")
			w.WriteHeader(code)
			_, _ = w.Write(b)
		})
	}
	// the build results from item `since` on, the same way
	http.HandleFunc("/built", func(w http.ResponseWriter, req *http.Request) {
		since, _ := strconv.Atoi(req.URL.Query().Get("since"))
		s := n.State()
		items := []state.BuiltItem{}
		if since >= 0 && since < len(s.Built) {
			items = s.Built[since:]
		}
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(map[string]any{"items": items, "next": len(s.Built), "height": s.Height, "time": s.Time})
	})
	srv := &http.Server{Addr: *listen, ReadHeaderTimeout: 5 * time.Second}
	if err := srv.ListenAndServe(); err != nil && !errors.Is(err, http.ErrServerClosed) {
		log.Fatal(err)
	}
}

// memberBoxes: the boxes members have enlisted that qualify, from the
// agreed identity state at s - placement's peers beside the release's
func memberBoxes(ids *ident.Server, s *state.State) map[string]bool {
	out := map[string]bool{}
	if ids == nil {
		return out
	}
	v := ids.View(len(s.Directory), func(i int) ident.Item {
		d := s.Directory[i]
		return ident.Item{Time: d.Time, Box: d.Box, Item: d.Item}
	}, s.Height, s.Time, s.IdentBoxes())
	for _, m := range v.Members {
		if m.Status != "member" {
			continue
		}
		for _, b := range m.Enlisted {
			if b.Qualifies != "" {
				out[b.Box] = true
			}
		}
	}
	return out
}

func splitRoles(s string) []string {
	var out []string
	for _, r := range strings.Split(s, ",") {
		if r = strings.TrimSpace(r); r != "" {
			out = append(out, r)
		}
	}
	return out
}

// softSigner: for tests on machines without this box's TPM
func softSigner(path string) func([]byte) (string, error) {
	return func(m []byte) (string, error) {
		der, err := os.ReadFile(path)
		if err != nil {
			return "", err
		}
		k, err := x509.ParsePKCS8PrivateKey(der)
		if err != nil {
			return "", err
		}
		ek, ok := k.(*ecdsa.PrivateKey)
		if !ok {
			return "", errors.New("not a P-256 key")
		}
		h := sha256.Sum256(m)
		sig, err := ecdsa.SignASN1(rand.Reader, ek, h[:])
		if err != nil {
			return "", err
		}
		return base64.RawURLEncoding.EncodeToString(sig), nil
	}
}

var joinedName = regexp.MustCompile(`^j[0-9a-f]{10}$`)

// joinedAddrs: from headscale's node list, each node tagged a joined box,
// by the user the gate made it under (joined-<name>, when it handed the box
// its key) - never by the hostname a node gives itself - and its IPv4
// address
func joinedAddrs(list []byte) map[string]uint32 {
	var nodes []struct {
		User struct {
			Name string `json:"name"`
		} `json:"user"`
		IPAddresses []string `json:"ipAddresses"`
		Tags        []string `json:"tags"`
		ValidTags   []string `json:"validTags"`
		ForcedTags  []string `json:"forcedTags"`
	}
	out := map[string]uint32{}
	if json.Unmarshal(list, &nodes) != nil {
		return out
	}
	for _, nd := range nodes {
		name, ok := strings.CutPrefix(nd.User.Name, "joined-")
		if !ok || !joinedName.MatchString(name) {
			continue
		}
		tagged := false
		for _, t := range append(append(nd.Tags, nd.ValidTags...), nd.ForcedTags...) {
			tagged = tagged || t == "tag:joined"
		}
		if !tagged {
			continue
		}
		for _, a := range nd.IPAddresses {
			if ip := net.ParseIP(a).To4(); ip != nil {
				out[name] = binary.BigEndian.Uint32(ip)
				break
			}
		}
	}
	return out
}
