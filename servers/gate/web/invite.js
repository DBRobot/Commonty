// An invite code, as the browser holds it: the seed of an ed25519 key. Only
// the public key and a signature ever leave the page; the box that issued
// the invite recognises the public key.

import { b64u, u8b64 } from './webauthn.js';

const enc = new TextEncoder();

async function codeKey(code) {
  const normalised = code.replace(/[^A-Za-z0-9]/g, '').toLowerCase();
  const seed = new Uint8Array(await crypto.subtle.digest('SHA-256', enc.encode('dd-invite:' + normalised)));
  // a PKCS#8 wrapper around the raw 32-byte seed
  const pkcs8 = new Uint8Array([
    0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04, 0x20,
    ...seed,
  ]);
  const key = await crypto.subtle.importKey('pkcs8', pkcs8, { name: 'Ed25519' }, true, ['sign']);
  const jwk = await crypto.subtle.exportKey('jwk', key);
  return { key, pub: btoa(String.fromCharCode(...b64u(jwk.x))) };
}

// is there an invite under this code, anywhere in the directory?
export async function checkInvite(code) {
  const { pub } = await codeKey(code);
  const r = await fetch('/_dd/invite/' + u8b64(enc.encode(pub)));
  if (!r.ok) throw new Error('That code is not valid, or it has expired.');
}

// the grant: this root redeems this invite, now, signed with the code's key
export async function claim(code, root) {
  const { key, pub } = await codeKey(code);
  const redeemed = Math.floor(Date.now() / 1000);
  const message = enc.encode(JSON.stringify({ invite: pub, root, redeemed }));
  const sig = new Uint8Array(await crypto.subtle.sign('Ed25519', key, message));
  return { invite_public_key: pub, redeemed, proof: btoa(String.fromCharCode(...sig)) };
}
