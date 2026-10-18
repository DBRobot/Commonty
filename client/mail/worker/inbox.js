// A member's locked inbox (docs/dev/mail.md). Once a member's app has made
// their mail key and they have confirmed it here with their passkey, mail
// to their address and to their aliases is locked to that key as it
// arrives - the whole message, and apart from it a summary for their list -
// and held in the Hold Durable Object until one of the boxes takes it to
// keep. A box only ever holds what it cannot open. Forwarding to their own
// address goes on beside it unless they turn it off.
//
// KV (PINNED): mailkey:<name> {pk, sign: [{fp, key}], fwd, at}. Hold (one
// object, "hold"): the locked mail, in parts, until a box has it; each
// box's newest signed time.
//
// Devices: the app signs what it asks here with its device key, and only a
// device the member confirmed with their passkey on this Worker's own page
// is believed - a box's page can have a passkey sign many things, but never
// one made on this origin.

import { seal, b64u, tob64u } from './mail-hpke.js';
import { summary } from './mail-mime.js';
import { signedBy } from './unlock.js';
import { newId, ID } from './mail-ids.js';
import { pack, header as objectHeader, signEd25519 } from './mail-object.js';
import { liveAliases } from './aliases.js';
import { BOXES, MAIL_BOXES } from './fleet.js';

const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;
const PK = /^[A-Za-z0-9_-]{43}$/;
const ED = /^[A-Za-z0-9+/]{43}=$/;
const FP = /^[A-Za-z0-9_-]{4,64}$/;
const SKEW = 300;
const PART = 1 << 20;
const MAX_DEVICES = 8;
const enc = new TextEncoder();
const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');
const sha = async (b) => hex(await crypto.subtle.digest('SHA-256', b));
const unb64 = (s) => Uint8Array.from(atob(String(s).replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));

export const INBOX_RULE = 'Commonty inbox ';

async function read(env, k, d) {
  const v = await env.PINNED.get(k);
  return v ? JSON.parse(v) : d;
}
export const mailKey = (env, name) => read(env, `mailkey:${name}`, null);

// what Cloudflare's own checks of the sender said: the results it puts on
// top, under its own name, and nothing a sender wrote in (an ARC header, an
// Authentication-Results of its own further down)
export function authOf(headers) {
  const all = String(headers.get('authentication-results') || '').toLowerCase();
  const at = all.indexOf('mx.cloudflare.net');
  if (at < 0) return { spf: '', dkim: '', dmarc: '' };
  const next = all.indexOf('mx.cloudflare.net', at + 1);
  const ours = all.slice(at, next < 0 ? undefined : next);
  const res = (k) => (ours.match(new RegExp(`\\b${k}=([a-z]+)`)) || [])[1] || '';
  return { spf: res('spf'), dkim: res('dkim'), dmarc: res('dmarc') };
}

// The message, locked: the summary and the whole of it, each bound to its id
// and which it is, so a box cannot pass one off as another. One blob for the
// Hold: u32 summary length, summary, raw.
export async function lockMail(env, pk, id, raw, extra) {
  let s;
  try { s = summary(raw); } catch { s = { subject: '', from: { name: '', address: '' }, to: [], cc: [], snippet: '', size: raw.length }; }
  Object.assign(s, extra, { id, at: Date.now() });
  const pkb = b64u(pk);
  const lockedSummary = await seal(pkb, `commonty mail v1 summary ${id}`, enc.encode(JSON.stringify(s)));
  const lockedRaw = await seal(pkb, `commonty mail v1 raw ${id}`, raw);
  // signed as this Worker's: a box can lock anything to a public key, and
  // the member's page believes what came only with this signature on it
  const k = await signer(env);
  const h = { ...(await objectHeader('in', id, lockedSummary, lockedRaw)), by: 'worker', kid: k.kid };
  h.sig = await signEd25519(k.pkcs8, h);
  return pack(h, lockedSummary, lockedRaw);
}

// MAIL_SIGN (secret: PKCS#8 Ed25519, `dd mail dkim` makes it): what signs
// mail locked here. Every public half it has had is kept (KV signkeys), so
// a device opening older mail after a new key still believes it.
let signerCache = null;
export async function signer(env) {
  if (!env.MAIL_SIGN) throw new Error('MAIL_SIGN is not set');
  if (!signerCache || signerCache.src !== env.MAIL_SIGN) {
    const pkcs8 = unb64(env.MAIL_SIGN);
    const k = await crypto.subtle.importKey('pkcs8', pkcs8, { name: 'Ed25519' }, true, ['sign']);
    const x = (await crypto.subtle.exportKey('jwk', k)).x;
    const pub = btoa(String.fromCharCode(...b64u(x)));
    signerCache = { src: env.MAIL_SIGN, pkcs8, pub, kid: x.slice(0, 12), known: false };
  }
  if (!signerCache.known) {
    const keys = await read(env, 'signkeys', []);
    if (!keys.some((k) => k.pub === signerCache.pub)) await env.PINNED.put('signkeys', JSON.stringify([...keys, { kid: signerCache.kid, pub: signerCache.pub, since: Date.now() }]));
    signerCache.known = true;
  }
  return signerCache;
}

const hold = (env) => env.HOLD.get(env.HOLD.idFromName('hold'));

// Locking costs the Worker about 2 ms of CPU a megabyte, and the free plan
// allows 10: past this, the inbox gets a locked note of what came, and the
// message itself goes on to the member's own address (LOCK_MAX raises it)
const LOCK_MAX = 2_000_000;
const enc8 = new TextEncoder();

// Mail to a member who has an inbox: locked and held. Returns false when it
// was not held whole, so the caller forwards it whatever the member chose.
export async function holdMail(env, name, key, message, alias) {
  try {
    let raw = new Uint8Array(await new Response(message.raw).arrayBuffer());
    const id = newId();
    const extra = { alias: alias || null, auth: authOf(message.headers), rcpt: String(message.to || '').toLowerCase() };
    const max = Number(env.LOCK_MAX || LOCK_MAX);
    let whole = true;
    if (raw.length > max) {
      // its headers and a summary are kept; the rest went to their address
      let s;
      try { s = summary(raw); } catch { s = {}; }
      const mb = (raw.length / 1e6).toFixed(1);
      raw = enc8.encode(`From: Commonty Mail <noreply@${env.DOMAIN}>\r\nSubject: ${String(s.subject || '').replace(/[\r\n]+/g, ' ').slice(0, 200)}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nThis message was ${mb} MB, more than can be locked as it arrives. It went to your own address instead, whole.\r\n`);
      Object.assign(extra, { tooBig: raw.length, from: s.from, to: s.to, cc: s.cc, date: s.date, mid: s.mid, attachments: s.attachments, snippet: `${mb} MB: sent to your own address. ${s.snippet || ''}`.slice(0, 240) });
      whole = false;
    }
    const blob = await lockMail(env, key.pk, id, raw, extra);
    const r = await hold(env).fetch(`https://hold/put?member=${encodeURIComponent(name)}&id=${id}`, { method: 'POST', body: blob });
    if (!r.ok) throw new Error(`hold said ${r.status}`);
    return whole;
  } catch (err) {
    console.log(`inbox: ${name}: not held: ${err.message || err}`);
    return false;
  }
}

// mail to <name>@<domain> once their rule sends it here
export async function inboxMail(message, env, h) {
  const [local, domain] = String(message.to || '').toLowerCase().split('@');
  if (domain !== env.DOMAIN || !NAME.test(local || '')) return message.setReject('No such address here');
  const key = await mailKey(env, local);
  const s = h.stateIn(await h.listsShared(env), env, local);
  if (!key) {
    // a rule left over from an inbox turned off: forward as before
    if (s.email && s.confirmed) return message.forward(s.email);
    return message.setReject('This address has nowhere to go yet');
  }
  const held = await holdMail(env, local, key, message, null);
  if ((key.fwd || !held) && s.email && s.confirmed) await message.forward(s.email);
  else if (!held) message.setReject('This address cannot take mail just now; try again later');
}

// "commonty mail device v1\0<name>\0<fp>\0<at>\0<METHOD>\0<path>\0<sha256 body>"
export async function device(req, env, path, bytes, entry) {
  const name = req.headers.get('x-dd-name') || '';
  const fp = req.headers.get('x-dd-device') || '';
  const at = req.headers.get('x-dd-at') || '';
  const sig = req.headers.get('x-dd-sig') || '';
  if (!NAME.test(name) || !FP.test(fp)) return null;
  const t = Number(at);
  if (!Number.isInteger(t) || Math.abs(Math.floor(Date.now() / 1000) - t) > SKEW) return null;
  const key = await mailKey(env, name);
  const d = key?.sign?.find((x) => x.fp === fp);
  if (!d) return null;
  try {
    const k = await crypto.subtle.importKey('raw', unb64(d.key), { name: 'Ed25519' }, false, ['verify']);
    const msg = `commonty mail device v1\0${name}\0${fp}\0${at}\0${req.method}\0${path}\0${await sha(bytes)}`;
    if (!(await crypto.subtle.verify('Ed25519', k, unb64(sig), enc.encode(msg)))) return null;
  } catch {
    return null;
  }
  // still a device of theirs, as their own entry has it now: one taken off
  // their account speaks for them no longer
  try {
    const e = JSON.parse(await entry(env, name)).entry;
    if (!(e.devices || []).some((x) => x.fingerprint === fp && x.public_key === d.key)) return null;
  } catch {
    return null;
  }
  // and a request that does something, once: a copy sent again is refused
  if (req.method !== 'GET') {
    const r = await hold(env).fetch(`https://hold/once?k=${await sha(enc.encode(sig))}`, { method: 'POST' });
    if (r.status !== 200) return null;
  }
  return { name, fp, key };
}

// GET /api/inbox: is it on; POST /api/inbox {pk, device: {fp, key}, fwd}
// with the passkey just now; POST /api/inbox/forward {fwd}; POST
// /api/inbox/off with the passkey just now. `h`: rules and freshness.
export async function inboxApi(req, env, path, body, json, me, h) {
  const key = await mailKey(env, me.n);
  if (req.method === 'GET' && path === '/api/inbox') {
    return json({ on: !!key, fwd: key ? !!key.fwd : true, devices: (key?.sign || []).map((d) => d.fp), pk: key?.pk || null });
  }
  if (req.method !== 'POST') return json({ error: 'not here' }, 404);
  const b = body();
  if (path === '/api/inbox/forward') {
    if (!key) return json({ error: 'your inbox is not on' }, 409);
    // written only when it changes, and a few times a day: KV's writes a day
    // are the fleet's to share
    if (!!key.fwd === !!b.fwd) return json({ fwd: !!b.fwd });
    const r = await hold(env).fetch(`https://hold/count?member=${encodeURIComponent(me.n)}&what=fwd&max=20`, { method: 'POST' });
    if (r.status === 429) return json({ error: 'that has changed often enough today' }, 429);
    await env.PINNED.put(`mailkey:${me.n}`, JSON.stringify({ ...key, fwd: !!b.fwd }));
    return json({ fwd: !!b.fwd });
  }
  if (!h.fresh(me)) return json({ error: 'confirm it is you first' }, 403);
  if (path === '/api/inbox') {
    if (!env.MAIL_SIGN) return json({ error: 'Mail is not set up here yet' }, 503);
    if (!PK.test(b.pk || '')) return json({ error: 'that is not a mail key' }, 400);
    const d = b.device || {};
    if (!FP.test(d.fp || '') || !ED.test(d.key || '')) return json({ error: 'that is not a device key' }, 400);
    // a device of theirs, as their own signed entry has it, that signed this
    // key: a link someone else made cannot have their passkey lock their
    // mail to a key of that someone's
    let dev = null;
    let passkeyRoot = false;
    try {
      const e = JSON.parse(await h.entry(env, me.n)).entry;
      passkeyRoot = String(e.root || '').startsWith('webauthn:');
      dev = (e.devices || []).find((x) => x.fingerprint === d.fp && x.public_key === d.key);
    } catch { /* no entry, no device */ }
    if (!dev) return json({ error: 'that device is not one of yours' }, 403);
    // an account a passkey signs: a box's page can have the passkey sign an
    // entry naming a device of the box's, so the device that turned Mail on
    // stays the only one (its key opens on that device alone in any case)
    if (passkeyRoot && key && !key.sign.some((x) => x.fp === d.fp)) return json({ error: 'Mail is on with another device of yours; for an account a passkey signs, it stays that one' }, 409);
    try {
      const k = await crypto.subtle.importKey('raw', unb64(d.key), { name: 'Ed25519' }, false, ['verify']);
      if (!(await crypto.subtle.verify('Ed25519', k, unb64(b.sig || ''), enc.encode(`commonty mail key v1\0${me.n}\0${b.pk}`)))) throw new Error('no');
    } catch {
      return json({ error: 'that key was not made by that device' }, 403);
    }
    if (key && key.pk !== b.pk) return json({ error: 'your inbox has another key; turn it off first' }, 409);
    const sign = (key?.sign || []).filter((x) => x.fp !== d.fp);
    if (sign.length >= MAX_DEVICES) return json({ error: 'that is as many devices as can send' }, 409);
    sign.push({ fp: d.fp, key: d.key });
    const next = { pk: b.pk, sign, fwd: key ? key.fwd : b.fwd !== false, at: key?.at || Date.now() };
    await env.PINNED.put(`mailkey:${me.n}`, JSON.stringify(next));
    try { await h.route(env, me.n, true); } catch (err) {
      return json({ error: 'Cloudflare did not answer; your inbox is on, and mail to your address starts arriving in a minute' }, 502);
    }
    return json({ on: true, fwd: next.fwd, devices: sign.map((x) => x.fp) });
  }
  if (path === '/api/inbox/off') {
    if (!key) return json({ on: false });
    try { await h.route(env, me.n, false); } catch {
      return json({ error: 'Cloudflare did not answer; try again in a minute' }, 502);
    }
    await env.PINNED.delete(`mailkey:${me.n}`);
    await hold(env).fetch(`https://hold/drop?member=${encodeURIComponent(me.n)}`, { method: 'POST' });
    return json({ on: false });
  }
  return json({ error: 'not here' }, 404);
}

// a member's public mail key, for another member's app to lock to; a device
// of theirs asks, signed
export async function keyApi(req, env, path, bytes, json, entry) {
  const who = await device(req, env, path, bytes, entry);
  if (!who) return json({ error: 'sign it with a device of yours' }, 401);
  const name = decodeURIComponent(path.slice('/api/mailkey/'.length));
  if (!NAME.test(name)) return json({ error: 'no such member' }, 404);
  const k = await mailKey(env, name);
  if (!k) return json({ error: 'they have no inbox' }, 404);
  // and the keys this Worker signs what it locks with, for the asker's page
  return json({ name, pk: k.pk, devices: k.sign.map((d) => ({ fp: d.fp, key: d.key })), worker: (await read(env, 'signkeys', [])).map((x) => ({ kid: x.kid, pub: x.pub })) });
}

// GET /api/addresses: what a device of theirs may send from - their address
// and their aliases on now
export async function addressesApi(req, env, path, bytes, json, entry) {
  const who = await device(req, env, path, bytes, entry);
  if (!who) return json({ error: 'sign it with a device of yours' }, 401);
  return json({ address: `${who.name}@${env.DOMAIN}`, aliases: await liveAliases(env, who.name), fwd: !!who.key.fwd });
}

// when they go: the key, and any mail waiting for a box
export async function forgetInbox(env, name) {
  await env.PINNED.delete(`mailkey:${name}`);
  try { await hold(env).fetch(`https://hold/drop?member=${encodeURIComponent(name)}`, { method: 'POST' }); } catch {}
}

// "commonty mail hold v1\0<box>\0<at ms>\0<METHOD>\0<path?query>\0<sha256 body>"
// GET /api/hold/list?after=<id>; GET /api/hold/get?id&n; POST
// /api/hold/ack?id; GET /api/hold/watch, a WebSocket told of each new one.
export async function holdApi(req, env, url, json) {
  const box = req.headers.get('x-dd-box') || '';
  const at = req.headers.get('x-dd-at') || '';
  const sig = req.headers.get('x-dd-sig') || '';
  const bytes = req.method === 'GET' ? new Uint8Array(0) : new Uint8Array(await req.arrayBuffer().catch(() => new ArrayBuffer(0)));
  if (!BOXES[box]) return json({ error: 'no such box' }, 403);
  // only a box that keeps members' mail takes it (fleet.js MAIL_BOXES, the
  // boxes with the library role): any other could only drop it
  if (!(MAIL_BOXES || []).includes(box)) return json({ error: 'this box keeps no mail' }, 403);
  const where = url.pathname + url.search;
  if (!(await signedBy(box, `commonty mail hold v1\0${box}\0${at}\0${req.method}\0${where}\0${await sha(bytes)}`, sig))) {
    return json({ error: 'not signed by that box' }, 403);
  }
  const t = Number(at);
  if (!Number.isInteger(t) || Math.abs(Date.now() - t) > SKEW * 1000) return json({ error: 'stale' }, 403);
  if (await env.PINNED.get(`stolen:${box}`)) return json({ error: 'this box is marked stolen' }, 403);
  const op = url.pathname.slice('/api/hold/'.length);
  if (!['list', 'get', 'ack', 'watch'].includes(op)) return json({ error: 'not here' }, 404);
  const fwd = new URL(`https://hold/${op}${url.search}`);
  fwd.searchParams.set('box', box);
  fwd.searchParams.set('at', String(t));
  return hold(env).fetch(fwd, { method: req.method, headers: req.headers, body: req.method === 'GET' ? undefined : bytes });
}

// One object for the fleet's waiting mail: little of it, for minutes.
export class Hold {
  constructor(state) {
    this.state = state;
    this.sql = state.storage.sql;
    this.sql.exec('CREATE TABLE IF NOT EXISTS item (id TEXT PRIMARY KEY, member TEXT NOT NULL, size INTEGER NOT NULL, parts INTEGER NOT NULL)');
    this.sql.exec('CREATE TABLE IF NOT EXISTS part (id TEXT NOT NULL, n INTEGER NOT NULL, data BLOB NOT NULL, PRIMARY KEY (id, n))');
    this.sql.exec('CREATE TABLE IF NOT EXISTS seen (box TEXT PRIMARY KEY, at INTEGER NOT NULL)');
  }

  json(v, status = 200) {
    return new Response(JSON.stringify(v), { status, headers: { 'content-type': 'application/json' } });
  }

  async fetch(req) {
    const url = new URL(req.url);
    const op = url.pathname.slice(1);
    const q = url.searchParams;
    if (op === 'put') {
      const member = q.get('member') || '';
      const id = q.get('id') || '';
      if (!NAME.test(member) || !ID.test(id)) return this.json({ error: 'bad' }, 400);
      const blob = new Uint8Array(await req.arrayBuffer());
      const n = Math.max(1, Math.ceil(blob.length / PART));
      this.state.storage.transactionSync(() => {
        for (let i = 0; i < n; i++) this.sql.exec('INSERT OR REPLACE INTO part (id, n, data) VALUES (?, ?, ?)', id, i, blob.subarray(i * PART, (i + 1) * PART));
        this.sql.exec('INSERT OR REPLACE INTO item (id, member, size, parts) VALUES (?, ?, ?, ?)', id, member, blob.length, n);
      });
      for (const ws of this.state.getWebSockets()) { try { ws.send(JSON.stringify({ id })); } catch {} }
      return this.json({ id });
    }
    if (op === 'once') {
      const k = q.get('k') || '';
      const now = Date.now();
      this.sql.exec('CREATE TABLE IF NOT EXISTS once (k TEXT PRIMARY KEY, at INTEGER NOT NULL)');
      this.sql.exec('DELETE FROM once WHERE at < ?', now - 15 * 60 * 1000);
      if ([...this.sql.exec('SELECT k FROM once WHERE k = ?', k)].length) return this.json({ error: 'seen' }, 409);
      this.sql.exec('INSERT INTO once (k, at) VALUES (?, ?)', k, now);
      return this.json({ ok: true });
    }
    if (op === 'count') {
      const member = `${q.get('member') || ''}${q.get('what') ? `/${q.get('what')}` : ''}`;
      const day = Math.floor(Date.now() / 86400000);
      this.sql.exec('CREATE TABLE IF NOT EXISTS sent (member TEXT NOT NULL, day INTEGER NOT NULL, n INTEGER NOT NULL, PRIMARY KEY (member, day))');
      this.sql.exec('DELETE FROM sent WHERE day < ?', day - 1);
      const n = ([...this.sql.exec('SELECT n FROM sent WHERE member = ? AND day = ?', member, day)][0]?.n || 0) + 1;
      if (n > Number(q.get('max') || 200)) return this.json({ error: 'enough' }, 429);
      this.sql.exec('INSERT OR REPLACE INTO sent (member, day, n) VALUES (?, ?, ?)', member, day, n);
      return this.json({ n });
    }
    if (op === 'drop') {
      const member = q.get('member') || '';
      const ids = [...this.sql.exec('SELECT id FROM item WHERE member = ?', member)].map((r) => r.id);
      for (const id of ids) { this.sql.exec('DELETE FROM part WHERE id = ?', id); this.sql.exec('DELETE FROM item WHERE id = ?', id); }
      return this.json({ dropped: ids.length });
    }
    // a box's: its signed time, newer than its last (worker.js checked the
    // signature and the skew)
    const box = q.get('box') || '';
    const at = Number(q.get('at'));
    const last = [...this.sql.exec('SELECT at FROM seen WHERE box = ?', box)][0]?.at || 0;
    if (!(at > last)) return this.json({ error: 'stale' }, 403);
    this.sql.exec('INSERT OR REPLACE INTO seen (box, at) VALUES (?, ?)', box, at);
    if (op === 'list') {
      const after = q.get('after') || '';
      const rows = [...this.sql.exec('SELECT id, member, size, parts FROM item WHERE id > ? ORDER BY id LIMIT 200', after)];
      return this.json({ items: rows });
    }
    if (op === 'get') {
      const id = q.get('id') || '';
      const n = Number(q.get('n') || 0);
      const row = [...this.sql.exec('SELECT data FROM part WHERE id = ? AND n = ?', id, n)][0];
      if (!row) return this.json({ error: 'gone' }, 404);
      return new Response(row.data, { headers: { 'content-type': 'application/octet-stream' } });
    }
    if (op === 'ack') {
      const id = q.get('id') || '';
      this.sql.exec('DELETE FROM part WHERE id = ?', id);
      this.sql.exec('DELETE FROM item WHERE id = ?', id);
      return this.json({ ok: true });
    }
    if (op === 'watch') {
      if (req.headers.get('upgrade') !== 'websocket') return this.json({ error: 'a WebSocket' }, 426);
      const [client, server] = Object.values(new WebSocketPair());
      this.state.acceptWebSocket(server, [box]);
      return new Response(null, { status: 101, webSocket: client });
    }
    return this.json({ error: 'not here' }, 404);
  }

  webSocketMessage(ws, msg) {
    if (msg === 'ping') ws.send('pong');
  }
  webSocketClose(ws, code) {
    try { ws.close(code, 'bye'); } catch {}
  }
}

export { tob64u };
