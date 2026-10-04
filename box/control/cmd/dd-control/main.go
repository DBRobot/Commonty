// dd-control: a box's part in the control plane. Every box heartbeats -
// what it has and what it runs, signed by its TPM - and keeps the agreed
// state, checked against the validators' signatures, at
// http://127.0.0.1:4190/state for the box's own services. A box named as
// its house's validator also votes. It dials out to the relay and nothing
// dials in.
package main

import (
	"context"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/base64"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"log"
	"net/http"
	"os"
	"os/exec"
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
		c, _, err := websocket.Dial(context.Background(), fmt.Sprintf("%s?id=%d", r.url, r.self), nil)
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
	return f
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
	listen := flag.String("listen", "127.0.0.1:4190", "the agreed state, for this box")
	roles := flag.String("roles", "", "roles this box runs, comma separated")
	every := flag.Duration("heartbeat", 30*time.Second, "how often to heartbeat")
	printKey := flag.Bool("print-validator-key", false, "make the validator key if missing, print its public half, exit")
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
	r := &relay{url: *relayURL, self: self}

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
		w, err := wal.Create(logger{}, filepath.Join(*dir, "wal"), &wal.Options{Metrics: wal.NewMetrics(&disabled.Provider{}, "control")})
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
			Config: cfg, ID: mine.ID, Key: key, Transport: r, Logger: logger{}, WAL: w, Saved: saved,
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

	// heartbeats, signed by the TPM; a reader also asks for the latest each time
	go func() {
		var seq uint64 = uint64(time.Now().UnixMilli()) // above any earlier run's
		for {
			seq++
			body, _ := json.Marshal(state.Heartbeat{Facts: facts(), Roles: splitRoles(*roles)})
			q := state.Request{Kind: "heartbeat", Box: *box, Seq: seq, Body: body}
			if sig, err := tpmSign(*tpmKeys, q.Bytes()); err != nil {
				log.Printf("heartbeat: %v", err)
			} else {
				q.Sig = sig
				b, _ := json.Marshal(q)
				r.Send(wire.ToValidators, wire.Submit, b)
				if mine != nil {
					n.Receive(self, wire.Submit, b) // our own validator too
				}
			}
			ask()
			time.Sleep(*every)
		}
	}()

	http.HandleFunc("/state", func(w http.ResponseWriter, _ *http.Request) {
		w.Header().Set("content-type", "application/json")
		_ = json.NewEncoder(w).Encode(n.State())
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
