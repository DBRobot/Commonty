// Mail a member sends goes out from their own app, through their box, to
// the other side's server: the box carries it encrypted and never reads it.
// What it needs from here is the domain's DKIM signature, so the other side
// can tell it is ours. The app sends the headers to be signed and the hash
// of the body, never the body; the From must be the member's own address or
// one of their aliases. The Subject is not signed, so it is not sent here.
//
// Bindings: DKIM_KEY (secret: PKCS#8 RSA, `dd mail dkim`), DKIM_SELECTOR.

import { device } from './inbox.js';
import { aliasOwned } from './aliases.js';
import { addresses } from './mail-mime.js';

const SIGNED = ['from', 'to', 'cc', 'reply-to', 'date', 'message-id', 'in-reply-to', 'references', 'mime-version', 'content-type', 'content-transfer-encoding'];
const BH = /^[A-Za-z0-9+/]{43}=$/;
const PER_DAY = 200;
const unb64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
const b64 = (u) => btoa(String.fromCharCode(...new Uint8Array(u)));

// RFC 6376 relaxed header canonicalization
export const relaxed = (name, value) => `${name.toLowerCase().trim()}:${String(value).replace(/\r?\n[ \t]/g, ' ').replace(/[ \t]+/g, ' ').trim()}\r\n`;

let keyCache = null;
async function key(env) {
  if (!keyCache || keyCache.src !== env.DKIM_KEY) {
    keyCache = { src: env.DKIM_KEY, k: await crypto.subtle.importKey('pkcs8', unb64(env.DKIM_KEY), { name: 'RSASSA-PKCS1-v1_5', hash: 'SHA-256' }, false, ['sign']) };
  }
  return keyCache.k;
}

// the DKIM-Signature for these headers (in order, as the message has them)
export async function sign(env, headers, bh, t = Math.floor(Date.now() / 1000)) {
  // each signed once more than the message has it (RFC 6376 5.4): a header
  // added after, a second From above the first, breaks the signature
  // (never the Subject: it is not sent here, and the message has one)
  const present = headers.map(([n]) => n.toLowerCase());
  const names = [...present, ...['from', 'to', 'cc', 'reply-to', 'date', 'message-id', 'in-reply-to', 'references', 'sender']];
  const value = `v=1; a=rsa-sha256; c=relaxed/relaxed; d=${env.DOMAIN}; s=${env.DKIM_SELECTOR || 'cm1'}; t=${t}; h=${names.join(':')}; bh=${bh}; b=`;
  const data = headers.map(([n, v]) => relaxed(n, v)).join('') + relaxed('DKIM-Signature', value).replace(/\r\n$/, '');
  const sig = await crypto.subtle.sign('RSASSA-PKCS1-v1_5', await key(env), new TextEncoder().encode(data));
  return `DKIM-Signature: ${value}${b64(sig)}`;
}

// POST /api/dkim {headers: [[name, value]], bh}, signed by a device of theirs
export async function dkimApi(req, env, path, bytes, json, h) {
  if (req.method !== 'POST') return json({ error: 'POST' }, 405);
  if (!env.DKIM_KEY) return json({ error: 'sending is not set up yet' }, 503);
  const who = await device(req, env, path, bytes, h.entry);
  if (!who) return json({ error: 'sign it with a device of yours' }, 401);
  let b;
  try { b = JSON.parse(new TextDecoder().decode(bytes)); } catch { return json({ error: 'not JSON' }, 400); }
  const headers = Array.isArray(b.headers) ? b.headers : [];
  if (!BH.test(b.bh || '') || headers.length > 20) return json({ error: 'not a message' }, 400);
  for (const x of headers) {
    if (!Array.isArray(x) || x.length !== 2 || typeof x[0] !== 'string' || typeof x[1] !== 'string') return json({ error: 'not a header' }, 400);
    if (!SIGNED.includes(x[0].toLowerCase()) || x[1].length > 4000 || /[\r\n]/.test(x[1])) return json({ error: `not a header to sign: ${x[0]}` }, 400);
  }
  const froms = headers.filter(([n]) => n.toLowerCase() === 'from');
  if (froms.length !== 1) return json({ error: 'one From' }, 400);
  // one plain From: an address, or a name without one in it before it -
  // what a mail program shows as the sender is what was checked here
  const raw = froms[0][1].trim();
  const plain = raw.match(/^(?:"?([^"<>@,;:\\]*)"?\s*)?<([^<>\s@,;:"]+@[^<>\s@,;:"]+)>$/) || raw.match(/^()([^<>\s@,;:"()]+@[^<>\s@,;:"()]+)$/);
  if (!plain || /=\?/.test(raw)) return json({ error: 'a From of an address and, if you like, a plain name' }, 400);
  const from = addresses(raw);
  if (from.length !== 1 || from[0].address !== plain[2].toLowerCase()) return json({ error: 'one From' }, 400);
  const [local, domain] = from[0].address.split('@');
  if (domain !== env.DOMAIN.toLowerCase()) return json({ error: 'only from your address here' }, 403);
  if (local !== who.name && !(await aliasOwned(env, who.name, local))) return json({ error: 'that is not your address' }, 403);
  // their address must still be theirs: a confirmed one, on the list
  const s = h.stateIn(await h.listsShared(env), env, who.name);
  if (!s.rule) return json({ error: 'you have no address here' }, 403);
  const r = await env.HOLD.get(env.HOLD.idFromName('hold')).fetch(`https://hold/count?member=${encodeURIComponent(who.name)}&max=${Number(env.SEND_PER_DAY || PER_DAY)}`, { method: 'POST' });
  if (r.status === 429) return json({ error: 'that is as much as you can send today' }, 429);
  return json({ header: await sign(env, headers, b.bh) });
}
