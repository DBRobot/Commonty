// HPKE (RFC 9180), base mode, one message per key: DHKEM(X25519,
// HKDF-SHA256), HKDF-SHA256, AES-GCM. WebCrypto alone, so the Worker and the
// mail page share it and the bulk AES runs native. Mail is AES-256-GCM; the
// RFC's own test vector is AES-128-GCM, hence the choice of aead.

const te = new TextEncoder();
const subtle = crypto.subtle;
const PKCS8_X25519 = Uint8Array.from([0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x04, 0x22, 0x04, 0x20]);

export const AES128 = 1;
export const AES256 = 2;

const cat = (...a) => {
  const out = new Uint8Array(a.reduce((n, x) => n + x.length, 0));
  let o = 0;
  for (const x of a) { out.set(x, o); o += x.length; }
  return out;
};
const u16 = (n) => Uint8Array.of(n >> 8, n & 255);

async function hmac(key, data) {
  const k = await subtle.importKey('raw', key.length ? key : new Uint8Array(32), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  return new Uint8Array(await subtle.sign('HMAC', k, data));
}
async function expand(prk, info, len) {
  const out = new Uint8Array(len);
  let t = new Uint8Array(0);
  for (let i = 1, o = 0; o < len; i++) {
    t = await hmac(prk, cat(t, info, Uint8Array.of(i)));
    out.set(t.subarray(0, Math.min(t.length, len - o)), o);
    o += t.length;
  }
  return out;
}
const lextract = (suite, salt, label, ikm) => hmac(salt, cat(te.encode('HPKE-v1'), suite, te.encode(label), ikm));
const lexpand = (suite, prk, label, info, len) => expand(prk, cat(u16(len), te.encode('HPKE-v1'), suite, te.encode(label), info), len);

const KEM = cat(te.encode('KEM'), u16(0x20));
const suiteOf = (aead) => cat(te.encode('HPKE'), u16(0x20), u16(1), u16(aead));

export async function privateKey(sk, extractable = false) {
  return subtle.importKey('pkcs8', cat(PKCS8_X25519, sk), { name: 'X25519' }, extractable, ['deriveBits']);
}
export async function publicKey(pk) {
  return subtle.importKey('raw', pk, { name: 'X25519' }, true, []);
}
export async function publicOf(sk) {
  const jwk = await subtle.exportKey('jwk', await privateKey(sk, true));
  return b64u(jwk.x);
}
// a key pair from 32 bytes of seed (RFC 9180 DeriveKeyPair)
export async function derive(ikm) {
  const prk = await lextract(KEM, new Uint8Array(0), 'dkp_prk', ikm);
  const sk = await lexpand(KEM, prk, 'sk', new Uint8Array(0), 32);
  return { sk, pk: await publicOf(sk) };
}
export async function generate() {
  return derive(crypto.getRandomValues(new Uint8Array(32)));
}

async function dh(sk, pk) {
  const priv = sk instanceof CryptoKey ? sk : await privateKey(sk);
  return new Uint8Array(await subtle.deriveBits({ name: 'X25519', public: await publicKey(pk) }, priv, 256));
}
async function shared(dhv, enc, pkR) {
  const prk = await lextract(KEM, new Uint8Array(0), 'eae_prk', dhv);
  return lexpand(KEM, prk, 'shared_secret', cat(enc, pkR), 32);
}
async function schedule(aead, ss, info) {
  const suite = suiteOf(aead), nk = aead === AES256 ? 32 : 16, none = new Uint8Array(0);
  const ctx = cat(Uint8Array.of(0), await lextract(suite, none, 'psk_id_hash', none), await lextract(suite, none, 'info_hash', info));
  const secret = await lextract(suite, ss, 'secret', none);
  const key = await lexpand(suite, secret, 'key', ctx, nk);
  const nonce = await lexpand(suite, secret, 'base_nonce', ctx, 12);
  return { key: await subtle.importKey('raw', key, 'AES-GCM', false, ['encrypt', 'decrypt']), nonce };
}
const seqNonce = (base, seq) => {
  const n = base.slice();
  for (let i = 11, s = seq; s > 0 && i >= 0; i--, s = Math.floor(s / 256)) n[i] ^= s & 255;
  return n;
};

// a sender's context; `eph` (a known ephemeral seed) is for the RFC's vectors only
export async function sender(pkR, info, aead = AES256, eph) {
  const e = eph ? await derive(eph) : await generate();
  const ss = await shared(await dh(e.sk, pkR), e.pk, pkR);
  const s = await schedule(aead, ss, info);
  let seq = 0;
  return {
    enc: e.pk,
    seal: async (pt, aad = new Uint8Array(0)) => new Uint8Array(await subtle.encrypt({ name: 'AES-GCM', iv: seqNonce(s.nonce, seq++), additionalData: aad }, s.key, pt)),
  };
}
// skR: 32 raw bytes, or a non-extractable CryptoKey kept by the page
export async function receiver(skR, pkR, enc, info, aead = AES256) {
  const ss = await shared(await dh(skR, enc), enc, pkR);
  const s = await schedule(aead, ss, info);
  let seq = 0;
  return {
    open: async (ct, aad = new Uint8Array(0)) => new Uint8Array(await subtle.decrypt({ name: 'AES-GCM', iv: seqNonce(s.nonce, seq++), additionalData: aad }, s.key, ct)),
  };
}

// one sealed blob: "CML1" | enc (32) | ciphertext, info binding what it is
const MAGIC = te.encode('CML1');
export async function seal(pkR, info, pt) {
  const s = await sender(pkR, te.encode(info));
  return cat(MAGIC, s.enc, await s.seal(pt));
}
export async function open(skR, pkR, info, blob) {
  if (blob.length < 36 + 16 || MAGIC.some((b, i) => blob[i] !== b)) throw new Error('not a locked message');
  const r = await receiver(skR, pkR, blob.subarray(4, 36), te.encode(info));
  return r.open(blob.subarray(36));
}

export function b64u(s) {
  const b = atob(s.replace(/-/g, '+').replace(/_/g, '/'));
  return Uint8Array.from(b, (c) => c.charCodeAt(0));
}
export function tob64u(u) {
  let s = '';
  for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000));
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
