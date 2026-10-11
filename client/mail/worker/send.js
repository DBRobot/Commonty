// Mail from a box (nix/modules/mail.nix, dd-sendmail): handed here over
// HTTPS, signed by the box's TPM key as unlock requests are, and sent on
// through Cloudflare's send_email binding (SEND). It goes only to a member's
// own <name>@<domain>, as the address their rule forwards to, or to
// "alerts", the owner's. Nothing of the body is logged.
//
// KV (PINNED): nonce:<box>:<nonce> for ten minutes; sent:box:<box>:<hour>
// and sent:to:<box>:<rcpt>:<hour>, counts for the hour.

import { EmailMessage } from 'cloudflare:email';
import { BOXES, OWNER } from './fleet.js';
import { signedBy } from './unlock.js';

const SKEW = 300;
const MAX = 256 * 1024;
const NONCE = /^[A-Za-z0-9_-]{16,64}$/;
const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;
// what a sender may call itself; anything else is "Commonty"
const DISPLAY = /^[A-Za-z0-9 .,'()&-]{1,64}$/;
const TYPE = /^(text\/(plain|html)|multipart\/(alternative|mixed|related))(\s*;\s*[A-Za-z][A-Za-z0-9-]*=("[^"\\]{0,200}"|[A-Za-z0-9._+=/-]{1,200}))*\s*;?\s*$/i;
const ENCODING = /^(7bit|8bit|quoted-printable|base64)$/i;
const HOUR = 3600;

const enc = new TextEncoder();
const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');

// the headers kept from what the box sent: Subject, the From's display name,
// and how the body is written. Everything else - To, Cc, Bcc, Reply-To,
// Received, Sender - is the box's to want, not to have.
function split(bytes) {
  let cut = -1;
  let skip = 0;
  for (let i = 0; i < bytes.length - 1; i++) {
    if (bytes[i] === 10 && bytes[i + 1] === 10) { cut = i; skip = 2; break; }
    if (bytes[i] === 10 && bytes[i + 1] === 13 && bytes[i + 2] === 10) { cut = i; skip = 3; break; }
  }
  const headText = cut < 0 ? '' : new TextDecoder().decode(bytes.subarray(0, cut));
  const lines = headText.replace(/\r/g, '').split('\n');
  const fields = {};
  let last = null;
  for (const line of lines) {
    if (/^[ \t]/.test(line) && last) { fields[last] += ' ' + line.trim(); continue; }
    const m = /^([!-9;-~]+):\s?(.*)$/.exec(line);
    // not a header block at all: the whole of it is the body
    if (!m) return { fields: {}, body: bytes };
    last = m[1].toLowerCase();
    if (!(last in fields)) fields[last] = m[2];
  }
  return { fields, body: cut < 0 ? bytes : bytes.subarray(cut + skip) };
}

// control characters (CR and LF among them) as spaces: no header is made
// from another's value
const clean = (s) => [...String(s || '')].map((c) => (c < ' ' || c === '\x7f' ? ' ' : c)).join('').trim();

function encodeWord(s) {
  if (/^[\x20-\x7e]*$/.test(s)) return s;
  return `=?UTF-8?B?${btoa(String.fromCharCode(...enc.encode(s)))}?=`;
}

function displayName(from) {
  const m = /^\s*"?([^"<]*?)"?\s*<[^>]*>\s*$/.exec(clean(from));
  const n = m ? m[1].trim() : '';
  return DISPLAY.test(n) ? n : 'Commonty';
}

// lone LFs as CRLF, as mail wants
function crlf(bytes) {
  const out = [];
  for (let i = 0; i < bytes.length; i++) {
    if (bytes[i] === 10 && (i === 0 || bytes[i - 1] !== 13)) out.push(13);
    out.push(bytes[i]);
  }
  return new Uint8Array(out);
}

async function bump(env, key, by = 1) {
  const n = Number((await env.PINNED.get(key)) || 0) + by;
  await env.PINNED.put(key, String(n), { expirationTtl: 2 * HOUR });
}

export async function sendApi(req, env, json, h) {
  if (req.method !== 'POST') return json({ error: 'POST' }, 405);
  const box = req.headers.get('x-dd-box') || '';
  const at = req.headers.get('x-dd-at') || '';
  const nonce = req.headers.get('x-dd-nonce') || '';
  const to = req.headers.get('x-dd-to') || '';
  const sig = req.headers.get('x-dd-sig') || '';
  // read before anything is decided, as worker.js does for /api/: an unread
  // body can break the next request on the same connection
  const bytes = new Uint8Array(await req.arrayBuffer().catch(() => new ArrayBuffer(0)));
  if (bytes.length > MAX) return json({ error: 'too big' }, 413);
  if (!BOXES[box]) return json({ error: 'no such box' }, 403);

  const digest = hex(await crypto.subtle.digest('SHA-256', bytes));
  if (!(await signedBy(box, `commonty mail send v1\0${box}\0${at}\0${nonce}\0${to}\0${digest}`, sig))) {
    return json({ error: 'not signed by that box' }, 403);
  }
  const t = Number(at);
  if (!Number.isInteger(t) || Math.abs(Math.floor(Date.now() / 1000) - t) > SKEW) return json({ error: 'stale' }, 403);
  if (!NONCE.test(nonce)) return json({ error: 'stale' }, 403);
  const seen = `nonce:${box}:${nonce}`;
  if (await env.PINNED.get(seen)) return json({ error: 'stale' }, 403);
  await env.PINNED.put(seen, '1', { expirationTtl: 2 * SKEW });
  if (await env.PINNED.get(`stolen:${box}`)) return json({ error: 'this box is marked stolen' }, 403);

  const names = [...new Set(to.split(',').map((s) => s.trim().toLowerCase()).filter(Boolean))];
  if (names.length === 0 || names.length > 10) return json({ error: 'one to ten recipients' }, 400);
  // each recipient by name: "alerts", or a member's own <name>@<domain>
  const wanted = [];
  for (const r of names) {
    if (r === 'alerts') { wanted.push({ r, name: OWNER, from: 'alerts' }); continue; }
    const at2 = r.lastIndexOf('@');
    const local = r.slice(0, at2);
    if (at2 < 0 || r.slice(at2 + 1) !== env.DOMAIN.toLowerCase() || !NAME.test(local)) return json({ error: `not a member's address: ${r}` }, 403);
    wanted.push({ r, name: local, from: 'noreply' });
  }

  const hour = Math.floor(Date.now() / 1000 / HOUR);
  const perBox = Number(env.SEND_PER_BOX || 100);
  const perRcpt = Number(env.SEND_PER_RCPT || 20);
  const boxKey = `sent:box:${box}:${hour}`;
  if (Number((await env.PINNED.get(boxKey)) || 0) + wanted.length > perBox) return json({ error: 'this box has sent its share this hour' }, 429);
  for (const w of wanted) {
    w.key = `sent:to:${box}:${w.r}:${hour}`;
    if (Number((await env.PINNED.get(w.key)) || 0) >= perRcpt) return json({ error: `enough to ${w.r} this hour` }, 429);
  }

  // a member: on the signed list, and their rule forwarding to an address
  // Cloudflare has confirmed. Every recipient is checked before any is sent.
  let l;
  try { l = await h.lists(env); } catch { return json({ error: 'Cloudflare did not answer' }, 502); }
  for (const w of wanted) {
    if (!w.name || !NAME.test(w.name)) return json({ error: `not a member's address: ${w.r}` }, 403);
    try { await h.entry(env, w.name); } catch { return json({ error: `not a member's address: ${w.r}` }, 403); }
    const s = h.stateIn(l, env, w.name);
    if (!s.rule || !s.email || !s.confirmed) return json({ error: `no confirmed address for ${w.r}` }, 403);
    w.dest = s.email;
  }

  const { fields, body } = split(bytes);
  const subject = encodeWord(clean(fields.subject).slice(0, 200) || '(no subject)');
  const type = clean(fields['content-type']);
  const cte = clean(fields['content-transfer-encoding']);
  const name = displayName(fields.from);
  const text = crlf(body);
  for (const w of wanted) {
    const from = `${w.from}@${env.DOMAIN}`;
    const head = [
      `From: "${name}" <${from}>`,
      `To: <${w.r === 'alerts' ? from : w.r}>`,
      `Subject: ${subject}`,
      `Date: ${new Date().toUTCString()}`,
      `Message-ID: <${crypto.randomUUID()}@${env.DOMAIN}>`,
      'MIME-Version: 1.0',
      `Content-Type: ${TYPE.test(type) ? type : 'text/plain; charset=utf-8'}`,
      `Content-Transfer-Encoding: ${ENCODING.test(cte) ? cte.toLowerCase() : '8bit'}`,
      'Auto-Submitted: auto-generated',
      '',
      '',
    ].join('\r\n');
    const raw = new Blob([head, text]).stream();
    try {
      await env.SEND.send(new EmailMessage(from, w.dest, raw));
    } catch (err) {
      console.log(`send: ${box}: to ${w.r} refused by Cloudflare: ${err.code || ''}`);
      return json({ error: 'Cloudflare did not take it' }, 502);
    }
    await bump(env, boxKey);
    await bump(env, w.key);
  }
  console.log(`send: ${box}: ${wanted.length} sent`);
  return json({ sent: wanted.length });
}
