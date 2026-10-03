// Unlocking a box's disks at boot. A box's disk key is two halves: one
// sealed in its own TPM, one kept here. This half goes only to that box -
// a request signed by the key its TPM holds, named in the signed fleet list
// (fleet.js BOXES) - and only when it is at home: from the house's internet
// address, or seeing the house's router, as it last reported while running.
// Anything else waits for a member to approve it here with their passkey,
// and a box marked stolen is never answered.
//
// KV (PINNED): share:<box> the half; home:<box> {ip, mac, at};
// pending:<box> {ip, mac, at}; approved:<box> {ip, mac, until};
// stolen:<box> "1"; seen:<box> the newest signed time taken.

import { BOXES } from './fleet.js';

const SKEW = 600;
const APPROVAL = 15 * 60;
const MAC = /^[0-9a-f]{2}(:[0-9a-f]{2}){5}$/;

const enc = new TextEncoder();
const unb64 = (s) => Uint8Array.from(atob(String(s).replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));

// An ECDSA signature as WebCrypto wants it, r||s, from the DER a TPM gives
function rawSig(sig) {
  if (sig.length === 64) return sig;
  if (sig[0] !== 0x30) throw new Error('not a signature');
  let i = 2;
  const part = () => {
    if (sig[i] !== 0x02) throw new Error('not a signature');
    const n = sig[i + 1];
    let v = sig.slice(i + 2, i + 2 + n);
    i += 2 + n;
    while (v.length > 32 && v[0] === 0) v = v.slice(1);
    const out = new Uint8Array(32);
    out.set(v, 32 - v.length);
    return out;
  };
  const r = part();
  const s = part();
  const out = new Uint8Array(64);
  out.set(r);
  out.set(s, 32);
  return out;
}

async function signedBy(box, message, sig) {
  const pub = BOXES[box];
  if (!pub) return false;
  try {
    const key = await crypto.subtle.importKey('spki', unb64(pub), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['verify']);
    return await crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, key, rawSig(unb64(sig)), enc.encode(message));
  } catch {
    return false;
  }
}

const getJson = async (env, k) => {
  const v = await env.PINNED.get(k);
  return v ? JSON.parse(v) : null;
};

// a signed time, fresh and newer than any taken before: a recorded request
// cannot be sent again
async function fresh(env, box, at) {
  const t = Number(at);
  const now = Math.floor(Date.now() / 1000);
  if (!Number.isInteger(t) || Math.abs(now - t) > SKEW) return false;
  const seen = Number((await env.PINNED.get(`seen:${box}`)) || 0);
  if (t <= seen) return false;
  await env.PINNED.put(`seen:${box}`, String(t));
  return true;
}

function where(req, mac) {
  // set by Cloudflare itself on every request: a client cannot name its own
  return { ip: req.headers.get('cf-connecting-ip') || '', mac: MAC.test(mac || '') ? mac : '' };
}

/// The routes a box calls, from its early boot or while running. Signed,
/// not signed in: no Origin, no cookie.
export async function boxApi(req, env, path, body, json) {
  if (req.method !== 'POST') return json({ error: 'not here' }, 404);
  const { box, at, mac, sig } = body;
  if (!BOXES[box]) return json({ error: 'no such box' }, 403);
  const here = where(req, mac);

  // while running: where home is
  if (path === '/api/unlock/checkin') {
    if (!(await signedBy(box, `commonty unlock checkin v1\0${box}\0${at}\0${mac || ''}`, sig))) return json({ error: 'not signed by that box' }, 403);
    if (!(await fresh(env, box, at))) return json({ error: 'stale' }, 403);
    if (await env.PINNED.get(`stolen:${box}`)) return json({ error: 'this box is marked stolen' }, 403);
    await env.PINNED.put(`home:${box}`, JSON.stringify({ ...here, at: Number(at) }));
    return json({ ok: true });
  }

  // once, when the disks are first encrypted: this half, kept here
  if (path === '/api/unlock/share') {
    const { share } = body;
    if (!/^[A-Za-z0-9_-]{43}$/.test(share || '')) return json({ error: 'not a share' }, 400);
    if (!(await signedBy(box, `commonty unlock share v1\0${box}\0${at}\0${share}`, sig))) return json({ error: 'not signed by that box' }, 403);
    if (!(await fresh(env, box, at))) return json({ error: 'stale' }, 403);
    if (await env.PINNED.get(`share:${box}`)) return json({ error: 'this box already has its half here' }, 409);
    await env.PINNED.put(`share:${box}`, share);
    await env.PINNED.put(`home:${box}`, JSON.stringify({ ...here, at: Number(at) }));
    return json({ ok: true });
  }

  // at boot: the half, if it is home
  if (path === '/api/unlock') {
    if (!(await signedBy(box, `commonty unlock v1\0${box}\0${at}\0${mac || ''}`, sig))) return json({ error: 'not signed by that box' }, 403);
    if (!(await fresh(env, box, at))) return json({ error: 'stale' }, 403);
    if (await env.PINNED.get(`stolen:${box}`)) return json({ error: 'this box is marked stolen' }, 403);
    const share = await env.PINNED.get(`share:${box}`);
    if (!share) return json({ error: 'no half kept for this box' }, 404);
    const home = await getJson(env, `home:${box}`);
    const ok = await getJson(env, `approved:${box}`);
    const approved = ok && ok.until > Date.now() / 1000 && ok.ip === here.ip && ok.mac === here.mac;
    const athome = home && ((here.ip && here.ip === home.ip) || (here.mac && here.mac === home.mac));
    if (athome || approved) {
      await env.PINNED.put(`home:${box}`, JSON.stringify({ ...here, at: Number(at) }));
      await env.PINNED.delete(`pending:${box}`);
      await env.PINNED.delete(`approved:${box}`);
      return json({ share });
    }
    await env.PINNED.put(`pending:${box}`, JSON.stringify({ ...here, at: Number(at) }));
    return json({ waiting: 'a member has to approve this box from its new place' }, 202);
  }
  return json({ error: 'not here' }, 404);
}

/// What a signed-in member sees and does: every box, whether it is waiting
/// to be let in, and the stolen switch. Changes want the passkey just now.
export async function memberApi(req, env, path, body, json, fresh15) {
  if (req.method === 'GET' && path === '/api/boxes') {
    const out = [];
    for (const box of Object.keys(BOXES).sort()) {
      const home = await getJson(env, `home:${box}`);
      const pending = await getJson(env, `pending:${box}`);
      out.push({
        box,
        kept: !!(await env.PINNED.get(`share:${box}`)),
        lastHome: home?.at || null,
        waiting: pending?.at || null,
        stolen: !!(await env.PINNED.get(`stolen:${box}`)),
      });
    }
    return json(out);
  }
  if (req.method !== 'POST') return json({ error: 'not here' }, 404);
  if (!fresh15) return json({ error: 'confirm it is you first' }, 401);
  const { box } = body;
  if (!BOXES[box]) return json({ error: 'no such box' }, 404);
  if (path === '/api/boxes/approve') {
    const p = await getJson(env, `pending:${box}`);
    if (!p) return json({ error: 'that box is not waiting' }, 409);
    await env.PINNED.put(`approved:${box}`, JSON.stringify({ ip: p.ip, mac: p.mac, until: Math.floor(Date.now() / 1000) + APPROVAL }));
    return json({ ok: true });
  }
  if (path === '/api/boxes/stolen') {
    if (body.stolen) {
      await env.PINNED.put(`stolen:${box}`, '1');
      await env.PINNED.delete(`approved:${box}`);
    } else {
      await env.PINNED.delete(`stolen:${box}`);
    }
    return json({ ok: true });
  }
  return json({ error: 'not here' }, 404);
}
