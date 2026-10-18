// One locked object of a member's mail, as the box keeps it and the Worker
// holds it (docs/dev/mail.md): locked to the member's key, and signed by
// whoever made it, so a box - which can lock anything to a public key -
// can neither make one up nor change one.
//
//   "CMA1" | u32 header length | header | u32 summary length | summary | message
//
// The summary and the message are sealed (mail-hpke.js); the header is
// JSON, in the open: {kind, id, hs, hr, by, ...} where hs and hr are the
// sha256 of the sealed summary and message, and
//   by "worker": sig, the mail Worker's Ed25519 key (kid names which)
//   by "me":     mac, HMAC by a key only the member's devices have
//   by "member": name, fp, sig, a device of that member's
// over "commonty mail object v1\0<kind>\0<id>\0<hs>\0<hr>[\0<name>\0<fp>]".

const te = new TextEncoder();
const td = new TextDecoder();
const MAGIC = te.encode('CMA1');

export function tob64u(u) {
  let s = '';
  for (let i = 0; i < u.length; i += 0x8000) s += String.fromCharCode(...u.subarray(i, i + 0x8000));
  return btoa(s).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
}
const b64 = (u) => btoa(String.fromCharCode(...new Uint8Array(u)));
const unb64 = (s) => Uint8Array.from(atob(String(s).replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));
const sha = async (b) => tob64u(new Uint8Array(await crypto.subtle.digest('SHA-256', b)));

export function signed(h) {
  return te.encode(`commonty mail object v1\0${h.kind}\0${h.id}\0${h.hs}\0${h.hr}${h.by === 'member' ? `\0${h.name}\0${h.fp}` : ''}`);
}

// the header for these sealed parts, not yet vouched for
export async function header(kind, id, sealedSummary, sealedRaw) {
  return { kind, id, hs: await sha(sealedSummary), hr: await sha(sealedRaw) };
}

export function pack(h, sealedSummary, sealedRaw) {
  const hb = te.encode(JSON.stringify(h));
  const out = new Uint8Array(4 + 4 + hb.length + 4 + sealedSummary.length + sealedRaw.length);
  const v = new DataView(out.buffer);
  out.set(MAGIC, 0);
  v.setUint32(4, hb.length);
  out.set(hb, 8);
  v.setUint32(8 + hb.length, sealedSummary.length);
  out.set(sealedSummary, 12 + hb.length);
  out.set(sealedRaw, 12 + hb.length + sealedSummary.length);
  return out;
}

// how many bytes from the front hold the header and the summary: null when
// `front` is too short to tell
export function headLength(front) {
  if (front.length < 8) return null;
  const v = new DataView(front.buffer, front.byteOffset, front.length);
  const hl = v.getUint32(4);
  if (front.length < 12 + hl) return 12 + hl + 1;
  return 12 + hl + v.getUint32(8 + hl);
}

// the parts of a locked object (or of its front, for the summary alone)
export function unpack(b) {
  if (b.length < 12 || MAGIC.some((x, i) => b[i] !== x)) throw new Error('not a locked object');
  const v = new DataView(b.buffer, b.byteOffset, b.length);
  const hl = v.getUint32(4);
  if (hl > 64 * 1024 || 12 + hl > b.length) throw new Error('not a locked object');
  const h = JSON.parse(td.decode(b.subarray(8, 8 + hl)));
  const sl = v.getUint32(8 + hl);
  const s0 = 12 + hl;
  if (s0 + sl > b.length) throw new Error('cut short');
  return { h, summary: b.subarray(s0, s0 + sl), raw: b.subarray(s0 + sl), whole: true };
}

export async function checkSummary(h, kind, id, summary) {
  if (h.kind !== kind || h.id !== id) throw new Error('not the object it is kept as');
  if ((await sha(summary)) !== h.hs) throw new Error('its summary was changed');
}
export async function checkRaw(h, raw) {
  if ((await sha(raw)) !== h.hr) throw new Error('it was changed after it was locked');
}

export async function signEd25519(pkcs8, h) {
  const k = await crypto.subtle.importKey('pkcs8', pkcs8, { name: 'Ed25519' }, false, ['sign']);
  return b64(await crypto.subtle.sign('Ed25519', k, signed(h)));
}
export async function verifyEd25519(pub, h) {
  try {
    const k = await crypto.subtle.importKey('raw', typeof pub === 'string' ? unb64(pub) : pub, { name: 'Ed25519' }, false, ['verify']);
    return await crypto.subtle.verify('Ed25519', k, unb64(h.sig || ''), signed(h));
  } catch {
    return false;
  }
}

// the member's own key for vouching, from their mail key's 32 bytes: a
// device has it; a box never does
export async function macKey(ikm) {
  const base = await crypto.subtle.importKey('raw', ikm, 'HKDF', false, ['deriveKey']);
  return crypto.subtle.deriveKey({ name: 'HKDF', hash: 'SHA-256', salt: new Uint8Array(32), info: te.encode('commonty mail mac v1') }, base, { name: 'HMAC', hash: 'SHA-256', length: 256 }, false, ['sign', 'verify']);
}
export async function mac(key, h) {
  return tob64u(new Uint8Array(await crypto.subtle.sign('HMAC', key, signed(h))));
}
export async function macOk(key, h) {
  try { return await crypto.subtle.verify('HMAC', key, unb64(h.mac || ''), signed(h)); } catch { return false; }
}
