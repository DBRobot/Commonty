// The mailbox on a device, against a made-up box: two devices of one
// member, and another member, each with their own key.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as hpke from '../mods/mail-hpke.js';
import { Mailbox, sealed } from '../mods/mail-mailbox.js';
import * as obj from '../mods/mail-object.js';
import { compose } from '../mods/mail-compose.js';
import { newId } from '../mods/mail-ids.js';

const te = new TextEncoder();
const b64 = (u) => Buffer.from(u).toString('base64');

// the mail Worker's signing key, as its MAIL_SIGN
const worker = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']);
const workerPkcs8 = new Uint8Array(await crypto.subtle.exportKey('pkcs8', worker.privateKey));
const workerPub = b64(new Uint8Array(await crypto.subtle.exportKey('raw', worker.publicKey)));
const WORKER = [{ kid: 'w1', pub: workerPub }];

// one box for everyone: members' objects by name, and the senders it saw
function box() {
  const objs = new Map();
  const key = (n, k, id) => `${n}/${k}/${id}`;
  return {
    objs,
    for(name, me) {
      return {
        list: async (kind, after) => [...objs.keys()].filter((k) => k.startsWith(`${name}/${kind}/`)).map((k) => k.split('/')[2]).filter((id) => id > after).sort().slice(0, 1000).map((id) => ({ id, size: objs.get(key(name, kind, id)).bytes.length })),
        get: async (kind, id, range) => {
          const o = objs.get(key(name, kind, id));
          if (!o) throw new Error('404');
          let b = o.bytes;
          if (range) { const [, a, z] = range.match(/bytes=(\d+)-(\d+)/); b = b.subarray(Number(a), Number(z) + 1); }
          return { bytes: b, from: o.from };
        },
        put: async (kind, id, bytes) => objs.set(key(name, kind, id), { bytes }),
        del: async (kind, id) => objs.delete(key(name, kind, id)),
        deliver: async (to, id, bytes) => objs.set(key(to, 'in', id), { bytes, from: name }),
        key: async (who) => ({ ...me.directory[who], worker: WORKER }),
        vouch: async (v) => me.vouch(v),
        send: async (p) => { me.sent.push(p); return { sent: p.to.map((to) => ({ to, ok: true })) }; },
      };
    },
  };
}

function db() {
  const m = new Map();
  return { m, get: async (k) => m.get(k), put: async (k, v) => m.set(k, v), del: async (k) => m.delete(k), all: async (p) => [...m.entries()].filter(([k]) => k.startsWith(p)).map(([, v]) => v) };
}

async function member(b, name, directory) {
  const dev = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']);
  const devPub = new Uint8Array(await crypto.subtle.exportKey('raw', dev.publicKey));
  const ikm = crypto.getRandomValues(new Uint8Array(32));
  const k = await hpke.derive(ikm);
  const me = {
    name, sent: [], directory,
    // the app's vouch: its own name and device put in by itself
    vouch: async ({ id, hs, hr }) => {
      const h = { kind: 'in', id, hs, hr, by: 'member', name, fp: `${name}-fp` };
      return { name, fp: `${name}-fp`, sig: b64(new Uint8Array(await crypto.subtle.sign('Ed25519', dev.privateKey, obj.signed(h)))) };
    },
  };
  directory[name] = { pk: hpke.tob64u(k.pk), devices: [{ fp: `${name}-fp`, key: b64(devPub) }] };
  const open = (store) => new Mailbox({ api: b.for(name, me), db: store, sk: k.sk, pk: k.pk, ikm, me: name, domain: 'commonty.test' });
  return { me, k, open };
}

// mail as the Worker locks it, and signs it
async function arrive(b, name, pk, raw, extra = {}, signer = workerPkcs8, id = newId()) {
  const s = { ...(await import('../mods/mail-mime.js')).summary(raw), id, at: Date.now(), ...extra };
  const { h, S, R } = await sealed(pk, 'in', id, s, raw);
  Object.assign(h, { by: 'worker', kid: 'w1' });
  h.sig = await obj.signEd25519(signer, h);
  b.objs.set(`${name}/in/${id}`, { bytes: obj.pack(h, S, R) });
  return id;
}

const shop = (n, to = 'shop.k7m2@commonty.test') => compose({ domain: 'shop.example', from: { name: 'Northside', address: 'orders@northside.example' }, to: [to], subject: `Order ${n} shipped`, text: `Two heat-set insert kits for order ${n}, arriving Tuesday.` }).raw;

test('what came opens, is listed newest first, and its body is searchable once read', async () => {
  const b = box(), dir = {};
  const t = await member(b, 'tester', dir);
  const mb = t.open(db());
  await mb.load();
  const a = await arrive(b, 'tester', t.k.pk, shop(1), { alias: 'shop.k7m2', rcpt: 'shop.k7m2@commonty.test' });
  await new Promise((r) => setTimeout(r, 2));
  const c = await arrive(b, 'tester', t.k.pk, shop(2, 'tester@commonty.test'));
  assert.equal(await mb.sync(), 2);
  assert.deepEqual(mb.folder('inbox').map((m) => m.id), [c, a]);
  assert.equal(mb.unread('inbox'), 2);
  assert.equal(mb.msgs.get(a).s.subject, 'Order 1 shipped');
  assert.deepEqual(mb.folder('inbox', 'shop.k7m2@commonty.test').map((m) => m.id), [a], 'by the alias it came to');
  const r = await mb.read(a);
  assert.match(r.plain, /heat-set insert kits/);
  assert.deepEqual((await mb.search('tuesday')).hits.map((m) => m.id), [c, a], 'the snippet before, the body after');
  assert.equal(await mb.sync(), 0, 'nothing new the second time');
});

test('mail a box stored after later mail still comes, and again after a day', async () => {
  const b = box(), dir = {};
  const t = await member(b, 'tester', dir);
  const mb = t.open(db());
  await mb.load();
  await arrive(b, 'tester', t.k.pk, shop(1));
  assert.equal(await mb.sync(), 1);
  // held by the Worker an hour while the box was away: an id from before
  const late = await arrive(b, 'tester', t.k.pk, shop(2), {}, workerPkcs8, newId(Date.now() - 3600000));
  assert.equal(await mb.sync(), 1, 'an id older than the last one seen');
  const old = await arrive(b, 'tester', t.k.pk, shop(3), {}, workerPkcs8, newId(Date.now() - 3 * 86400000));
  assert.equal(await mb.sync(), 0, 'older than a day back: the daily look from the start finds it');
  mb.fullAt = Date.now() - 86400001;
  assert.equal(await mb.sync(), 1);
  assert.ok(mb.msgs.has(late) && mb.msgs.has(old));
});

test('a change on one device is the same on the other', async () => {
  const b = box(), dir = {};
  const t = await member(b, 'tester', dir);
  const one = t.open(db()), two = t.open(db());
  await one.load(); await two.load();
  const a = await arrive(b, 'tester', t.k.pk, shop(1));
  await one.sync(); await two.sync();
  await one.change([a], { read: true, star: true });
  await one.change([a], { folder: 'archive' });
  await two.sync();
  const m = two.msgs.get(a);
  assert.equal(m.read, true); assert.equal(m.star, true); assert.equal(m.folder, 'archive');
  await two.change([a], { gone: true });
  await one.sync();
  assert.equal(one.msgs.has(a), false, 'deleted on both');
  assert.equal(b.objs.has(`tester/in/${a}`), false, 'and on the box');
});

test('a device keeps what it opened: a second start draws it before asking the box', async () => {
  const b = box(), dir = {};
  const t = await member(b, 'tester', dir);
  const store = db();
  const one = t.open(store);
  await one.load();
  const a = await arrive(b, 'tester', t.k.pk, shop(7));
  await one.sync();
  const again = t.open(store);
  await again.load();
  assert.equal(again.msgs.get(a).s.subject, 'Order 7 shipped');
  assert.deepEqual((await again.search('order 7')).hits.map((m) => m.id), [a]);
});

test('member to member: locked on the sender\'s device, checked on the reader\'s', async () => {
  const b = box(), dir = {};
  const t = await member(b, 'tester', dir);
  const s = await member(b, 'sam', dir);
  const sam = s.open(db()), tester = t.open(db());
  await sam.load(); await tester.load();
  const out = await sam.send({ from: { name: 'Sam', address: 'sam@commonty.test' }, to: ['tester@commonty.test'], subject: 'Lunch?', html: '<p>Noodles at <b>noon</b>.</p>' });
  assert.deepEqual(out.results, [{ to: 'tester@commonty.test', ok: true, e2e: true }]);
  assert.equal(s.me.sent.length, 0, 'nothing went the outside way');
  assert.equal(sam.folder('sent').length, 1, 'kept in Sent, locked');
  await tester.sync();
  const [m] = tester.folder('inbox');
  assert.equal(m.s.subject, 'Lunch?');
  assert.equal(m.by, 'member', 'signed by a device of Sam\'s');
  assert.equal(m.sender, 'sam');
  assert.match((await tester.read(m.id)).html, /Noodles at <b>noon<\/b>/);
  const raw = [...b.objs.keys()].find((k) => k.startsWith('tester/in/'));
  assert.ok(raw);
});

test('one the box says is from someone else, or changed after signing, is not believed', async () => {
  const b = box(), dir = {};
  const t = await member(b, 'tester', dir);
  const s = await member(b, 'sam', dir);
  const sam = s.open(db()), tester = t.open(db());
  await sam.load(); await tester.load();
  await sam.send({ from: 'sam@commonty.test', to: ['tester@commonty.test'], subject: 'Pay me', html: '<p>account 1</p>' });
  const k = [...b.objs.keys()].find((x) => x.startsWith('tester/in/'));
  b.objs.get(k).from = 'otter';
  await tester.sync();
  assert.equal(tester.msgs.size, 0, 'the box named another sender: not believed, not shown');
  // made up by the box: locked to their public key, signed by nobody they believe
  const raw = te.encode('From: sam@commonty.test\r\nSubject: Pay me\r\n\r\naccount 2');
  const fake = await sealed(t.k.pk, 'in', newId(), { subject: 'Pay me', from: { address: 'sam@commonty.test' } }, raw);
  Object.assign(fake.h, { by: 'worker', kid: 'w1', sig: 'AAAA' });
  b.objs.set(`tester/in/${fake.h.id}`, { bytes: obj.pack(fake.h, fake.S, fake.R) });
  const other = await crypto.subtle.generateKey('Ed25519', true, ['sign', 'verify']);
  await arrive(b, 'tester', t.k.pk, raw, {}, new Uint8Array(await crypto.subtle.exportKey('pkcs8', other.privateKey)));
  const mine = await sealed(t.k.pk, 'state', newId(), { ops: [] }, new Uint8Array(0));
  Object.assign(mine.h, { by: 'me', mac: 'AAAA' });
  b.objs.set(`tester/state/${mine.h.id}`, { bytes: obj.pack(mine.h, mine.S, mine.R) });
  b.objs.set(`tester/in/${newId()}`, { bytes: obj.pack(mine.h, mine.S, mine.R) });
  await tester.sync();
  assert.equal(tester.msgs.size, 0, 'a forged signature, another key, a change it made, an object kept under another name: none believed');
  // and a believed one whose message the box swapped does not open
  const good = await arrive(b, 'tester', t.k.pk, raw);
  await tester.sync();
  const blob = b.objs.get(`tester/in/${good}`).bytes;
  const parts = obj.unpack(blob);
  const swapped = await sealed(t.k.pk, 'in', good, {}, te.encode('From: sam@commonty.test\r\n\r\naccount 3'));
  b.objs.set(`tester/in/${good}`, { bytes: obj.pack(parts.h, parts.summary, swapped.R) });
  await assert.rejects(tester.read(good), /changed after it was locked/);
});

test('outside: through the app, a copy in Sent; a member with no inbox the same way', async () => {
  const b = box(), dir = {};
  const t = await member(b, 'tester', dir);
  const me = t.open(db());
  await me.load();
  const out = await me.send({ from: 'tester@commonty.test', to: ['friend@example.net'], cc: ['nobody@commonty.test'], bcc: ['hidden@example.org'], subject: 'Hi', html: '<p>hello</p>' });
  assert.equal(t.me.sent.length, 1);
  const p = t.me.sent[0];
  assert.deepEqual(p.to.sort(), ['friend@example.net', 'hidden@example.org', 'nobody@commonty.test'].sort());
  const raw = Buffer.from(p.raw, 'base64url').toString();
  assert.match(raw, /^From: tester@commonty\.test\r\n/);
  assert.ok(!/hidden@example\.org/.test(raw), 'Bcc is the envelope\'s, not the message\'s');
  assert.ok(out.results.every((r) => r.ok));
  const [sent] = me.folder('sent');
  assert.deepEqual(sent.s.bcc, ['hidden@example.org'], 'Sent remembers who was Bcc\'d');
});

test('spam: marked so, learnt; drafts kept and dropped; an mbox brought in', async () => {
  const b = box(), dir = {};
  const t = await member(b, 'tester', dir);
  const me = t.open(db());
  await me.load();
  const spams = [], hams = [];
  for (let i = 0; i < 6; i++) {
    spams.push(await arrive(b, 'tester', t.k.pk, compose({ domain: 'x', from: 'win@prizes.example', to: ['tester@commonty.test'], subject: `You won a prize ${i}`, text: 'claim your gift card prize now winner' }).raw));
    hams.push(await arrive(b, 'tester', t.k.pk, compose({ domain: 'x', from: 'sam@commonty.test', to: ['tester@commonty.test'], subject: `Lunch ${i}`, text: 'noodles at noon with the team' }).raw));
  }
  await me.sync();
  await me.change(spams, { folder: 'spam' });
  await me.change(hams, { read: true });
  for (const h of hams) me.filter.learn(me.docOf(me.msgs.get(h)), false);
  const next = await arrive(b, 'tester', t.k.pk, compose({ domain: 'x', from: 'win@prizes.example', to: ['tester@commonty.test'], subject: 'You won a prize again', text: 'claim your gift card prize now winner' }).raw);
  await me.sync();
  assert.equal(me.msgs.get(next).folder, 'spam', 'sorted by what was learnt');

  const d = await me.saveDraft({ from: 'tester@commonty.test', to: ['a@b.example'], subject: 'half written', html: '<p>so far</p>' });
  assert.equal(me.folder('drafts')[0].s.subject, 'half written');
  await me.discard(d);
  assert.equal(me.folder('drafts').length, 0);

  const box2 = 'From someone Sat Jan  1 00:00:00 2022\nFrom: old@example.com\nSubject: from 2022\n\n>From the archive\n\nFrom x Sun Jan  2 00:00:00 2022\nFrom: b@example.com\nSubject: second\n\nbody\n';
  assert.equal(await me.import(te.encode(box2)), 2);
  assert.equal((await me.search('archive')).hits.length, 1, 'found by its snippet before it is opened');
  assert.deepEqual((await me.search('subject:2022')).hits.map((m) => m.s.subject), ['from 2022']);
  const [imp] = (await me.search('subject:2022')).hits;
  assert.match((await me.read(imp.id)).plain, /^From the archive/, 'mbox ">From" unquoted');
});
