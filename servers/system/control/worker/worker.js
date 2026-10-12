// The control plane's relay, at control.<domain>: one Durable Object every
// box and validator dials out to. It forwards frames by address and cannot
// forge any (they are signed by their senders); it can only delay them. The
// same object runs the witness - a validator compiled to wasm - while there
// are fewer than four houses (servers/system/control/cmd/witness).
//
// A connection says who it is with a signed hello: its address and the
// time, signed by its validator key (ed25519) or its box's TPM key (P-256),
// both from the signed config. A hello older than a minute is refused, so
// nobody can sit on another node's address.
//
// Its agreed state's leases go to the DNS Worker, which binds this object
// (/leases), and nowhere else.
//
// Bindings: RELAY (this class), WITNESS_KEY (secret, the witness's ed25519
// seed; unset: no witness here).
import './wasm_exec.js';
import witnessWasm from './witness.wasm';
import CONFIG from './config.js';

const TO_VALIDATORS = 2n ** 50n;
const TO_ALL = 2n ** 50n + 1n;
const SKEW = 60;

const b64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const b64url = (s) => b64(s.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - (s.length % 4)) % 4));

// r||s, which WebCrypto wants, from the DER a TPM may give
function rawSig(sig) {
  if (sig.length === 64) return sig;
  let i = 2;
  const part = () => {
    const n = sig[i + 1];
    let v = sig.slice(i + 2, i + 2 + n);
    i += 2 + n;
    while (v.length > 32 && v[0] === 0) v = v.slice(1);
    const out = new Uint8Array(32);
    out.set(v, 32 - v.length);
    return out;
  };
  const out = new Uint8Array(64);
  out.set(part());
  out.set(part(), 32);
  return out;
}

// who may connect as what: validators by their ids, boxes by theirs
const validators = new Map(CONFIG.validators.map((v) => [BigInt(v.id), v]));
const boxes = new Map(Object.values(CONFIG.boxes).map((b) => [BigInt(b.id), b]));

// the relay address a key joins under (state.JoinName): 2^40 plus the
// first five bytes of the sha256 of its SPKI
const JOINED_BASE = 1n << 40n;
async function joinID(spki) {
  const h = new Uint8Array(await crypto.subtle.digest('SHA-256', b64(spki)));
  let n = 0n;
  for (let i = 0; i < 5; i++) n = (n << 8n) | BigInt(h[i]);
  return JOINED_BASE | n;
}

// who may connect as id: a validator, a box of the release, a box that
// joined (the witness's agreed state), or a box joining on the key it
// shows, whose address that key gives. Says 'joining' for the last, which
// the relay caps.
async function helloOK(id, at, sig, key, joined) {
  if (Math.abs(Date.now() / 1000 - at) > SKEW) return false;
  const msg = new TextEncoder().encode(`commonty relay v1\0${id}\0${at}`);
  const p256 = async (spki) => {
    const k = await crypto.subtle.importKey('spki', b64(spki), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['verify']);
    return crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, k, rawSig(b64url(sig)), msg);
  };
  try {
    const v = validators.get(id);
    if (v) {
      const k = await crypto.subtle.importKey('raw', b64(v.key), { name: 'Ed25519' }, false, ['verify']);
      return await crypto.subtle.verify({ name: 'Ed25519' }, k, b64url(sig), msg);
    }
    const b = boxes.get(id);
    if (b) return await p256(b.key);
    const j = joined.get(id);
    if (j) return await p256(j.key);
    if (key && id >= JOINED_BASE && (await joinID(key)) === id && (await p256(key))) return 'joining';
  } catch {
    return false;
  }
  return false;
}

// boxes connected that have not joined yet, at most this many at once
const MAX_JOINING = 8;

function frame(to, from, kind, payload) {
  const b = new Uint8Array(17 + payload.length);
  const v = new DataView(b.buffer);
  v.setBigUint64(0, BigInt(to));
  v.setBigUint64(8, BigInt(from));
  b[16] = kind;
  b.set(payload, 17);
  return b;
}

export class Relay {
  constructor(ctx, env) {
    this.ctx = ctx;
    this.env = env;
    this.socks = new Map(); // id (BigInt) -> WebSocket
    this.witness = null; // its id, once running
    this.booting = null;
  }

  // the witness, if this deployment has its key
  boot() {
    this.booting ??= (async () => {
      if (!this.env.WITNESS_KEY) return;
      const saved = (await this.ctx.storage.get('saved')) || '';
      globalThis.controlInit = { config: JSON.stringify(CONFIG), key: this.env.WITNESS_KEY, saved };
      globalThis.controlSend = (to, kind, arr) => this.route(BigInt(to), this.witness, kind, arr);
      globalThis.controlPersist = (s) => { this.ctx.storage.put('saved', s); };
      const ready = new Promise((r) => { globalThis.controlReady = r; });
      const go = new Go();
      const inst = await WebAssembly.instantiate(witnessWasm, go.importObject);
      go.run(inst);
      this.witness = BigInt(await ready);
    })();
    return this.booting;
  }

  deliver(id, from, kind, payload) {
    if (id === from) return;
    if (id === this.witness) {
      globalThis.controlReceive(Number(from), kind, payload);
      return;
    }
    const s = this.socks.get(id);
    if (s) { try { s.send(frame(id, from, kind, payload)); } catch { /* gone */ } }
  }

  route(to, from, kind, payload) {
    if (to === TO_VALIDATORS || to === TO_ALL) {
      const targets = new Set(to === TO_ALL ? [...this.socks.keys()] : []);
      for (const id of validators.keys()) targets.add(id);
      if (this.witness !== null) targets.add(this.witness);
      for (const id of targets) this.deliver(id, from, kind, payload);
    } else {
      this.deliver(to, from, kind, payload);
    }
  }

  async fetch(req) {
    await this.boot();
    const url = new URL(req.url);
    if (url.pathname === '/health') {
      let height = null;
      try { height = this.witness !== null ? JSON.parse(globalThis.controlState()).height : null; } catch { /* starting */ }
      return Response.json({ connected: this.socks.size, witness: this.witness !== null, height });
    }
    // who holds each lease, by the witness's agreed state: for the DNS
    // Worker, which binds this object (servers/system/dns); never reached
    // from outside, as the front below passes only /relay and /health
    if (url.pathname === '/leases') {
      try {
        const s = JSON.parse(globalThis.controlState());
        // and the boxes that joined on their own keys, at the address each
        // gives in its heartbeat
        const joined = {};
        for (const name of Object.keys(s.joined || {})) {
          const ip = ((s.boxes || {})[name] || {}).facts?.net_ipv4;
          if (typeof ip === 'number') joined[name] = { ipv4: ip };
        }
        return Response.json({ time: s.time, leases: s.leases || {}, joined });
      } catch {
        return new Response('no state yet', { status: 503 });
      }
    }
    if (url.pathname !== '/relay' || req.headers.get('upgrade') !== 'websocket') return new Response('not here', { status: 404 });
    let id;
    try { id = BigInt(url.searchParams.get('id') || ''); } catch { return new Response('no id', { status: 400 }); }
    // the boxes that joined, by the witness's agreed state
    const joined = new Map();
    try {
      const s = this.witness !== null ? JSON.parse(globalThis.controlState()) : {};
      for (const j of Object.values(s.joined || {})) joined.set(BigInt(j.id), j);
    } catch { /* starting */ }
    const ok = await helloOK(id, Number(url.searchParams.get('at')), url.searchParams.get('sig') || '', url.searchParams.get('key') || '', joined);
    if (!ok) {
      return new Response('that hello is not signed by whoever has that address', { status: 403 });
    }
    if (ok === 'joining') {
      this.joining ??= new Set();
      for (const j of this.joining) if (!this.socks.has(j)) this.joining.delete(j);
      if (!this.joining.has(id) && this.joining.size >= MAX_JOINING) {
        return new Response('too many boxes joining at once', { status: 429 });
      }
      this.joining.add(id);
    }
    const [client, server] = Object.values(new WebSocketPair());
    server.accept();
    const old = this.socks.get(id);
    if (old) { try { old.close(4000, 'replaced'); } catch { /* gone */ } }
    this.socks.set(id, server);
    // in order: a Blob (how the local runtime hands binary over) is read first
    let queue = Promise.resolve();
    server.addEventListener('message', (ev) => {
      queue = queue.then(async () => {
        const b = new Uint8Array(ev.data instanceof Blob ? await ev.data.arrayBuffer() : ev.data);
        if (b.length < 17) return;
        const v = new DataView(b.buffer, b.byteOffset);
        // a frame claims to be from its own connection, whatever it says
        this.route(v.getBigUint64(0), id, b[16], b.subarray(17));
      });
    });
    server.addEventListener('close', () => { if (this.socks.get(id) === server) this.socks.delete(id); });
    return new Response(null, { status: 101, webSocket: client });
  }
}

export default {
  async fetch(req, env) {
    // only a WebSocket's hello comes here, with no body; anything sent with
    // one is read and turned away, as a body left unread faults the request
    // after it (a 503)
    const path = new URL(req.url).pathname;
    if (req.method !== 'GET' || (path !== '/relay' && path !== '/health')) {
      await req.text().catch(() => '');
      return new Response('not here', { status: 404 });
    }
    return env.RELAY.get(env.RELAY.idFromName('relay')).fetch(req);
  },
};
