// Search over a member's mail, on their own device (docs/dev/mail.md): it
// must answer at once however much mail there is. Messages are numbered as
// they come (newer, higher). Every SEG of them make a segment: a sorted
// term list, each term's postings (message numbers, ascending), and a bloom
// filter over the terms and their first few letters. A query walks the
// segments newest first, skips any whose bloom says a word cannot be in it,
// and stops when it has enough: the answer is the newest matches, and old
// segments are touched only when the new ones fall short. The segment
// still filling is held in memory as maps.
//
// No DOM here: the mail page runs it in a Web Worker, node's tests run it
// as it is. `store` keeps sealed segments: {get(key), put(key, value)}.

export const SEG = 2048;
const PREFIXES = [1, 2, 3, 4];
const BLOOM_BITS = 1 << 20;
const BLOOM_K = 4;
const MAX_TERM = 32;

// words: letters and digits of any alphabet, without their accents; Chinese,
// Japanese and Korean, which have no spaces, by each character and each pair
const CJK = /[぀-ヿ㐀-䶿一-鿿가-힯豈-﫿]/u;
export function tokens(text) {
  const out = [];
  const s = String(text || '').normalize('NFKD').replace(/[̀-ͯ]/g, '').toLowerCase();
  for (const m of s.matchAll(/[\p{L}\p{N}]+/gu)) {
    const w = m[0];
    if (CJK.test(w)) {
      const chars = [...w];
      for (let i = 0; i < chars.length; i++) {
        if (CJK.test(chars[i])) out.push(chars[i]);
        if (i + 1 < chars.length) out.push(chars[i] + chars[i + 1]);
      }
      continue;
    }
    if (w.length <= MAX_TERM) out.push(w);
  }
  return out;
}

// the fields a message is found by, each word once
export function termsOf(doc) {
  const t = new Set();
  const add = (s, tag) => { for (const w of tokens(s)) { t.add(w); if (tag) t.add(`${tag}:${w}`); } };
  add(doc.subject, 'subject');
  add([doc.from?.name, doc.from?.address].join(' '), 'from');
  add((doc.to || []).concat(doc.cc || []).map((a) => `${a.name || ''} ${a.address || ''}`).join(' '), 'to');
  add((doc.attachments || []).join(' '), 'file');
  add(doc.body || doc.snippet || '', null);
  return t;
}

function fnv(s, seed) {
  let h = 2166136261 ^ seed;
  for (let i = 0; i < s.length; i++) { h ^= s.charCodeAt(i); h = Math.imul(h, 16777619); }
  return h >>> 0;
}
class Bloom {
  constructor(bits) { this.bits = bits || new Uint32Array(BLOOM_BITS / 32); }
  add(s) { for (let k = 0; k < BLOOM_K; k++) { const b = fnv(s, k * 0x9e3779b9) % BLOOM_BITS; this.bits[b >>> 5] |= 1 << (b & 31); } }
  has(s) { for (let k = 0; k < BLOOM_K; k++) { const b = fnv(s, k * 0x9e3779b9) % BLOOM_BITS; if (!(this.bits[b >>> 5] & (1 << (b & 31)))) return false; } return true; }
}
const bloomKeys = (term) => {
  const keys = [`=${term}`];
  for (const n of PREFIXES) if (term.length >= n) keys.push(`^${term.slice(0, n)}`);
  return keys;
};

// a full segment, frozen: terms sorted, postings packed in one array
function freeze(map) {
  const terms = [...map.keys()].sort();
  let n = 0;
  for (const t of terms) n += map.get(t).length;
  const post = new Uint32Array(n);
  const off = new Uint32Array(terms.length + 1);
  let o = 0;
  terms.forEach((t, i) => { off[i] = o; const p = map.get(t); post.set(p, o); o += p.length; });
  off[terms.length] = o;
  const bloom = new Bloom();
  for (const t of terms) for (const k of bloomKeys(t)) bloom.add(k);
  return { terms, off, post, bloom: bloom.bits };
}

// the first index whose term is >= key
function lower(terms, key) {
  let lo = 0, hi = terms.length;
  while (lo < hi) { const mid = (lo + hi) >> 1; if (terms[mid] < key) lo = mid + 1; else hi = mid; }
  return lo;
}

// postings for a word, or for every word starting so (the one being typed)
function lookup(seg, word, prefix) {
  if (seg.map) {
    if (!prefix) return seg.map.get(word) || null;
    // the filling segment's words, sorted once per change
    if (!seg.sorted) seg.sorted = [...seg.map.keys()].sort();
    let all = null;
    for (let i = lower(seg.sorted, word); i < seg.sorted.length && seg.sorted[i].startsWith(word); i++) {
      const p = seg.map.get(seg.sorted[i]);
      all = all ? union(all, p) : p;
    }
    return all;
  }
  const i = lower(seg.terms, word);
  if (!prefix) return i < seg.terms.length && seg.terms[i] === word ? seg.post.subarray(seg.off[i], seg.off[i + 1]) : null;
  let j = i;
  while (j < seg.terms.length && seg.terms[j].startsWith(word)) j++;
  if (j === i) return null;
  if (j === i + 1) return seg.post.subarray(seg.off[i], seg.off[i + 1]);
  // many words start so: their postings merged, at most a segment's worth
  const mark = new Uint8Array(SEG);
  let any = false;
  for (let k = i; k < j; k++) for (let x = seg.off[k]; x < seg.off[k + 1]; x++) { mark[seg.post[x] - seg.base] = 1; any = true; }
  if (!any) return null;
  const out = [];
  for (let d = 0; d < SEG; d++) if (mark[d]) out.push(seg.base + d);
  return out;
}
function union(a, b) {
  const out = [];
  let i = 0, j = 0;
  while (i < a.length || j < b.length) {
    if (j >= b.length || (i < a.length && a[i] < b[j])) out.push(a[i++]);
    else if (i >= a.length || b[j] < a[i]) out.push(b[j++]);
    else { out.push(a[i++]); j++; }
  }
  return out;
}
function intersect(a, b) {
  const out = [];
  let i = 0, j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] < b[j]) i++; else if (b[j] < a[i]) j++; else { out.push(a[i]); i++; j++; }
  }
  return out;
}

// "from:otter lunch noo" -> words that must all be there, the last a prefix
export function parseQuery(q) {
  const parts = String(q || '').trim().split(/\s+/).filter(Boolean);
  const words = [];
  parts.forEach((p, idx) => {
    const m = p.match(/^(from|to|subject|file):(.*)$/i);
    const tag = m ? m[1].toLowerCase() : null;
    const toks = tokens(m ? m[2] : p);
    toks.forEach((t, k) => {
      words.push({ term: tag ? `${tag}:${t}` : t, prefix: idx === parts.length - 1 && k === toks.length - 1 && !/\s$/.test(q) });
    });
  });
  return words;
}

export class Index {
  constructor(store) {
    this.store = store;
    this.segs = []; // frozen: {base, terms, off, post, bloom} or {base, bloom, cold: true}
    this.live = { base: 0, map: new Map(), count: 0 };
    this.next = 0;
    this.deleted = new Set();
  }

  // a message, numbered next; returns its number
  add(doc) {
    const n = this.next++;
    for (const t of termsOf(doc)) {
      let p = this.live.map.get(t);
      if (!p) {
        p = [];
        this.live.map.set(t, p);
        this.live.sorted = null;
      }
      p.push(n);
    }
    this.live.count++;
    if (this.live.count >= SEG) this.seal();
    return n;
  }

  remove(n) { this.deleted.add(n); }

  // the segment filling up is frozen and kept; another starts
  seal() {
    if (!this.live.count) return;
    const f = freeze(this.live.map);
    const seg = { base: this.live.base, ...f };
    this.segs.push(seg);
    if (this.store) this.store.put(`seg:${seg.base}`, { terms: f.terms, off: f.off, post: f.post, bloom: f.bloom });
    this.live = { base: this.next, map: new Map(), count: 0 };
  }

  async load(seg) {
    if (!seg.cold) return seg;
    const v = await this.store.get(`seg:${seg.base}`);
    Object.assign(seg, v, { cold: false });
    return seg;
  }

  // drops postings of old segments from memory, keeping their blooms
  cool(keep = 8) {
    for (let i = 0; i < this.segs.length - keep; i++) {
      const s = this.segs[i];
      if (!s.cold && this.store) { s.terms = s.off = s.post = null; s.cold = true; }
    }
  }

  maybe(seg, words) {
    if (seg.map) return true;
    const b = new Bloom(seg.bloom);
    return words.every((w) => {
      const t = w.term;
      if (!w.prefix) return b.has(`=${t}`);
      return b.has(`^${t.slice(0, Math.min(t.length, PREFIXES[PREFIXES.length - 1]))}`) && (t.length > PREFIXES[PREFIXES.length - 1] || b.has(`^${t}`));
    });
  }

  // the newest `limit` messages with every word, and whether there were more
  async search(q, limit = 50) {
    const words = parseQuery(q);
    if (!words.length) return { hits: [], more: false, looked: 0 };
    const hits = [];
    let looked = 0;
    const all = [...this.segs, this.live];
    for (let i = all.length - 1; i >= 0 && hits.length <= limit; i--) {
      const seg = all[i];
      seg.base ??= 0;
      if (!this.maybe(seg, words)) continue;
      looked++;
      await this.load(seg);
      // rarest first: fewest postings to walk
      const lists = [];
      let none = false;
      for (const w of words) {
        const p = lookup(seg, w.term, w.prefix);
        if (!p || !p.length) { none = true; break; }
        lists.push(p);
      }
      if (none) continue;
      lists.sort((a, b) => a.length - b.length);
      let r = lists[0];
      for (let k = 1; k < lists.length && r.length; k++) r = intersect(r, lists[k]);
      for (let k = r.length - 1; k >= 0 && hits.length <= limit; k--) if (!this.deleted.has(r[k])) hits.push(r[k]);
    }
    const more = hits.length > limit;
    return { hits: hits.slice(0, limit), more, looked };
  }

  // what is kept to start again without reading every message: the frozen
  // segments' blooms and bounds; the postings are in the store
  snapshot() {
    this.seal();
    return { next: this.next, segs: this.segs.map((s) => ({ base: s.base, bloom: s.bloom })), deleted: [...this.deleted] };
  }
  restore(snap) {
    this.next = snap.next;
    this.segs = snap.segs.map((s) => ({ base: s.base, bloom: s.bloom, cold: true }));
    this.deleted = new Set(snap.deleted);
    this.live = { base: this.next, map: new Map(), count: 0 };
  }
}
