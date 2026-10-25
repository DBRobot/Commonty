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
// Its agreed state's leases, and the boxes the agreed plan gives each placed
// service, go to the DNS Worker, which binds this object (/leases), and
// nowhere else.
//
// Bindings: RELAY (this class), WITNESS_KEY (secret, the witness's ed25519
// seed; unset: no witness here).
import './wasm_exec.js';
import witnessWasm from './witness.wasm';
import CONFIG from './config.js';
import PLACEMENT from './placement.js';

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

// A copy of the newest box release, beside the forge's, for a box whose
// forge is down (docs/dev/releases.md, "Where a box finds the release").
// Anyone may hand one in, the laptop or any box or not: it is kept only if
// the release key signed it and it is newer than the one kept, and kept as
// rebuilt from what was verified. So the most a stranger can do is hand in
// a release a little early; nothing it sends can stand in for another.
const RELEASE_MAX = 1 << 20;

// what the release key signs (release::canonical_v2): every key sorted, no
// whitespace. A payload this cannot write back byte for byte (a float, a
// key past the basic plane) fails the signature, and is refused
function canonical(v) {
  if (Array.isArray(v)) return `[${v.map(canonical).join(',')}]`;
  if (v !== null && typeof v === 'object') {
    return `{${Object.keys(v).sort().map((k) => `${JSON.stringify(k)}:${canonical(v[k])}`).join(',')}}`;
  }
  return JSON.stringify(v);
}

// JSON as a release is written, and nothing looser: no key twice in an
// object, every number an integer. A file read more ways than one could
// verify as one thing and be served as another
const STR = /"(?:[^"\\]|\\["\\/bfnrt]|\\u[0-9a-fA-F]{4})*"/y;
const INT = /-?(?:0|[1-9][0-9]*)(?![0-9.eE])/y;
function strictParse(text) {
  let i = 0;
  const fail = (why) => { throw new Error(why); };
  const ws = () => { while (i < text.length && ' \t\n\r'.includes(text[i])) i++; };
  const str = () => {
    STR.lastIndex = i;
    const m = STR.exec(text);
    if (!m) fail('a bad string');
    // a control character is written escaped, never as itself
    for (let k = 0; k < m[0].length; k++) if (m[0].charCodeAt(k) < 0x20) fail('a bad string');
    i += m[0].length;
    return JSON.parse(m[0]);
  };
  const val = () => {
    ws();
    const c = text[i];
    if (c === '{') {
      i++;
      const o = {};
      ws();
      if (text[i] === '}') { i++; return o; }
      for (;;) {
        ws();
        const k = str();
        if (Object.prototype.hasOwnProperty.call(o, k)) fail(`${k} twice`);
        ws();
        if (text[i++] !== ':') fail('no colon');
        o[k] = val();
        ws();
        const d = text[i++];
        if (d === '}') return o;
        if (d !== ',') fail('no comma');
      }
    }
    if (c === '[') {
      i++;
      const a = [];
      ws();
      if (text[i] === ']') { i++; return a; }
      for (;;) {
        a.push(val());
        ws();
        const d = text[i++];
        if (d === ']') return a;
        if (d !== ',') fail('no comma');
      }
    }
    if (c === '"') return str();
    for (const [w, v] of [['true', true], ['false', false], ['null', null]]) {
      if (text.startsWith(w, i)) { i += w.length; return v; }
    }
    INT.lastIndex = i;
    const m = INT.exec(text);
    if (!m) fail('not an integer');
    i += m[0].length;
    const n = Number(m[0]);
    if (!Number.isSafeInteger(n)) fail('an integer too big');
    return n;
  };
  const v = val();
  ws();
  if (i !== text.length) fail('more after the document');
  return v;
}

// A release signed by the release key, as the document this relay keeps:
// rebuilt from what was verified, every key sorted, so two encodings of
// one signed release are one document. Or why it is not one
async function releaseDoc(text) {
  const key = CONFIG.identity && CONFIG.identity.release;
  if (!key) return [null, 'this relay has no release key'];
  let v;
  try { v = strictParse(text); } catch (e) { return [null, `not a release file: ${e.message}`]; }
  if (!v || typeof v !== 'object' || Array.isArray(v)) return [null, 'not a release file'];
  if (Object.keys(v).sort().join() !== 'payload,signature,signer') return [null, 'a release file is payload, signer and signature'];
  const p = v.payload;
  // a box release: an app manifest or any other signed document names its kind
  if (!p || typeof p !== 'object' || Array.isArray(p) || 'kind' in p) return [null, 'not a box release'];
  if (!Number.isSafeInteger(p.counter) || p.counter < 1) return [null, 'no counter'];
  if (v.signer !== key) return [null, 'not signed by the release key'];
  if (typeof v.signature !== 'string') return [null, 'no signature'];
  try {
    const k = await crypto.subtle.importKey('raw', b64(key), { name: 'Ed25519' }, false, ['verify']);
    if (await crypto.subtle.verify({ name: 'Ed25519' }, k, b64(v.signature), new TextEncoder().encode(canonical(p)))) {
      return [{ counter: p.counter, text: canonical({ payload: p, signature: v.signature, signer: v.signer }) }, null];
    }
  } catch { /* a signature that is not base64 */ }
  return [null, 'the signature does not hold'];
}

function frame(to, from, kind, payload) {
  const b = new Uint8Array(17 + payload.length);
  const v = new DataView(b.buffer);
  v.setBigUint64(0, BigInt(to));
  v.setBigUint64(8, BigInt(from));
  b[16] = kind;
  b.set(payload, 17);
  return b;
}

// The object every connection and Worker reaches. 'relay' wedged on its
// witness's saved state on 2026-10-22 and answered nothing, however often
// the code was replaced; the DNS and mail Workers bind this name too
// (servers/system/dns/worker.js, client/mail/worker/identity.js)
const RELAY_NAME = 'relay-1023';

// how long the witness may take to start before the relay gives up on it
const WITNESS_BOOT_MS = 20000;
// how long after a failed start the witness is tried again
const WITNESS_RETRY_MS = 60000;

// what the witness registers, and the relay calls
const HOOKS = ['controlState', 'controlIdentity', 'controlPlaced', 'controlReceive', 'controlSend', 'controlPersist', 'controlReady'];

function unhook() {
  for (const h of HOOKS) delete globalThis[h];
  // a witness that hung may still call out: nothing it says goes anywhere
  globalThis.controlSend = () => {};
  globalThis.controlPersist = () => {};
  globalThis.controlReady = () => {};
}

// the validator id a witness key votes as, from the config: known before
// the witness is ready, so its first catching up is answered
async function witnessID(seed) {
  try {
    const pkcs8 = new Uint8Array([...b64('MC4CAQAwBQYDK2VwBCIEIA=='), ...b64(seed)]);
    const k = await crypto.subtle.importKey('pkcs8', pkcs8, { name: 'Ed25519' }, true, ['sign']);
    const pub = btoa(String.fromCharCode(...b64url((await crypto.subtle.exportKey('jwk', k)).x)));
    const v = CONFIG.validators.find((x) => x.key === pub);
    return v ? BigInt(v.id) : null;
  } catch {
    return null;
  }
}

export class Relay {
  // for a test's front, which reaches the same object
  static NAME = RELAY_NAME;

  constructor(ctx, env) {
    this.ctx = ctx;
    this.env = env;
    this.socks = new Map(); // id (BigInt) -> WebSocket
    this.witness = null; // its id, once ready and while it runs
    this.self = null; // its id, from its start until it fails
    this.booting = null;
  }

  // the witness, if this deployment has its key. Nothing waits on it: the
  // relay carries the boxes' frames from the first moment, and a witness
  // that panics or hangs on what it saved only leaves the relay without it
  // (2026-10-22, when one that never started stopped every box's dial)
  boot() {
    if (!this.env.WITNESS_KEY) return null;
    // one that failed is tried again a minute on: the boxes keep this object
    // open, so it was otherwise never tried again (off from 2026-10-23 until
    // a redeploy, with nothing saying why)
    if (this.booting && this.failedAt && Date.now() - this.failedAt > WITNESS_RETRY_MS) {
      this.booting = null;
    }
    this.booting ??= this.start().then(
      () => { this.failedAt = 0; this.witnessError = null; },
      (e) => {
        console.log(`witness: ${e.message}; relaying without it`);
        this.failedAt = Date.now();
        this.witnessError = e.message;
        this.drop();
      },
    );
    return this.booting;
  }

  async start() {
    const saved = (await this.ctx.storage.get('saved')) || '';
    try {
      return await this.run(saved);
    } catch (e) {
      this.drop();
      if (saved === '') throw e;
      // what it saved is the one thing it brings: without it, it catches
      // up from the validators like one that was away. The delete is made
      // durable before anything else, or a reset drops it and the next
      // start meets the same state
      console.log(`witness: ${e.message}; starting again without what it saved`);
      await this.ctx.storage.delete('saved');
      await this.ctx.storage.sync();
      if (e.hung) {
        // a hung one cannot be stopped from here: the object starts afresh
        this.ctx.abort(e.message);
        return;
      }
    }
    return await this.run('');
  }

  // one start of the witness: resolves once it is ready, throws if it
  // exits first or is not ready in time
  async run(saved) {
    this.self = await witnessID(this.env.WITNESS_KEY);
    globalThis.controlInit = { config: JSON.stringify(CONFIG), key: this.env.WITNESS_KEY, saved, placement: JSON.stringify(PLACEMENT) };
    globalThis.controlSend = (to, kind, arr) => { if (this.self !== null) this.route(BigInt(to), this.self, kind, arr); };
    globalThis.controlPersist = (s) => { this.ctx.storage.put('saved', s); };
    const ready = new Promise((r) => { globalThis.controlReady = r; });
    const go = new Go();
    const inst = await WebAssembly.instantiate(witnessWasm, go.importObject);
    let up = false;
    const exited = go.run(inst).then(
      () => { throw new Error('the witness exited before it was ready'); },
      (e) => { throw new Error(`the witness failed: ${e && e.message}`); },
    );
    // and once it ran: gone, it is no witness, and the next request starts
    // one again (from what it saved since)
    exited.catch(() => {
      if (!up) return;
      console.log('witness: it exited; relaying without it until it starts again');
      up = false;
      this.booting = null;
      this.drop();
    });
    let timer;
    const late = new Promise((_, no) => {
      timer = setTimeout(() => no(Object.assign(new Error(`the witness was not ready in ${WITNESS_BOOT_MS / 1000} s`), { hung: true })), WITNESS_BOOT_MS);
    });
    try {
      const id = await Promise.race([ready, exited, late]);
      up = true;
      this.witness = BigInt(id);
    } finally {
      clearTimeout(timer);
    }
  }

  drop() {
    this.witness = null;
    this.self = null;
    unhook();
  }

  // the witness's own answers, only while it runs
  witnessUp() {
    return this.witness !== null && typeof globalThis.controlState === 'function';
  }

  deliver(id, from, kind, payload) {
    if (id === from) return;
    if (this.self !== null && id === this.self) {
      if (typeof globalThis.controlReceive !== 'function') return;
      try { globalThis.controlReceive(Number(from), kind, payload); } catch (e) { console.log(`witness: ${e.message}`); }
      return;
    }
    const s = this.socks.get(id);
    if (s) { try { s.send(frame(id, from, kind, payload)); } catch { /* gone */ } }
  }

  async release(req) {
    const kept = await this.ctx.storage.get('release');
    if (req.method === 'GET') {
      if (!kept) return new Response('none yet', { status: 404 });
      return new Response(kept.text, { headers: { 'content-type': 'application/json', 'cache-control': 'no-store' } });
    }
    // read whole even when too big: a body left unread faults the request
    // after it (a 503)
    const text = await req.text();
    if (text.length > RELEASE_MAX) return new Response('too big', { status: 413 });
    const [doc, why] = await releaseDoc(text);
    if (doc === null) return new Response(why, { status: 400 });
    const { counter } = doc;
    // read again: another may have been kept while this one was checked
    const now = await this.ctx.storage.get('release');
    if (now && now.counter > counter) return new Response(`release ${now.counter} is kept`, { status: 409 });
    // one counter is one release: the first kept stays. The same release
    // in other bytes is the same document here, and fine
    if (now && now.counter === counter) return new Response(`release ${counter}`, { status: now.text === doc.text ? 200 : 409 });
    await this.ctx.storage.put('release', doc);
    return new Response(`release ${counter}`, { status: 201 });
  }

  route(to, from, kind, payload) {
    if (to === TO_VALIDATORS || to === TO_ALL) {
      const targets = new Set(to === TO_ALL ? [...this.socks.keys()] : []);
      for (const id of validators.keys()) targets.add(id);
      if (this.self !== null) targets.add(this.self);
      for (const id of targets) this.deliver(id, from, kind, payload);
    } else {
      this.deliver(to, from, kind, payload);
    }
  }

  async fetch(req) {
    this.boot();
    const url = new URL(req.url);
    if (url.pathname === '/health') {
      let height = null;
      try { height = this.witnessUp() ? JSON.parse(globalThis.controlState()).height : null; } catch { /* starting */ }
      return Response.json({ connected: this.socks.size, witness: this.witnessUp(), height, witnessError: this.witnessUp() ? null : (this.witnessError ?? null) });
    }
    // what only the witness knows is never answered without it, nor from
    // whatever a witness that failed left behind
    if ((url.pathname === '/leases' || url.pathname === '/identity') && !this.witnessUp()) {
      return new Response('no witness here', { status: 503 });
    }
    // who holds each lease, by the witness's agreed state: for the DNS
    // Worker, which binds this object (servers/system/dns); never reached
    // from outside, as the front below passes only /relay and /health
    if (url.pathname === '/leases') {
      try {
        const s = JSON.parse(globalThis.controlState());
        // and the boxes that joined on their own keys, at the address the
        // network's control server gave each (never the box's own word)
        const joined = {};
        for (const [name, j] of Object.entries(s.joined || {})) {
          if (typeof j.ipv4 === 'number') joined[name] = { ipv4: j.ipv4 };
        }
        // and where the agreed plan puts each placed service: the
        // release's placement inputs planned by the witness on the agreed
        // state, as every box plans it. Never a box's own word
        let placed = {};
        try { if (typeof globalThis.controlPlaced === 'function') placed = JSON.parse(globalThis.controlPlaced()); } catch { placed = {}; }
        return Response.json({ time: s.time, leases: s.leases || {}, joined, placed });
      } catch {
        return new Response('no state yet', { status: 503 });
      }
    }
    // the agreed identity state, judged by the witness from the directory
    // log (docs/dev/identity-state.md): for the mail and DNS Workers, the
    // same way and from nowhere else
    if (url.pathname === '/identity') {
      if (typeof globalThis.controlIdentity !== 'function') return new Response('no identity here', { status: 503 });
      const [status, body] = globalThis.controlIdentity(url.searchParams.get('name') || '', Number(url.searchParams.get('since') || 0));
      if (status === 304) return new Response(null, { status });
      return new Response(body, { status, headers: { 'content-type': 'application/json' } });
    }
    if (url.pathname === '/release') return this.release(req);
    if (url.pathname !== '/relay' || req.headers.get('upgrade') !== 'websocket') return new Response('not here', { status: 404 });
    let id;
    try { id = BigInt(url.searchParams.get('id') || ''); } catch { return new Response('no id', { status: 400 }); }
    // the boxes that joined, by the witness's agreed state
    const joined = new Map();
    try {
      const s = this.witnessUp() ? JSON.parse(globalThis.controlState()) : {};
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
      }).catch(() => { /* one frame lost, never the connection's later ones */ });
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
    // and the release: read by any box, handed in by any box or publish
    if (path === '/release' && (req.method === 'GET' || req.method === 'PUT')) {
      return env.RELAY.get(env.RELAY.idFromName(RELAY_NAME)).fetch(req);
    }
    if (req.method !== 'GET' || (path !== '/relay' && path !== '/health')) {
      await req.text().catch(() => '');
      return new Response('not here', { status: 404 });
    }
    return env.RELAY.get(env.RELAY.idFromName(RELAY_NAME)).fetch(req);
  },
};
