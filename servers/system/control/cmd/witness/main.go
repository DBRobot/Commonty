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

	"commonty/control/state"
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
	init := js.Global().Get("controlInit")
	cfg := &state.Config{}
	if err := json.Unmarshal([]byte(init.Get("config").String()), cfg); err != nil {
		panic(err)
	}
	seed, err := base64.StdEncoding.DecodeString(init.Get("key").String())
	if err != nil || len(seed) != ed25519.SeedSize {
		panic("the witness key is not an ed25519 seed")
	}
	key := ed25519.NewKeyFromSeed(seed)
	var id uint64
	for _, v := range cfg.Validators {
		if v.Key == base64.StdEncoding.EncodeToString(key.Public().(ed25519.PublicKey)) {
			id = v.ID
		}
	}
	if id == 0 {
		panic("the witness key is not a validator in the config")
	}
	var saved *validator.Saved
	if s := init.Get("saved"); s.Type() == js.TypeString && s.String() != "" {
		saved = &validator.Saved{}
		if json.Unmarshal([]byte(s.String()), saved) != nil {
			saved = nil
		}
	}
	v, err := validator.New(validator.Options{
		Config: cfg, ID: id, Key: key, Transport: jsTransport{}, Logger: logger{}, WAL: memWAL{}, Saved: saved,
		Persist: func(s validator.Saved) {
			b, _ := json.Marshal(s)
			js.Global().Call("controlPersist", string(b))
		},
	})
	if err != nil {
		panic(err)
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
			}, s.Height, s.Time, name, since)
			return js.ValueOf([]any{code, string(b)})
		}))
	}
	if err := v.Start(); err != nil {
		panic(err)
	}
	js.Global().Call("controlReady", int(id))
	select {}
}
