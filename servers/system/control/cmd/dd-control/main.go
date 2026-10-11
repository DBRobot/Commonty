// dd-control: a box's part in the control plane. Every box heartbeats -
// what it has and what it runs, signed by its TPM - and keeps the agreed
// state, checked against the validators' signatures, at
// http://127.0.0.1:4195/state for the box's own services. A box named as
// its house's validator also votes. It dials out to the relay and nothing
// dials in.
package main

import (
	"context"
	"crypto/ecdsa"
	"crypto/ed25519"
	"crypto/rand"
	"crypto/sha256"
	"crypto/x509"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"os"
	"os/exec"
	"os/user"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"

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
	mu    sync.Mutex
	conn  *websocket.Conn
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
		c, _, err := websocket.Dial(context.Background(), fmt.Sprintf("%s?id=%d&at=%d&sig=%s", r.url, r.self, at, sig), nil)
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
	usage, _ := filepath.Glob("/var/lib/dd-facts/usage-*.prom")
	for name, v := range promFacts(append([]string{"/var/lib/dd-facts/dd_agent.prom", "/var/lib/dd-facts/backup.prom"}, usage...)...) {
		f[name] = v
	}
	return f
}

// promFacts reads the few samples the heartbeat carries from text-format
// metric files: release (dd_agent_counter), agent_ok (dd_agent_info's
// result: 1 ok, 0 refused) and backup_last (dd_backup_last_success_seconds)
// maxFacts bounds what one heartbeat carries, members and services and all
const maxFacts = 512

// usageKey turns dd_usage{service="code",member="tom"} into "usage/code/tom",
// names of letters, digits, '.', '_' and '-' only
func usageKey(metric string) (string, bool) {
	labels := map[string]string{}
	inner := strings.TrimSuffix(strings.TrimPrefix(metric, "dd_usage{"), "}")
	for _, kv := range strings.Split(inner, ",") {
		k, v, ok := strings.Cut(kv, "=")
		if !ok {
			return "", false
		}
		labels[strings.TrimSpace(k)] = strings.Trim(strings.TrimSpace(v), `"`)
	}
	svc, member := labels["service"], labels["member"]
	ok := func(s string) bool {
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
	if !ok(svc) || !ok(member) {
		return "", false
	}
	return "usage/" + svc + "/" + member, true
}

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
			// what a member keeps in a service on this box, as the service
			// counted it (nix/modules/storage/usage.nix, the ledger): every
			// box's gate adds these up across the fleet for the allowance
			if strings.HasPrefix(l, "dd_usage{") {
				// the labels whole, up to their closing brace: a value with
				// a space in it is not split into a shorter, valid name
				end := strings.Index(l, "}")
				if end < 0 || strings.TrimSpace(l[end+1:]) != fs[len(fs)-1] {
					continue
				}
				if k, ok := usageKey(l[:end+1]); ok && len(out) < maxFacts {
					out[k] = v
				}
				continue
			}
			switch name := strings.SplitN(fs[0], "{", 2)[0]; name {
			case "dd_agent_counter":
				out["release"] = v
			case "dd_backup_last_success_seconds":
				out["backup_last"] = v
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
	every := flag.Duration("heartbeat", 30*time.Second, "how often to heartbeat")
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
	if !ok {
		log.Fatalf("%s is not in the config", *box)
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
	submit := func(kind string, body []byte) error {
		seqMu.Lock()
		seq++
		q := state.Request{Kind: kind, Box: *box, Seq: seq, Body: body}
		seqMu.Unlock()
		sig, err := sign(q.Bytes())
		if err != nil {
			return err
		}
		q.Sig = sig
		b, _ := json.Marshal(q)
		r.Send(wire.ToValidators, wire.Submit, b)
		if mine != nil {
			n.Receive(self, wire.Submit, b) // our own validator too
		}
		return nil
	}

	// heartbeats; a reader also asks for the latest each time
	go func() {
		for {
			body, _ := json.Marshal(state.Heartbeat{Facts: facts(), Roles: splitRoles(*roles)})
			if err := submit("heartbeat", body); err != nil {
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
			if err := submit("directory", body); err != nil {
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
			if err := submit("built", body); err != nil {
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
