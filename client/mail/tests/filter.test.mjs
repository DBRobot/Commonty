import { test } from 'node:test';
import assert from 'node:assert/strict';
import { Filter } from '../mods/mail-filter.js';

const spam = (i) => ({ from: { name: 'Prize Desk', address: `win${i}@prizes-now.example` }, subject: `You won a $500 gift card ${i}`, body: 'claim your prize now, confirm your details before midnight, winner' });
const ham = (i) => ({ from: { name: 'Sam', address: 'sam@commonty.org' }, subject: `Lunch on Friday ${i}`, body: 'noodles at noon, the game server is back, see you then' });

test('learns from what they mark, and keeps its word to itself', () => {
  const f = new Filter();
  assert.equal(f.isSpam(spam(0)), false, 'knows nothing yet: nothing is spam');
  for (let i = 0; i < 10; i++) { f.learn(spam(i), true); f.learn(ham(i), false); }
  assert.equal(f.isSpam(spam(99)), true);
  assert.equal(f.isSpam(ham(99)), false);
  const again = new Filter(JSON.parse(JSON.stringify(f)));
  assert.equal(again.isSpam(spam(98)), true, 'kept and read back, it is the same');
});

test('Not spam takes it back', () => {
  const f = new Filter();
  for (let i = 0; i < 10; i++) { f.learn(spam(i), true); f.learn(ham(i), false); }
  const mixed = { from: { name: 'Prize Desk', address: 'news@prizes-now.example' }, subject: 'Your monthly prize newsletter', body: 'claim your prize now winner' };
  assert.equal(f.isSpam(mixed), true);
  for (let i = 0; i < 12; i++) f.moved({ ...mixed, subject: `${mixed.subject} ${i}` }, false);
  assert.equal(f.isSpam(mixed), false);
});

test('a forged sender and a runnable attachment', () => {
  const f = new Filter();
  assert.equal(f.isSpam({ ...ham(1), auth: { dmarc: 'fail' } }), true, 'failed its own domain\'s checks');
  assert.equal(f.isSpam({ ...ham(1), attachments: ['invoice.pdf.exe'] }), true);
  assert.equal(f.isSpam({ ...ham(1), attachments: ['invoice.pdf'] }), false);
});
