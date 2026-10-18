// A member's mailbox on their own device (docs/dev/mail.md). The box keeps
// only locked objects; this opens them with the member's key, keeps what
// it opened in the device's own store, and is what the mail page draws.
//
//   in/<id>      what came: the Worker's lock, or another member's app's
//   sent/<id>    what they sent, locked to their own key here
//   drafts/<id>  what they are writing
//   state/<id>   changes: read, starred, moved, deleted; every device
//                replays every other's, in the order of their ids
//   filter/model their spam filter
//
// Every locked object (mail-object.js) is a header, a summary and the
// message, the last two sealed (mail-hpke.js) to the member's key with info
// naming what and which it is. The header says who vouches for it: the mail
// Worker for mail that came, the member's own devices (a key from their mail
// key that no box has) for everything they made, another member's device
// for mail from them. Anything else is not believed: a box can lock what it
// likes to a public key, and is never taken at its word.
//
// `api` is the box and the app: list(kind, after), get(kind, id, range),
// put(kind, id, bytes), del(kind, id), deliver(to, id, bytes),
// key(name) (another member's, from the Worker), send({from, to, raw}),
// vouch(bytes). `db` is the device's store: get/put/del/all(prefix).

import { seal, open, b64u, tob64u } from './mail-hpke.js';
import { summary as summarize, readable, parse, plainOf, addresses } from './mail-mime.js';
import { Index } from './mail-search.js';
import { Filter } from './mail-filter.js';
import { compose, textOf } from './mail-compose.js';
import { newId } from './mail-ids.js';
import * as obj from './mail-object.js';

const te = new TextEncoder();
const td = new TextDecoder();
const HEAD = 64 * 1024;
const FOLDERS = ['inbox', 'spam', 'trash', 'archive', 'sent', 'drafts'];
const DAY = 86400000;
// the first id a day before this one (ids start with their time: mail-ids.js)
const back = (id) => {
  const t = id ? parseInt(id.slice(0, 9), 36) - DAY : 0;
  return t > 0 ? `${t.toString(36).padStart(9, '0')}${'0'.repeat(12)}` : '';
};

const info = (kind, part, id) => (kind === 'in' ? `commonty mail v1 ${part} ${id}` : `commonty mail v1 ${kind} ${part} ${id}`);

// sealed to `pk`, with a header not yet vouched for: {h, S, R}
export async function sealed(pk, kind, id, summ, raw) {
  const S = await seal(pk, info(kind, 'summary', id), te.encode(JSON.stringify(summ)));
  const R = await seal(pk, info(kind, 'raw', id), raw);
  return { h: await obj.header(kind, id, S, R), S, R };
}

export class Mailbox {
  constructor({ api, db, sk, pk, ikm, me, domain }) {
    Object.assign(this, { api, db, sk, pk, ikm, me, domain });
    this.workerKeys = [];
    this.msgs = new Map(); // id -> {id, kind, s (summary), folder, read, star, n (index number), deleted}
    this.byN = new Map(); // index number -> message, kept as they change
    this.index = new Index({ get: (k) => db.get(`ix:${k}`), put: (k, v) => db.put(`ix:${k}`, v) });
    this.filter = new Filter();
    this.cursor = { in: '', sent: '', drafts: '', state: '' };
    this.listeners = new Set();
    this.applied = new Set();
    this.unopened = new Set(); // ids that did not open, not asked again this session
  }

  on(f) { this.listeners.add(f); return () => this.listeners.delete(f); }
  changed(what) { for (const f of this.listeners) f(what); }

  // what the device kept: drawn before the box is asked anything
  async load() {
    this.mk = await obj.macKey(this.ikm);
    const meta = (await this.db.get('meta')) || {};
    this.workerKeys = meta.workerKeys || [];
    await this.workers().catch(() => {});
    this.cursor = { ...this.cursor, ...(meta.cursor || {}) };
    for (const id of meta.applied || []) this.applied.add(id);
    this.fullAt = meta.fullAt || 0;
    if (meta.filter) this.filter = new Filter(meta.filter);
    for (const m of await this.db.all('m:')) this.msgs.set(m.id, m);
    if (meta.index) {
      this.index.restore(meta.index);
      for (const m of this.msgs.values()) if (m.n !== undefined) this.byN.set(m.n, m);
    } else for (const m of this.sorted()) { m.n = undefined; this.number(m); }
    this.changed('all');
  }

  // the keys the mail Worker signs what it locks with, as it says over TLS
  async workers() {
    const k = await this.api.key(this.me);
    if (Array.isArray(k.worker)) this.workerKeys = k.worker;
  }

  async keep() {
    await this.db.put('meta', { workerKeys: this.workerKeys, cursor: this.cursor, fullAt: this.fullAt, applied: [...this.applied].slice(-5000), filter: this.filter.toJSON(), index: this.index.snapshot() });
  }

  docOf(m) { return { ...m.s, body: m.body || m.s.snippet }; }

  // indexed (again): its number, and the way back from it
  number(m) {
    if (m.n !== undefined) { this.index.remove(m.n); this.byN.delete(m.n); }
    m.n = this.index.add(this.docOf(m));
    this.byN.set(m.n, m);
  }

  sorted() { return [...this.msgs.values()].sort((a, b) => (a.id < b.id ? -1 : 1)); }

  // an object of ours, vouched for by our own devices
  async lockMine(kind, id, summ, raw) {
    const { h, S, R } = await sealed(this.pk, kind, id, summ, raw);
    h.by = 'me';
    h.mac = await obj.mac(this.mk, h);
    return obj.pack(h, S, R);
  }

  // who vouches for this header, if anyone we believe: `from` is who the box
  // says handed it over (another member's mail, through their box)
  async vouched(kind, h, from) {
    if (h.by === 'me') return !from && (await obj.macOk(this.mk, h)) ? 'me' : null;
    if (kind !== 'in') return null;
    if (h.by === 'worker') {
      if (from) return null;
      let k = this.workerKeys.find((x) => x.kid === h.kid);
      if (!k) { await this.workers().catch(() => {}); k = this.workerKeys.find((x) => x.kid === h.kid); }
      return k && (await obj.verifyEd25519(k.pub, h)) ? 'worker' : null;
    }
    if (h.by === 'member') {
      if (!from || from !== h.name) return null;
      const k = await this.api.key(h.name).catch(() => null);
      const d = k?.devices?.find((x) => x.fp === h.fp);
      return d && (await obj.verifyEd25519(d.key, h)) ? 'member' : null;
    }
    return null;
  }

  // the summary at the front of a locked object, read without the rest,
  // and believed only as its header is
  async head(kind, id) {
    let r = await this.api.get(kind, id, `bytes=0-${HEAD - 1}`);
    let b = r.bytes;
    const need = obj.headLength(b);
    if (need === null) throw new Error('too short');
    if (need > b.length) { r = await this.api.get(kind, id, `bytes=0-${need - 1}`); b = r.bytes; }
    const { h, summary } = obj.unpack(b);
    await obj.checkSummary(h, kind, id, summary);
    const by = await this.vouched(kind, h, r.from || null);
    if (!by) throw new Error('vouched for by nobody this device believes');
    const s = JSON.parse(td.decode(await open(this.sk, this.pk, info(kind, 'summary', id), summary)));
    // another member's: from their own address, as the box says it is them
    if (by === 'member' && s.from?.address !== `${h.name}@${this.domain}`) throw new Error('not from the member who signed it');
    return { s, by, h };
  }

  // what is new on the box, of each kind, after the last one seen
  // ids are made by whoever sent or took the mail, and a box can store one
  // after a later one, so each sync asks again from a day back, and once a
  // day from the start
  async sync() {
    let got = 0;
    const full = Date.now() - (this.fullAt || 0) > DAY;
    for (const kind of ['in', 'sent', 'drafts']) {
      let after = full ? '' : back(this.cursor[kind]);
      for (;;) {
        const items = await this.api.list(kind, after);
        if (!items.length) break;
        for (const it of items) {
          after = it.id;
          if (it.id > this.cursor[kind]) this.cursor[kind] = it.id;
          if (this.msgs.has(it.id) || this.unopened.has(it.id)) continue;
          try {
            const { s, by, h } = await this.head(kind, it.id);
            await this.take(kind, it.id, s, by, h);
            got++;
          } catch (err) {
            this.unopened.add(it.id);
            console.warn(`mail: ${kind}/${it.id} does not open: ${err.message || err}`);
          }
        }
        if (items.length < 1000) break;
      }
    }
    if (full) this.fullAt = Date.now();
    await this.syncState();
    if (got) await this.keep();
    return got;
  }

  async take(kind, id, s, by, h) {
    const folder = kind === 'sent' ? 'sent' : kind === 'drafts' ? 'drafts' : 'inbox';
    const m = { id, kind, s, by, sender: by === 'member' ? h.name : null, hs: h.hs, hr: h.hr, folder, read: kind !== 'in', star: false };
    if (kind === 'in' && this.filter.isSpam(s)) m.folder = 'spam';
    this.number(m);
    this.msgs.set(id, m);
    await this.db.put(`m:${id}`, m);
    this.changed(id);
  }

  // the whole message, opened; kept locked on the device after the first time
  async raw(id) {
    const m = this.msgs.get(id);
    let blob = await this.db.get(`r:${id}`);
    if (!blob) {
      blob = (await this.api.get(m.kind, id)).bytes;
      await this.db.put(`r:${id}`, blob);
    }
    // the very object whose summary was believed: the same header, and a
    // message that is the one it names
    const { h, raw: R } = obj.unpack(blob);
    if (h.kind !== m.kind || h.id !== id || h.hs !== m.hs || h.hr !== m.hr) throw new Error('not the message that was listed');
    await obj.checkRaw(h, R);
    return open(this.sk, this.pk, info(m.kind, 'raw', id), R);
  }

  // read it for the reader: html or text, attachments, and its words for search
  async read(id) {
    const raw = await this.raw(id);
    const root = parse(raw);
    const r = readable(root);
    const m = this.msgs.get(id);
    if (!m.body) {
      m.body = (r.plain ?? (r.html ? plainOf(r.html) : '')).slice(0, 64 * 1024);
      this.number(m);
      await this.db.put(`m:${id}`, m);
    }
    return { raw, root, ...r };
  }

  async change(ids, op) {
    const id = newId();
    const ops = ids.map((i) => ({ id: i, ...op }));
    for (const o of ops) await this.apply(o, true);
    const blob = await this.lockMine('state', id, { ops, by: this.me }, new Uint8Array(0));
    await this.api.put('state', id, blob);
    this.applied.add(id);
    await this.keep();
    this.changed('state');
  }

  async apply(o, mine) {
    const m = this.msgs.get(o.id);
    if (!m) return;
    if ('read' in o) m.read = !!o.read;
    if ('star' in o) m.star = !!o.star;
    if (o.folder && FOLDERS.includes(o.folder)) {
      // marked so by them: the filter learns, on the device that was told
      if (mine && m.kind === 'in' && (o.folder === 'spam') !== (m.folder === 'spam')) this.filter.moved(this.docOf(m), o.folder === 'spam');
      m.folder = o.folder;
    }
    if (o.gone) {
      m.deleted = true;
      this.index.remove(m.n);
      this.byN.delete(m.n);
      this.msgs.delete(o.id);
      await this.db.del(`m:${o.id}`);
      await this.db.del(`r:${o.id}`);
      if (mine) { try { await this.api.del(m.kind, o.id); } catch {} }
      return;
    }
    await this.db.put(`m:${o.id}`, m);
  }

  async syncState() {
    let after = back(this.cursor.state);
    for (;;) {
      const items = await this.api.list('state', after);
      if (!items.length) return;
      for (const it of items) {
        after = it.id;
        if (it.id > this.cursor.state) this.cursor.state = it.id;
        if (this.applied.has(it.id)) continue;
        try {
          const r = await this.api.get('state', it.id);
          const { h, summary } = obj.unpack(r.bytes);
          await obj.checkSummary(h, 'state', it.id, summary);
          if ((await this.vouched('state', h, r.from || null)) !== 'me') throw new Error('not made by a device of yours');
          const s = JSON.parse(td.decode(await open(this.sk, this.pk, info('state', 'summary', it.id), summary)));
          for (const o of s.ops || []) await this.apply(o, false);
          this.applied.add(it.id);
        } catch (err) {
          console.warn(`mail: a change does not open: ${err.message || err}`);
        }
      }
      this.changed('state');
      if (items.length < 1000) return;
    }
  }

  folder(name, address) {
    const out = [];
    for (const m of this.msgs.values()) {
      if (name === 'starred' ? !m.star || m.folder === 'trash' : m.folder !== name) continue;
      if (address && !this.toAddress(m, address)) continue;
      out.push(m);
    }
    return out.sort((a, b) => (a.id < b.id ? 1 : -1));
  }
  toAddress(m, address) {
    const a = address.toLowerCase();
    if (m.s.rcpt === a) return true;
    if (m.s.alias && `${m.s.alias}@${this.domain}` === a) return true;
    return [...(m.s.to || []), ...(m.s.cc || [])].some((x) => x.address === a) || m.s.from?.address === a;
  }
  unread(name) { let n = 0; for (const m of this.msgs.values()) if (m.folder === name && !m.read) n++; return n; }

  async search(q, limit = 50) {
    const r = await this.index.search(q, limit * 2);
    const byN = this.byN;
    // newest first by when it came: reading one indexes it again, later
    const hits = r.hits.map((n) => byN.get(n)).filter((m) => m && m.folder !== 'trash').sort((a, b) => (a.id < b.id ? 1 : -1)).slice(0, limit);
    return { hits, more: r.more };
  }

  // {from, to, cc, bcc, subject, html, attachments, reply} -> what became of each
  async send(d) {
    const html = d.html || '';
    const { raw, mid } = compose({
      domain: this.domain,
      from: d.from,
      to: d.to,
      cc: d.cc,
      subject: d.subject,
      html: html || null,
      text: textOf(html),
      attachments: d.attachments,
      inReplyTo: d.reply?.mid || null,
      references: d.reply ? [...(d.reply.refs || []), d.reply.mid].filter(Boolean).slice(-20) : null,
    });
    const all = [...(d.to || []), ...(d.cc || []), ...(d.bcc || [])].map((a) => String(a.address || a).toLowerCase());
    // to members, end to end, from their own address: from an alias it goes
    // the way any mail goes, so the alias is not tied to their name
    const own = String(d.from?.address || d.from).toLowerCase() === `${this.me}@${this.domain}`;
    const members = own ? all.filter((a) => a.endsWith(`@${this.domain}`)) : [];
    const outside = all.filter((a) => !members.includes(a));
    const results = [];
    // members: locked on this device to each one's key, their box told
    for (const to of members) {
      const name = to.split('@')[0];
      try {
        const k = await this.api.key(name);
        const id = newId();
        const s = summarize(raw);
        Object.assign(s, { id, at: Date.now(), rcpt: to, auth: { e2e: true } });
        const { h, S, R } = await sealed(b64u(k.pk), 'in', id, s, raw);
        const v = await this.api.vouch({ id, hs: h.hs, hr: h.hr });
        Object.assign(h, { by: 'member', name: v.name, fp: v.fp, sig: v.sig });
        await this.api.deliver(name, id, obj.pack(h, S, R));
        results.push({ to, ok: true, e2e: true });
      } catch (err) {
        // no inbox of theirs: it goes the way any mail goes
        outside.push(to);
      }
    }
    if (outside.length) {
      const r = await this.api.send({ from: d.from.address || d.from, to: outside, raw: tob64u(raw) });
      results.push(...r.sent);
    }
    // a copy for Sent, locked here to their own key
    if (results.some((r) => r.ok)) {
      const id = newId();
      const s = summarize(raw);
      Object.assign(s, { id, at: Date.now(), bcc: d.bcc || [], results });
      const blob = await this.lockMine('sent', id, s, raw);
      await this.api.put('sent', id, blob);
      await this.take('sent', id, s, 'me', obj.unpack(blob).h);
      this.cursor.sent = this.cursor.sent > id ? this.cursor.sent : id;
      await this.db.put(`r:${id}`, blob);
      await this.keep();
    }
    if (d.draftId) await this.discard(d.draftId);
    return { results, mid };
  }

  async saveDraft(d) {
    const id = d.draftId || newId();
    const s = { id, at: Date.now(), subject: d.subject || '', to: (d.to || []).map((a) => ({ name: '', address: a.address || a })), from: { name: '', address: d.from?.address || d.from || '' }, snippet: textOf(d.html || '').slice(0, 240), draft: d };
    const blob = await this.lockMine('drafts', id, s, new Uint8Array(0));
    await this.api.put('drafts', id, blob);
    const m = this.msgs.get(id) || { id, kind: 'drafts', by: 'me', folder: 'drafts', read: true, star: false };
    const h = obj.unpack(blob).h;
    Object.assign(m, { s, hs: h.hs, hr: h.hr });
    this.number(m);
    this.msgs.set(id, m);
    await this.db.put(`m:${id}`, m);
    this.changed(id);
    return id;
  }

  async discard(id) {
    await this.change([id], { gone: true });
  }

  // a Google Takeout or any mbox: each message locked here, kept as theirs
  async import(bytes, onProgress) {
    let n = 0;
    for (const raw of mbox(bytes)) {
      const id = newId();
      const s = summarize(raw);
      Object.assign(s, { id, at: s.date || Date.now(), imported: true });
      const blob = await this.lockMine('in', id, s, raw);
      await this.api.put('in', id, blob);
      await this.take('in', id, s, 'me', obj.unpack(blob).h);
      const m = this.msgs.get(id);
      m.read = true;
      if (m.folder === 'spam') m.folder = 'inbox';
      await this.db.put(`m:${id}`, m);
      if (++n % 50 === 0) { await this.keep(); onProgress?.(n); }
    }
    this.cursor.in = [...this.msgs.keys()].filter((k) => this.msgs.get(k).kind === 'in').sort().pop() || this.cursor.in;
    await this.keep();
    onProgress?.(n);
    return n;
  }
}

// an mbox's messages: split at "From " lines, ">From " unquoted
export function* mbox(bytes) {
  const s = bytes instanceof Uint8Array ? bytes : te.encode(String(bytes));
  let start = -1;
  const isFrom = (i) => s[i] === 70 && s[i + 1] === 114 && s[i + 2] === 111 && s[i + 3] === 109 && s[i + 4] === 32;
  const emit = function* (a, b) {
    let i = a;
    while (i < b && s[i] !== 10) i++;
    const body = s.subarray(i + 1, b);
    const text = td.decode(body).replace(/^>(>*From )/gm, '$1');
    if (text.trim()) yield te.encode(text);
  };
  for (let i = 0; i < s.length; i++) {
    if ((i === 0 || s[i - 1] === 10) && isFrom(i)) {
      if (start >= 0) yield* emit(start, i);
      start = i;
    }
  }
  if (start >= 0) yield* emit(start, s.length);
  else if (s.length) yield s;
}

export { addresses };
