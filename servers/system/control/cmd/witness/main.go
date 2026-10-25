//go:build js && wasm

// The witness: a validator inside the relay's Durable Object. The object
// gives it the config, its key and whatever it saved before
// (globalThis.controlInit), hands it frames (globalThis.controlReceive), and
// takes its frames and its saved state back (controlSend, controlPersist).
package main

import (
	"commonty/control/state/ident"
	"crypto/ed25519"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"syscall/js"

	"commonty/control/place"
	"commonty/control/state"
	"commonty/control/state/computers"
	"commonty/control/validator"
)

type logger struct{}

func (logger) Debugf(string, ...any)     {}
func (logger) Infof(string, ...any)      {}
func (logger) Warnf(f string, a ...any)  { fmt.Printf("witness warn: "+f+"\n", a...) }
func (logger) Errorf(f string, a ...any) { fmt.Printf("witness error: "+f+"\n", a...) }
func (logger) Panicf(f string, a ...any) { panic(fmt.Sprintf(f, a...)) }

type memWAL struct{}

func (memWAL) Append([]byte, bool) error { return nil }

type jsTransport struct{}

func (jsTransport) Send(to uint64, kind byte, p []byte) {
	arr := js.Global().Get("Uint8Array").New(len(p))
	js.CopyBytesToJS(arr, p)
	js.Global().Call("controlSend", js.ValueOf(float64(to)), int(kind), arr)
}

func main() {
	// why it stopped before it was ready, for the relay's health
	// (worker.js controlExit): the object's own log cannot be read
	defer func() {
		if r := recover(); r != nil {
			if f := js.Global().Get("controlExit"); f.Type() == js.TypeFunction {
				f.Invoke(fmt.Sprint(r))
			}
			panic(r)
		}
	}()
	init := js.Global().Get("controlInit")
	cfg := &state.Config{}
	if err := json.Unmarshal([]byte(init.Get("config").String()), cfg); err != nil {
		panic(fmt.Sprintf("the config: %v", err))
	}
	seed, err := base64.StdEncoding.DecodeString(init.Get("key").String())
	if err != nil {
		panic("the witness key is not base64")
	}
	if len(seed) != ed25519.SeedSize {
		panic(fmt.Sprintf("the witness key is %d bytes, not a 32-byte ed25519 seed", len(seed)))
	}
	key := ed25519.NewKeyFromSeed(seed)
	pub := base64.StdEncoding.EncodeToString(key.Public().(ed25519.PublicKey))
	var id uint64
	for _, v := range cfg.Validators {
		if v.Key == pub {
			id = v.ID
		}
	}
	// its public half, to set against fleet/control.json
	if id == 0 {
		panic(fmt.Sprintf("the witness key (public %s) is not a validator in the config", pub))
	}
	var saved *validator.Saved
	if s := init.Get("saved"); s.Type() == js.TypeString && s.String() != "" {
		saved = &validator.Saved{}
		if json.Unmarshal([]byte(s.String()), saved) != nil {
			saved = nil
		}
	}
	o := validator.Options{
		Config: cfg, ID: id, Key: key, Transport: jsTransport{}, Logger: logger{}, WAL: memWAL{}, Saved: saved,
		Persist: func(s validator.Saved) {
			b, _ := json.Marshal(s)
			js.Global().Call("controlPersist", string(b))
		},
	}
	v, err := validator.New(o)
	if err != nil && saved != nil {
		// another release's state hashes differently here (a field this one
		// does not know is dropped); panicking on it wedged the relay
		// (2026-10-22). It catches up from the validators instead
		fmt.Printf("witness warn: %v; starting without it, to catch up from the validators\n", err)
		o.Saved = nil
		v, err = validator.New(o)
	}
	if err != nil {
		panic(fmt.Sprintf("the validator: %v", err))
	}
	js.Global().Set("controlReceive", js.FuncOf(func(_ js.Value, a []js.Value) any {
		b := make([]byte, a[2].Get("length").Int())
		js.CopyBytesToGo(b, a[2])
		v.Receive(uint64(a[0].Float()), byte(a[1].Int()), b)
		return nil
	}))
	js.Global().Set("controlState", js.FuncOf(func(js.Value, []js.Value) any {
		b, _ := json.Marshal(v.State())
		return string(b)
	}))
	// the agreed identity state, for the Workers that bind the relay:
	// controlIdentity(name, since) -> [status, body]
	if cfg.Identity != nil {
		ids := ident.NewServer(cfg.Identity, cfg.BoxVoters())
		js.Global().Set("controlIdentity", js.FuncOf(func(_ js.Value, a []js.Value) any {
			name, since := "", uint64(0)
			if len(a) > 0 && a[0].Type() == js.TypeString {
				name = a[0].String()
			}
			if len(a) > 1 && a[1].Type() == js.TypeNumber {
				since = uint64(a[1].Float())
			}
			s := v.State()
			code, b := ids.Answer(len(s.Directory), func(i int) ident.Item {
				d := s.Directory[i]
				return ident.Item{Time: d.Time, Box: d.Box, Item: d.Item}
			}, s.Height, s.Time, s.IdentBoxes(), name, since)
			return js.ValueOf([]any{code, string(b)})
		}))
	}
	// the agreed placement: the release's placement inputs
	// (controlInit.placement) planned on the agreed state, as every box
	// plans it (dd-control -placement). For the DNS Worker, which points a
	// placed service's names at the boxes this plan gives it, and never at
	// a box on its own word: controlPlaced() -> {service: [box...]}
	if p := init.Get("placement"); p.Type() == js.TypeString && p.String() != "" {
		in := place.Inputs{}
		if err := json.Unmarshal([]byte(p.String()), &in); err != nil {
			panic(fmt.Sprintf("the placement inputs: %v", err))
		}
		var ids *ident.Server
		if cfg.Identity != nil {
			ids = ident.NewServer(cfg.Identity, cfg.BoxVoters())
		}
		var rc *computers.Server
		if cfg.Identity != nil && cfg.Computers != nil {
			rc = computers.NewServer(cfg.Identity, cfg.Computers, cfg.BoxVoters())
		}
		js.Global().Set("controlPlaced", js.FuncOf(func(js.Value, []js.Value) any {
			s := v.State()
			b, _ := json.Marshal(placed(in, s, cfg.Rules.GoneAfter, ids, rc))
			return string(b)
		}))
	}
	if err := v.Start(); err != nil {
		panic(fmt.Sprintf("starting: %v", err))
	}
	js.Global().Call("controlReady", int(id))
	select {}
}

// placed: of the plan at s, each placed service and the boxes it is given,
// sorted by box
func placed(in place.Inputs, s *state.State, goneAfter int64, ids *ident.Server, rc *computers.Server) map[string][]string {
	item := func(i int) ident.Item {
		d := s.Directory[i]
		return ident.Item{Time: d.Time, Box: d.Box, Item: d.Item}
	}
	if rc != nil {
		in.Computers = rc.Computers(len(s.Directory), item, s.Height, s.Time, s.IdentBoxes())
	}
	members := map[string]bool{}
	if ids != nil {
		for _, m := range ids.View(len(s.Directory), item, s.Height, s.Time, s.IdentBoxes()).Members {
			if m.Status != "member" {
				continue
			}
			for _, b := range m.Enlisted {
				if b.Qualifies != "" {
					members[b.Box] = true
				}
			}
		}
	}
	return place.Placed(in, place.Decide(in, s, goneAfter, members))
}
