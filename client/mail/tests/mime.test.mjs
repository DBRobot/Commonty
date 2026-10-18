import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as mime from '../mods/mail-mime.js';

const enc = (s) => new TextEncoder().encode(s.replace(/\n/g, '\r\n'));

test('encoded words, split across lines, in two charsets', () => {
  assert.equal(mime.words('=?UTF-8?B?w6l0w6k=?= =?ISO-8859-1?Q?caf=E9?='), 'étécafé');
  assert.equal(mime.words('=?utf-8?q?Hello_World?= and more'), 'Hello World and more');
  assert.equal(mime.words('plain'), 'plain');
});

test('addresses with names, quotes and commas', () => {
  assert.deepEqual(mime.addresses('"Doe, Jane" <Jane@Example.com>, bob@x.org, =?utf-8?q?Zo=C3=AB?= <zoe@y.io>'), [
    { name: 'Doe, Jane', address: 'jane@example.com' },
    { name: '', address: 'bob@x.org' },
    { name: 'Zoë', address: 'zoe@y.io' },
  ]);
});

test('RFC 2231 file names', () => {
  const p = mime.params("attachment; filename*=UTF-8''r%C3%A9sum%C3%A9.pdf");
  assert.equal(p.value, 'attachment');
  assert.equal(p.filename, 'résumé.pdf');
  const q = mime.params('attachment; filename*0="long"; filename*1="name.txt"');
  assert.equal(q.filename, 'longname.txt');
});

const shop = enc(`From: Northside Hardware <orders@northside.example>
To: shop.k7m2@commonty.org
Subject: =?utf-8?q?Your_order_=E2=80=94_shipped?=
Date: Sat, 17 Oct 2026 08:15:00 -0400
Message-ID: <48213@northside.example>
List-Unsubscribe: <https://northside.example/u>
MIME-Version: 1.0
Content-Type: multipart/mixed; boundary="mix"

preamble
--mix
Content-Type: multipart/alternative; boundary=alt

--alt
Content-Type: text/plain; charset=utf-8
Content-Transfer-Encoding: quoted-printable

Two kits are on their way =E2=80=94 Tuesday.=
 Thanks!
--alt
Content-Type: text/html; charset=utf-8
Content-Transfer-Encoding: base64

PHA+VHdvIGtpdHMgYXJlIG9uIHRoZWlyIHdheS48L3A+
--alt--
--mix
Content-Type: application/pdf; name="invoice.pdf"
Content-Disposition: attachment; filename="invoice.pdf"
Content-Transfer-Encoding: base64

JVBERi0xLjQK
--mix--
`);

test('a multipart order: html and plain, an attachment', () => {
  const root = mime.parse(shop);
  const r = mime.readable(root);
  assert.equal(r.html, '<p>Two kits are on their way.</p>');
  assert.equal(r.plain.trim(), 'Two kits are on their way — Tuesday. Thanks!');
  assert.equal(r.attachments.length, 1);
  assert.equal(r.attachments[0].filename, 'invoice.pdf');
  assert.equal(new TextDecoder().decode(r.attachments[0].body), '%PDF-1.4\n');
});

test('the summary the list shows', () => {
  const s = mime.summary(shop);
  assert.equal(s.from.address, 'orders@northside.example');
  assert.equal(s.from.name, 'Northside Hardware');
  assert.equal(s.subject, 'Your order — shipped');
  assert.equal(s.to[0].address, 'shop.k7m2@commonty.org');
  assert.equal(s.mid, '<48213@northside.example>');
  assert.ok(s.snippet.startsWith('Two kits are on their way'));
  assert.deepEqual(s.attachments, ['invoice.pdf']);
  assert.equal(s.list, true);
  assert.equal(s.date, Date.parse('2026-10-17T12:15:00Z'));
});

test('a bare text message, LF line ends, a latin-1 body', () => {
  const raw = Uint8Array.from([...new TextEncoder().encode('From: a@b.c\nSubject: hi\nContent-Type: text/plain; charset=iso-8859-1\n\ncaf'), 0xe9]);
  const r = mime.readable(mime.parse(raw));
  assert.equal(r.plain, 'café');
});

test('html only: the snippet is its text', () => {
  const raw = enc('From: x@y.z\nContent-Type: text/html\n\n<html><head><style>p{}</style></head><body><p>Hello&nbsp;<b>there</b></p><script>bad()</script></body></html>');
  assert.equal(mime.summary(raw).snippet, 'Hello there');
});

test('a forwarded message is an attachment of its own', () => {
  const raw = enc('From: a@b.c\nContent-Type: multipart/mixed; boundary=b\n\n--b\nContent-Type: text/plain\n\nsee below\n--b\nContent-Type: message/rfc822\n\nFrom: z@z.z\nSubject: inner\n\ninner body\n--b--\n');
  const r = mime.readable(mime.parse(raw));
  assert.equal(r.plain, 'see below');
  assert.equal(r.attachments[0].type, 'message/rfc822');
  assert.equal(mime.summary(r.attachments[0].body).subject, 'inner');
});

test('inline images by cid are not attachments', () => {
  const raw = enc('From: a@b.c\nContent-Type: multipart/related; boundary=r\n\n--r\nContent-Type: text/html\n\n<img src="cid:logo">\n--r\nContent-Type: image/png\nContent-ID: <logo>\nContent-Disposition: inline; filename=logo.png\nContent-Transfer-Encoding: base64\n\niVBORw0K\n--r--\n');
  const r = mime.readable(mime.parse(raw));
  assert.equal(r.attachments.length, 0);
  assert.ok(r.inline.logo);
});

test('garbage does not throw', () => {
  for (const s of ['', '\n\n', 'no headers at all', 'Content-Type: multipart/mixed; boundary=x\n\n--x\n', 'Content-Transfer-Encoding: base64\n\n!!!!']) {
    mime.summary(enc(s));
  }
});
