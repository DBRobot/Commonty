// mail.<domain>: a member's email address, kept at Cloudflare and handled
// here, where no box can reach it. Mail to <name>@<domain> is forwarded by
// an Email Routing rule; this Worker holds the only token that can read or
// change those rules, shows a member their address, and changes it - after
// their passkey, checked against their own signed entry (dd_mail.wasm, the
// boxes' code) and the member list the release was signed with (fleet.js).
//
// Bindings: CF_TOKEN (secret: Email Routing, this zone), SESSION_KEY
// (secret), PINNED (KV: the newest entry accepted per name), DOMAIN. A
// local test also sets CF_API, DIRECTORY and ORIGIN; deployed, they are unset.

import { initSync, admit, login } from './dd_mail.js';
import wasm from './dd_mail_bg.wasm';
import { MEMBERS, RELEASE } from './fleet.js';
import { boxApi, memberApi } from './unlock.js';
import boxesHtml from './boxes.html';
import boxesJs from './boxes.js';
import changeHtml from './change.html';
import rowHtml from './row.html';
import changeJs from './change.js';
import rowJs from './row.js';
import passkeyJs from './passkey.js';
import mailCss from './mail.css';
import homeCss from './home.css';
import barCss from './bar.css';
import sans from './public-sans.woff2';
import mono400 from './plex-mono-400.woff2';
import mono500 from './plex-mono-500.woff2';

initSync({ module: wasm });

const API = 'https://api.cloudflare.com/client/v4';
const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;
const EMAIL = /^[^@\s]{1,64}@[A-Za-z0-9.-]{1,253}\.[A-Za-z]{2,}$/;
// shown your address for this long after the passkey, in this browser
const SESSION = 30 * 86400;
// changing it wants the passkey this recently
const FRESH = 15 * 60;
const CHALLENGE = 5 * 60;

const STATIC = {
  '/_dd/static/change.js': [changeJs, 'text/javascript; charset=utf-8'],
  '/_dd/static/row.js': [rowJs, 'text/javascript; charset=utf-8'],
  '/_dd/static/boxes.js': [boxesJs, 'text/javascript; charset=utf-8'],
  '/_dd/static/passkey.js': [passkeyJs, 'text/javascript; charset=utf-8'],
  '/_dd/static/mail.css': [mailCss, 'text/css; charset=utf-8'],
  '/_dd/static/home.css': [homeCss, 'text/css; charset=utf-8'],
  '/_dd/static/bar.css': [barCss, 'text/css; charset=utf-8'],
  '/_dd/static/public-sans.woff2': [sans, 'font/woff2'],
  '/_dd/static/plex-mono-400.woff2': [mono400, 'font/woff2'],
  '/_dd/static/plex-mono-500.woff2': [mono500, 'font/woff2'],
};

const policy = (frames) => [
  "default-src 'self'",
  "script-src 'self' 'wasm-unsafe-eval'",
  "style-src 'self'",
  "img-src 'self' data:",
  "font-src 'self'",
  "connect-src 'self'",
  "base-uri 'none'",
  "form-action 'self'",
  `frame-ancestors ${frames}`,
].join('; ');

function page(html, env, frames = "'none'") {
  return new Response(html.replaceAll('{{domain}}', env.DOMAIN), {
    headers: {
      'content-type': 'text/html; charset=utf-8',
      'content-security-policy': policy(frames),
      'cache-control': 'no-store',
      'referrer-policy': 'no-referrer',
      'x-content-type-options': 'nosniff',
    },
  });
}

const json = (v, status = 200, headers = {}) =>
  new Response(JSON.stringify(v), { status, headers: { 'content-type': 'application/json', 'cache-control': 'no-store', ...headers } });

const b64u = (bytes) => btoa(String.fromCharCode(...new Uint8Array(bytes))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const unb64u = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));
const now = () => Math.floor(Date.now() / 1000);

// signed little tokens: a challenge handed out, a session after a passkey
async function key(env) {
  return crypto.subtle.importKey('raw', new TextEncoder().encode(env.SESSION_KEY), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign', 'verify']);
}
async function seal(env, v) {
  const body = b64u(new TextEncoder().encode(JSON.stringify(v)));
  const mac = await crypto.subtle.sign('HMAC', await key(env), new TextEncoder().encode(body));
  return `${body}.${b64u(mac)}`;
}
async function unseal(env, token, kind) {
  const [body, mac] = String(token || '').split('.');
  if (!body || !mac) return null;
  let ok = false;
  try {
    ok = await crypto.subtle.verify('HMAC', await key(env), unb64u(mac), new TextEncoder().encode(body));
  } catch { return null; }
  if (!ok) return null;
  const v = JSON.parse(new TextDecoder().decode(unb64u(body)));
  return v.k === kind && v.e > now() ? v : null;
}

function cookie(req, name) {
  for (const part of (req.headers.get('cookie') || '').split(';')) {
    const [k, ...v] = part.trim().split('=');
    if (k === name) return v.join('=');
  }
  return '';
}
// a session lasts while the passkey that opened it is still the member's,
// and until they sign out everywhere else (ENDED: the time they last did)
async function session(req, env) {
  const s = await unseal(env, cookie(req, 'mail_session'), 'session');
  if (!s || !s.i || !s.t) return null;
  if (s.t < Number((await env.PINNED.get(`ended:${s.n}`)) || 0)) return null;
  try {
    const e = JSON.parse(await entry(env, s.n));
    return e.entry.passkeys.some((p) => p.id === s.i) ? s : null;
  } catch {
    return null;
  }
}
const opened = async (env, v) => {
  const s = await seal(env, { k: 'session', ...v, e: now() + SESSION });
  return { 'set-cookie': `mail_session=${s}; Path=/; Max-Age=${SESSION}; HttpOnly; Secure; SameSite=Strict` };
};

// The member's entry, as the directory has it, held to the newest this
// Worker has accepted. Throws for a name that is not a member.
async function entry(env, name) {
  const r = await fetch(`${env.DIRECTORY || `https://home.${env.DOMAIN}/_dd/directory/`}${encodeURIComponent(name)}`);
  if (!r.ok) throw new Error('no such member');
  const pinned = await env.PINNED.get(`entry:${name}`);
  const take = admit(name, await r.text(), pinned ?? undefined, MEMBERS, RELEASE);
  if (take !== pinned) await env.PINNED.put(`entry:${name}`, take);
  return take;
}

// Cloudflare: the rule for <name>@<domain>, and the destinations
async function cf(env, method, path, body) {
  const r = await fetch((env.CF_API || API) + path, {
    method,
    headers: { authorization: `Bearer ${env.CF_TOKEN}`, 'content-type': 'application/json' },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  return r.json();
}
let zoneCache = null;
async function zone(env) {
  if (!zoneCache) {
    const z = (await cf(env, 'GET', `/zones?name=${env.DOMAIN}`)).result[0];
    zoneCache = { id: z.id, account: z.account.id };
  }
  return zoneCache;
}
// every page of a listing; a page Cloudflare refused stops it, rather than
// passing for "there are none" and a second rule being made
async function pages(env, path) {
  const out = [];
  for (let page = 1; ; page++) {
    const d = await cf(env, 'GET', `${path}${path.includes('?') ? '&' : '?'}per_page=50&page=${page}`);
    if (!d.success) throw new Error('Cloudflare did not answer; try again in a minute');
    const got = d.result || [];
    out.push(...got);
    if (got.length < 50) return out;
  }
}
// the zone's rules and the account's addresses, both at once
async function lists(env) {
  const z = await zone(env);
  const [rules, addresses] = await Promise.all([
    pages(env, `/zones/${z.id}/email/routing/rules`),
    pages(env, `/accounts/${z.account}/email/routing/addresses`),
  ]);
  return { rules, addresses };
}
const target = (rule) => (rule?.actions || []).find((a) => a.type === 'forward')?.value?.[0] || null;
const same = (a, b) => (a || '').toLowerCase() === (b || '').toLowerCase();
function stateIn(l, env, name) {
  const address = `${name}@${env.DOMAIN}`;
  const rule = l.rules.find((r) => (r.matchers || []).some((m) => m.field === 'to' && m.value === address)) || null;
  const to = target(rule);
  const d = to ? l.addresses.find((a) => same(a.email, to)) : null;
  return { rule, email: to, confirmed: !!d?.verified, destination: d || null };
}
async function state(env, name) {
  return stateIn(await lists(env), env, name);
}

// What anyone may ask - has this member an address, is it confirmed - is
// answered from lists at most a minute old, one fetch of each for every
// name: asking over and over cannot spend the token's Cloudflare allowance.
// Your own address, behind your passkey, is always read fresh.
const SHARED = 60 * 1000;
let shared = null;
async function sharedState(env, name) {
  if (!shared || Date.now() - shared.at > SHARED) shared = { at: Date.now(), ...(await lists(env)) };
  const s = stateIn(shared, env, name);
  return { forwarding: !!s.rule, confirmed: s.confirmed };
}

async function setEmail(env, name, email) {
  const z = await zone(env);
  const l = await lists(env);
  const before = stateIn(l, env, name);
  // Adding an address Cloudflare does not know sends its confirmation; one
  // the account already holds confirmed gets none, and none is waited for
  const had = l.addresses.find((a) => same(a.email, email));
  if (!had) await cf(env, 'POST', `/accounts/${z.account}/email/routing/addresses`, { email });
  const body = {
    name: `Commonty member ${name}`,
    enabled: true,
    matchers: [{ type: 'literal', field: 'to', value: `${name}@${env.DOMAIN}` }],
    actions: [{ type: 'forward', value: [email] }],
  };
  const d = before.rule
    ? await cf(env, 'PUT', `/zones/${z.id}/email/routing/rules/${before.rule.id}`, body)
    : await cf(env, 'POST', `/zones/${z.id}/email/routing/rules`, body);
  if (!d.success) throw new Error('that did not work; try again in a minute');
  // the old address is no one's business once no rule sends there
  if (before.destination && !same(before.email, email)) {
    const used = l.rules.some((r) => r.id !== before.rule.id && (r.actions || []).some((a) => (a.value || []).some((v) => same(v, before.email))));
    const old = before.destination;
    if (!used) await cf(env, 'DELETE', `/accounts/${z.account}/email/routing/addresses/${old.tag || old.id}`);
  }
  return { confirm: !had?.verified };
}

async function resend(env, name) {
  const z = await zone(env);
  const s = await state(env, name);
  if (!s.email) return { ok: false };
  if (s.confirmed) return { ok: true, confirmed: true };
  const d = s.destination;
  if (d) await cf(env, 'DELETE', `/accounts/${z.account}/email/routing/addresses/${d.tag || d.id}`);
  const again = await cf(env, 'POST', `/accounts/${z.account}/email/routing/addresses`, { email: s.email });
  return { ok: !!again.success, confirmed: false };
}

// where a flow may send the person back to: a page of this fleet
function back(env, raw) {
  try {
    const u = new URL(raw);
    if (u.protocol === 'https:' && (u.hostname === env.DOMAIN || u.hostname.endsWith(`.${env.DOMAIN}`))) return u.href;
  } catch { /* fall through */ }
  return `https://home.${env.DOMAIN}/_dd/settings`;
}

const attr = (s) => s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');

function parse(text) {
  try { return JSON.parse(text); } catch { return {}; }
}

async function api(req, env, path, sent) {
  const body = () => parse(sent);
  if (req.method === 'POST' && path === '/api/challenge') {
    const { name } = body();
    if (!NAME.test(name || '')) return json({ error: 'no such member' }, 400);
    let e;
    try { e = JSON.parse(await entry(env, name)); } catch (err) { return json({ error: String(err.message || err) }, 403); }
    const challenge = b64u(crypto.getRandomValues(new Uint8Array(32)));
    return json({
      challenge,
      token: await seal(env, { k: 'challenge', n: name, c: challenge, e: now() + CHALLENGE }),
      rpId: env.DOMAIN,
      allow: e.entry.passkeys.map((p) => p.id),
    });
  }
  if (req.method === 'POST' && path === '/api/login') {
    const { token, id, assertion } = body();
    const c = await unseal(env, token, 'challenge');
    if (!c) return json({ error: 'that took too long; try again' }, 403);
    try {
      // made on this Worker's own page, not on any of the fleet's
      login(await entry(env, c.n), id, JSON.stringify(assertion), c.c, `https://mail.${env.DOMAIN}`);
    } catch (err) {
      return json({ error: 'your passkey was not accepted' }, 403);
    }
    return json({ ok: true }, 200, await opened(env, { n: c.n, i: id, p: now(), t: Date.now() }));
  }
  if (req.method === 'GET' && path === '/api/state') {
    const name = new URL(req.url).searchParams.get('name') || '';
    if (!NAME.test(name)) return json({ error: 'no such member' }, 400);
    return json(await sharedState(env, name));
  }
  const me = await session(req, env);
  if (!me) return json({ error: 'confirm it is you first' }, 401);
  if (req.method === 'GET' && path === '/api/me') {
    const s = await state(env, me.n);
    return json({ name: me.n, email: s.email, confirmed: s.confirmed, fresh: now() - (me.p || 0) < FRESH });
  }
  // the boxes' disks: which are waiting to be let in, and the stolen switch
  if (path === '/api/boxes' || path.startsWith('/api/boxes/')) {
    return memberApi(req, env, path, body(), json, now() - (me.p || 0) < FRESH);
  }
  // Settings' "sign out everywhere else": every other session of this
  // member's ends; this one is opened again, after the line
  if (req.method === 'POST' && path === '/api/end-others') {
    const t = Date.now();
    await env.PINNED.put(`ended:${me.n}`, String(t));
    return json({ ok: true }, 200, await opened(env, { n: me.n, i: me.i, p: me.p, t }));
  }
  // a change wants the passkey just now, not a month ago
  if (now() - (me.p || 0) >= FRESH) return json({ error: 'confirm it is you first' }, 401);
  if (req.method === 'POST' && path === '/api/email') {
    const { email } = body();
    if (!EMAIL.test((email || '').trim())) return json({ error: 'that is not an email address' }, 400);
    shared = null;
    try {
      return json(await setEmail(env, me.n, email.trim()));
    } catch (err) {
      return json({ error: String(err.message || err) }, 502);
    }
  }
  if (req.method === 'POST' && path === '/api/resend') return json(await resend(env, me.n));
  if (req.method === 'POST' && path === '/api/signout') {
    return json({ ok: true }, 200, { 'set-cookie': 'mail_session=; Path=/; Max-Age=0; HttpOnly; Secure; SameSite=Strict' });
  }
  return json({ error: 'not here' }, 404);
}

export default {
  async fetch(req, env) {
    const url = new URL(req.url);
    const path = url.pathname;
    if (STATIC[path]) {
      const [body, type] = STATIC[path];
      return new Response(body, { headers: { 'content-type': type, 'cache-control': 'public, max-age=300', 'x-content-type-options': 'nosniff' } });
    }
    if (path.startsWith('/api/')) {
      // the pages here are the only callers
      const origin = req.headers.get('origin');
      // what was sent, read before anything is decided: a request answered
      // with its body unread can be dropped by the runtime, a 503 to the caller
      const sent = req.method === 'GET' ? '' : await req.text().catch(() => '');
      // a box at boot: signed by its TPM, from no page at all
      if (path === '/api/unlock' || path.startsWith('/api/unlock/')) return boxApi(req, env, path, parse(sent), json);
      if (req.method !== 'GET' && origin !== (env.ORIGIN || `https://mail.${env.DOMAIN}`)) {
        return json({ error: 'not from here' }, 403);
      }
      return api(req, env, path, sent);
    }
    // the change, from Settings; and an email for a new member, from sign-up
    if (path === '/change' || path === '/start') {
      const html = changeHtml
        .replace('{{mode}}', path.slice(1))
        .replace('{{back}}', attr(back(env, url.searchParams.get('back') || '')));
      return page(html, env);
    }
    // the email line on Settings, in a frame on the fleet's own pages
    if (path === '/row') return page(rowHtml, env, `https://*.${env.DOMAIN}`);
    // the boxes: approve one in a new place, or mark one stolen
    if (path === '/boxes') return page(boxesHtml, env);
    return Response.redirect(`https://home.${env.DOMAIN}/_dd/settings`, 302);
  },
};
