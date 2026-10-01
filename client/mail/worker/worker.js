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
const SESSION = 15 * 60;
const CHALLENGE = 5 * 60;

const STATIC = {
  '/_dd/static/change.js': [changeJs, 'text/javascript; charset=utf-8'],
  '/_dd/static/row.js': [rowJs, 'text/javascript; charset=utf-8'],
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
async function session(req, env) {
  return unseal(env, cookie(req, 'mail_session'), 'session');
}

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
async function pages(env, path) {
  const out = [];
  for (let page = 1; ; page++) {
    const d = await cf(env, 'GET', `${path}${path.includes('?') ? '&' : '?'}per_page=50&page=${page}`);
    const got = d.result || [];
    out.push(...got);
    if (got.length < 50) return out;
  }
}
async function ruleFor(env, name) {
  const z = await zone(env);
  const address = `${name}@${env.DOMAIN}`;
  const rules = await pages(env, `/zones/${z.id}/email/routing/rules`);
  return rules.find((r) => (r.matchers || []).some((m) => m.field === 'to' && m.value === address)) || null;
}
const target = (rule) => (rule?.actions || []).find((a) => a.type === 'forward')?.value?.[0] || null;
async function destination(env, email) {
  const z = await zone(env);
  const all = await pages(env, `/accounts/${z.account}/email/routing/addresses`);
  return all.find((a) => (a.email || '').toLowerCase() === email.toLowerCase()) || null;
}
async function state(env, name) {
  const rule = await ruleFor(env, name);
  const to = target(rule);
  const d = to ? await destination(env, to) : null;
  return { rule, email: to, confirmed: !!d?.verified };
}

async function setEmail(env, name, email) {
  const z = await zone(env);
  const before = await state(env, name);
  // Adding an address Cloudflare does not know sends its confirmation; one
  // the account already holds confirmed gets none, and none is waited for
  await cf(env, 'POST', `/accounts/${z.account}/email/routing/addresses`, { email });
  const known = await destination(env, email);
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
  if (before.email && before.email.toLowerCase() !== email.toLowerCase()) {
    const rules = await pages(env, `/zones/${z.id}/email/routing/rules`);
    const used = rules.some((r) => (r.actions || []).some((a) => (a.value || []).some((v) => v.toLowerCase() === before.email.toLowerCase())));
    const old = used ? null : await destination(env, before.email);
    if (old) await cf(env, 'DELETE', `/accounts/${z.account}/email/routing/addresses/${old.tag || old.id}`);
  }
  return { confirm: !known?.verified };
}

async function resend(env, name) {
  const z = await zone(env);
  const s = await state(env, name);
  if (!s.email) return { ok: false };
  if (s.confirmed) return { ok: true, confirmed: true };
  const d = await destination(env, s.email);
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
      login(await entry(env, c.n), id, JSON.stringify(assertion), c.c);
    } catch (err) {
      return json({ error: 'your passkey was not accepted' }, 403);
    }
    const s = await seal(env, { k: 'session', n: c.n, e: now() + SESSION });
    return json({ ok: true }, 200, { 'set-cookie': `mail_session=${s}; Path=/; Max-Age=${SESSION}; HttpOnly; Secure; SameSite=Strict` });
  }
  if (req.method === 'GET' && path === '/api/state') {
    const name = new URL(req.url).searchParams.get('name') || '';
    if (!NAME.test(name)) return json({ error: 'no such member' }, 400);
    const s = await state(env, name);
    return json({ forwarding: !!s.rule, confirmed: s.confirmed });
  }
  const me = await session(req, env);
  if (!me) return json({ error: 'confirm it is you first' }, 401);
  if (req.method === 'GET' && path === '/api/me') {
    const s = await state(env, me.n);
    return json({ name: me.n, email: s.email, confirmed: s.confirmed });
  }
  if (req.method === 'POST' && path === '/api/email') {
    const { email } = body();
    if (!EMAIL.test((email || '').trim())) return json({ error: 'that is not an email address' }, 400);
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
    return Response.redirect(`https://home.${env.DOMAIN}/_dd/settings`, 302);
  },
};
