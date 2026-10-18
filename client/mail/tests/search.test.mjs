// The search: right answers, and at once however much mail there is.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Index, tokens, parseQuery, SEG } from '../mods/mail-search.js';

const mem = () => { const m = new Map(); return { get: async (k) => m.get(k), put: async (k, v) => m.set(k, v), m }; };

test('words, accents and CJK', () => {
  assert.deepEqual(tokens('Café, NAÏVE déjà-vu 2026!'), ['cafe', 'naive', 'deja', 'vu', '2026']);
  assert.deepEqual(tokens('東京タワー'), ['東', '東京', '京', '京タ', 'タ', 'タワ', 'ワ', 'ワー', 'ー']);
  assert.deepEqual(parseQuery('from:Otter lunch no').map((w) => [w.term, w.prefix]), [['from:otter', false], ['lunch', false], ['no', true]]);
});

test('found by subject, sender, body and file, newest first', async () => {
  const ix = new Index(mem());
  const a = ix.add({ subject: 'Lunch on Friday', from: { name: 'Sam', address: 'sam@commonty.org' }, body: 'noodles at noon' });
  const b = ix.add({ subject: 'Order shipped', from: { name: 'Northside', address: 'orders@northside.example' }, body: 'two kits', attachments: ['invoice-48213.pdf'] });
  const c = ix.add({ subject: 'Re: Lunch on Friday', from: { name: 'Lingyun', address: 'lingyun@commonty.org' }, body: 'yes, noodles' });
  assert.deepEqual((await ix.search('lunch')).hits, [c, a]);
  assert.deepEqual((await ix.search('noodl')).hits, [c, a], 'the word being typed is a prefix');
  assert.deepEqual((await ix.search('from:sam lunch')).hits, [a]);
  assert.deepEqual((await ix.search('file:invoice')).hits, [b]);
  assert.deepEqual((await ix.search('northside kits')).hits, [b]);
  assert.deepEqual((await ix.search('lunch kits')).hits, []);
  ix.remove(c);
  assert.deepEqual((await ix.search('lunch')).hits, [a], 'a deleted one is not found');
});

test('across segments, frozen and cold, and back from a snapshot', async () => {
  const store = mem();
  const ix = new Index(store);
  for (let i = 0; i < SEG * 3 + 10; i++) ix.add({ subject: `note ${i}`, body: i % 1000 === 7 ? 'rare zebra' : 'common words here' });
  ix.cool(0);
  const r = await ix.search('zebra', 100);
  assert.deepEqual(r.hits, [6007, 5007, 4007, 3007, 2007, 1007, 7]);
  const again = new Index(store);
  again.restore(ix.snapshot());
  assert.deepEqual((await again.search('zebra', 3)).hits, [6007, 5007, 4007]);
  const n = again.add({ subject: 'zebra crossing' });
  assert.equal((await again.search('zebra', 1)).hits[0], n);
});

// a made-up mailbox: words drawn as real text draws them (Zipf), a few
// hundred thousand messages
function corpus(n, seed = 1) {
  let x = seed;
  const rnd = () => {
    x = (Math.imul(x, 1103515245) + 12345) >>> 0;
    return x / 2 ** 32;
  };
  const vocab = Array.from({ length: 60000 }, (_, i) => `w${i.toString(36)}`);
  const pick = () => vocab[Math.min(vocab.length - 1, Math.floor(1 / (rnd() * 0.9999 + 0.0001)) - 1 + Math.floor(rnd() * 3))];
  const docs = [];
  for (let i = 0; i < n; i++) {
    const body = Array.from({ length: 80 }, pick).join(' ');
    docs.push({ subject: `${pick()} ${pick()} ${pick()}`, from: { name: pick(), address: `${pick()}@${pick()}.example` }, body });
  }
  return docs;
}

test('instant on 300,000 messages', async () => {
  const N = 300_000;
  const ix = new Index(mem());
  let t = performance.now();
  for (const d of corpus(N)) ix.add(d);
  const build = performance.now() - t;
  ix.cool(16);
  const queries = ['w1', 'w2 w3', 'w1 w5', 'w9z', 'w1a w2', 'wz', 'w4 w7 w2', 'nothinglikeit', 'w1k w1', 'w', 'w5f'];
  const times = [];
  for (let round = 0; round < 3; round++) {
    for (const q of queries) {
      t = performance.now();
      await ix.search(q, 50);
      times.push(performance.now() - t);
    }
  }
  times.sort((a, b) => a - b);
  const p50 = times[Math.floor(times.length / 2)];
  const p95 = times[Math.floor(times.length * 0.95)];
  console.log(`indexed ${N} in ${(build / 1000).toFixed(1)} s; a search: median ${p50.toFixed(2)} ms, 95th ${p95.toFixed(2)} ms, worst ${times[times.length - 1].toFixed(2)} ms`);
  assert.ok(p95 < 16, `a search must fit in a frame: 95th ${p95.toFixed(2)} ms`);
});
