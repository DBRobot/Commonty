// The locking in hpke.js against RFC 9180's own vector (A.1.1, base mode,
// X25519 with AES-128-GCM), then a round trip in the AES-256 mail uses.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import * as hpke from '../mods/mail-hpke.js';

const hex = (s) => Uint8Array.from(s.replace(/\s/g, '').match(/../g), (h) => parseInt(h, 16));
const toHex = (u) => Buffer.from(u).toString('hex');

const V = {
  info: '4f6465206f6e2061204772656369616e2055726e',
  ikmE: '7268600d403fce431561aef583ee1613527cff655c1343f29812e66706df3234',
  pkEm: '37fda3567bdbd628e88668c3c8d7e97d1d1253b6d4ea6d44c150f741f1bf4431',
  ikmR: '6db9df30aa07dd42ee5e8181afdb977e538f5e1fec8a06223f33f7013e525037',
  pkRm: '3948cfe0ad1ddb695d780e59077195da6c56506b027329794ab02bca80815c4d',
  skRm: '4612c550263fc8ad58375df3f557aac531d26850903e55a9f23f21d8534e8ac8',
  pt: '4265617574792069732074727574682c20747275746820626561757479',
  ct: [
    ['436f756e742d30', 'f938558b5d72f1a23810b4be2ab4f84331acc02fc97babc53a52ae8218a355a96d8770ac83d07bea87e13c512a'],
    ['436f756e742d31', 'af2d7e9ac9ae7e270f46ba1f975be53c09f8d875bdc8535458c2494e8a6eab251c03d0c22a56b8ca42c2063b84'],
    ['436f756e742d32', '498dfcabd92e8acedc281e85af1cb4e3e31c7dc394a1ca20e173cb72516491588d96a19ad4a683518973dcc180'],
  ],
};

test('DeriveKeyPair gives the RFC keys', async () => {
  const r = await hpke.derive(hex(V.ikmR));
  assert.equal(toHex(r.sk), V.skRm);
  assert.equal(toHex(r.pk), V.pkRm);
  assert.equal(toHex((await hpke.derive(hex(V.ikmE))).pk), V.pkEm);
});

test('sealing matches the RFC ciphertexts, in sequence', async () => {
  const s = await hpke.sender(hex(V.pkRm), hex(V.info), hpke.AES128, hex(V.ikmE));
  assert.equal(toHex(s.enc), V.pkEm);
  for (const [aad, ct] of V.ct) assert.equal(toHex(await s.seal(hex(V.pt), hex(aad))), ct);
});

test('opening the RFC ciphertexts', async () => {
  const r = await hpke.receiver(hex(V.skRm), hex(V.pkRm), hex(V.pkEm), hex(V.info), hpke.AES128);
  for (const [aad, ct] of V.ct) assert.equal(toHex(await r.open(hex(ct), hex(aad))), V.pt);
});

test('a mail blob opens with its key and info only', async () => {
  const me = await hpke.generate();
  const other = await hpke.generate();
  const body = new TextEncoder().encode('Subject: hi\r\n\r\nhello');
  const blob = await hpke.seal(me.pk, 'commonty mail v1 raw', body);
  assert.deepEqual(await hpke.open(me.sk, me.pk, 'commonty mail v1 raw', blob), body);
  await assert.rejects(hpke.open(other.sk, other.pk, 'commonty mail v1 raw', blob));
  await assert.rejects(hpke.open(me.sk, me.pk, 'commonty mail v1 summary', blob));
  const bad = blob.slice(); bad[bad.length - 1] ^= 1;
  await assert.rejects(hpke.open(me.sk, me.pk, 'commonty mail v1 raw', bad));
  await assert.rejects(hpke.open(me.sk, me.pk, 'x', new Uint8Array(10)));
});

test('a page keeps the private key as a CryptoKey it cannot export', async () => {
  const me = await hpke.generate();
  const key = await hpke.privateKey(me.sk);
  assert.equal(key.extractable, false);
  const blob = await hpke.seal(me.pk, 'i', Uint8Array.of(1, 2, 3));
  assert.deepEqual(await hpke.open(key, me.pk, 'i', blob), Uint8Array.of(1, 2, 3));
});

test('a 25 MB message locks', async () => {
  const me = await hpke.generate();
  const big = new Uint8Array(25 * 1024 * 1024).fill(7);
  const t = performance.now();
  const blob = await hpke.seal(me.pk, 'raw', big);
  const ms = performance.now() - t;
  assert.equal(blob.length, big.length + 36 + 16);
  console.log(`25 MB sealed in ${ms.toFixed(1)} ms`);
});
