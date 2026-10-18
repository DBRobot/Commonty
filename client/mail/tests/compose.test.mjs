import { test } from 'node:test';
import assert from 'node:assert/strict';
import { compose, encodeWords, textOf } from '../mods/mail-compose.js';
import * as mime from '../mods/mail-mime.js';

test('what is written reads back as written', () => {
  const { raw, mid } = compose({
    domain: 'commonty.org',
    from: { name: 'Dávid', address: 'david@commonty.org' },
    to: [{ name: 'Sam', address: 'sam@example.net' }, 'lingyun@commonty.org'],
    cc: ['otter@example.org'],
    subject: 'Lunch — Friday? 🍜',
    text: 'See you at noon.\n.a dotted line\ncafé',
    html: '<p>See you at <b>noon</b>.</p>',
    attachments: [{ name: 'résumé.pdf', type: 'application/pdf', bytes: new Uint8Array([37, 80, 68, 70]) }],
    inReplyTo: '<1@x>',
    references: ['<0@x>', '<1@x>'],
  });
  const s = new TextDecoder().decode(raw);
  assert.ok(!/(?<!\r)\n/.test(s), 'every line ends CRLF');
  assert.ok(s.split('\r\n').every((l) => l.length <= 998), 'no line too long');
  assert.ok(!/^bcc:/im.test(s));
  const sum = mime.summary(raw);
  assert.equal(sum.subject, 'Lunch — Friday? 🍜');
  assert.equal(sum.from.name, 'Dávid');
  assert.deepEqual(sum.to.map((a) => a.address), ['sam@example.net', 'lingyun@commonty.org']);
  assert.deepEqual(sum.cc.map((a) => a.address), ['otter@example.org']);
  assert.equal(sum.mid, mid);
  assert.equal(sum.irt, '<1@x>');
  assert.deepEqual(sum.attachments, ['résumé.pdf']);
  const r = mime.readable(mime.parse(raw));
  assert.equal(r.plain, 'See you at noon.\r\n.a dotted line\r\ncafé');
  assert.equal(r.html.trim(), '<p>See you at <b>noon</b>.</p>');
  assert.deepEqual([...r.attachments[0].body], [37, 80, 68, 70]);
});

test('a header cannot be made from a value', () => {
  assert.throws(() => compose({ domain: 'x', from: 'a@b.c', to: ['x@y.z\r\nBcc: evil@e.vil'], subject: 's', text: '' }));
  const { raw } = compose({ domain: 'x', from: 'a@b.c', to: ['x@y.z'], subject: 'hi\r\nBcc: evil@e.vil', text: '' });
  assert.ok(!/^Bcc:/m.test(new TextDecoder().decode(raw)));
});

test('long encoded words fold', () => {
  const w = encodeWords('ü'.repeat(100));
  assert.ok(w.split('\r\n').every((l) => l.length < 78));
  assert.equal(mime.words(w.replace(/\r\n /g, ' ')), 'ü'.repeat(100));
  assert.equal(textOf('<p>a</p><ul><li>b</li></ul>x&amp;y<br>z'), 'a\n• b\nx&y\nz');
});
