// Spam, sorted on the member's own devices (docs/dev/mail.md): a filter that
// learns from what they mark Spam and Not spam, kept locked on their box
// like everything else. Words of the sender, the subject and the body
// weighed as Graham's "A Plan for Spam" does: each word's chance of being
// spam's, the fifteen most telling combined. The Worker's own word - a
// sender whose domain's checks failed - counts before any of it.

import { tokens } from './mail-search.js';

const MAX_WORDS = 400;
const TELLING = 15;

export function features(doc) {
  const f = new Set();
  const from = String(doc.from?.address || '').toLowerCase();
  const dom = from.split('@')[1] || '';
  if (dom) { f.add(`dom:${dom}`); f.add(`dom:${dom.split('.').slice(-2).join('.')}`); }
  for (const w of tokens(doc.from?.name || '')) f.add(`name:${w}`);
  for (const w of tokens(doc.subject || '')) f.add(`subj:${w}`);
  let n = 0;
  for (const w of tokens(doc.body || doc.snippet || '')) { if (n++ > MAX_WORDS) break; if (w.length > 2) f.add(w); }
  if (doc.list) f.add('meta:list');
  if ((doc.attachments || []).some((a) => /\.(exe|scr|js|vbs|bat|cmd|jar|iso|img|lnk|hta|msi)$/i.test(a))) f.add('meta:runnable');
  return f;
}

export class Filter {
  constructor(model) {
    const m = model || {};
    this.spam = new Map(Object.entries(m.spam || {}));
    this.ham = new Map(Object.entries(m.ham || {}));
    this.nSpam = m.nSpam || 0;
    this.nHam = m.nHam || 0;
  }

  learn(doc, isSpam, undo = false) {
    const map = isSpam ? this.spam : this.ham;
    const d = undo ? -1 : 1;
    for (const w of features(doc)) {
      const v = (map.get(w) || 0) + d;
      if (v > 0) map.set(w, v); else map.delete(w);
    }
    if (isSpam) this.nSpam = Math.max(0, this.nSpam + d); else this.nHam = Math.max(0, this.nHam + d);
  }

  // moved by them from one to the other: unlearnt there, learnt here
  moved(doc, toSpam) { this.learn(doc, !toSpam, true); this.learn(doc, toSpam); }

  word(w) {
    const g = 2 * (this.ham.get(w) || 0);
    const b = this.spam.get(w) || 0;
    if (g + b < 3) return null;
    const pb = Math.min(1, b / Math.max(1, this.nSpam));
    const pg = Math.min(1, g / Math.max(1, this.nHam));
    if (pb + pg === 0) return null;
    return Math.max(0.01, Math.min(0.99, pb / (pb + pg)));
  }

  // 0 (theirs) to 1 (spam)
  score(doc) {
    if (doc.auth && doc.auth.dmarc === 'fail') return 1;
    if (this.nSpam < 5 || this.nHam < 5) return features(doc).has('meta:runnable') ? 0.95 : 0;
    const ps = [];
    for (const w of features(doc)) { const p = this.word(w); if (p !== null) ps.push(p); }
    ps.sort((a, b) => Math.abs(b - 0.5) - Math.abs(a - 0.5));
    const top = ps.slice(0, TELLING);
    if (!top.length) return 0;
    let a = 1, b = 1;
    for (const p of top) { a *= p; b *= 1 - p; }
    return a / (a + b);
  }

  isSpam(doc) { return this.score(doc) > 0.9; }

  toJSON() {
    return { spam: Object.fromEntries(this.spam), ham: Object.fromEntries(this.ham), nSpam: this.nSpam, nHam: this.nHam };
  }
}
