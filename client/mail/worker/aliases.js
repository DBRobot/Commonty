// A member's own aliases: <words>.<digits>@<domain>, one for each site
// they give an address to. Mail to one reaches the email() handler here
// through the zone's catch-all rule - the members' and the release's
// aliases keep their own rules, which come first - and goes on to the
// member's confirmed address, or bounces if the alias is off or gone. Only
// this Worker knows which aliases are whose; no box is ever told.
//
// KV (PINNED): alias:<local> {m, l, on, t, x, e, by} (member, label, on, made,
// off since, turns off at, made by Passwords); owner:<member>:<local>, one key
// per alias, so no write ever rewrites a list another request is changing;
// seen:<local> {a, p} (the sites that have mailed it and are fine, the new
// ones not yet seen to), apart from the alias so mail coming in never writes
// over a change; pwkey:<member> {h, i, t} and pwtok:<hash> = member, the key
// Passwords makes aliases with, and pwused:<member>, when it last made one,
// apart so a use never writes back a key just taken away;
// pwsite:<member>:<site>, the alias it just made there, for ten minutes; forgot:<member>, when they left, for a day. A deleted
// alias stays as alias:<local> {gone}, with no member, so its address is
// never given out again. Mail goes on only while both alias:<local> and its
// owner key say so: a change racing a delete can write the first back, never
// the second, and a leaver's aliases made before they left stay dead even
// if a listing missed one.

import { OWNER } from './fleet.js';
import { mailKey, holdMail } from './inbox.js';

const MAX = 100;
// the addresses a member is offered in a day, the same ten however often
// they ask: no one presses Another until an address they like comes up,
// and no more than ten a day are made, as every one is kept for good
const OFFERS = 10;
const LABEL = 60;
// how long until one turns itself off, in days: New alias, and the menu
const TTLS = [0, 1, 7, 30];
// the sites one keeps, the fine and the new together
const SITES = 20;
const LOCAL = /^[a-z]{3,10}\.[a-z]{3,10}\.[0-9]{4}$/;
const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;

const FIRST = [
  'alder', 'amber', 'aspen', 'autumn', 'basil', 'birch', 'bramble', 'brook', 'canyon', 'cedar',
  'clover', 'coast', 'copper', 'coral', 'cove', 'dawn', 'delta', 'dune', 'elm', 'ember',
  'fern', 'field', 'fir', 'frost', 'glade', 'granite', 'grove', 'harbor', 'hazel', 'heath',
  'hollow', 'indigo', 'iris', 'ivy', 'juniper', 'lake', 'larch', 'laurel', 'linden', 'lotus',
  'maple', 'marsh', 'meadow', 'mesa', 'mint', 'misty', 'moss', 'oak', 'ocean', 'olive',
  'orchid', 'pebble', 'pine', 'plum', 'poplar', 'prairie', 'quartz', 'rain', 'reed', 'ridge',
  'river', 'rowan', 'sage', 'sand', 'shore', 'silver', 'slate', 'snow', 'spruce', 'stone',
  'summer', 'sun', 'thistle', 'thyme', 'tide', 'timber', 'valley', 'violet', 'walnut', 'willow',
];
const SECOND = [
  'badger', 'beaver', 'bison', 'crane', 'crow', 'deer', 'dove', 'eagle', 'egret', 'falcon',
  'finch', 'fox', 'gecko', 'goose', 'grouse', 'hare', 'hawk', 'heron', 'ibis', 'jay',
  'kestrel', 'kite', 'lark', 'lynx', 'magpie', 'marten', 'mink', 'moose', 'newt', 'otter',
  'owl', 'panda', 'pelican', 'pika', 'plover', 'puffin', 'quail', 'rabbit', 'raven', 'robin',
  'salmon', 'seal', 'shrike', 'skylark', 'sparrow', 'stork', 'swallow', 'swan', 'swift', 'tern',
  'thrush', 'toad', 'trout', 'turtle', 'vole', 'walrus', 'warbler', 'weasel', 'whale', 'wren',
];

// offer n of the member's day: from the Worker's own key, so no one can
// work out another member's, nor ask for more than these
async function offered(env, member, day, n) {
  const k = await crypto.subtle.importKey('raw', new TextEncoder().encode(env.SESSION_KEY), { name: 'HMAC', hash: 'SHA-256' }, false, ['sign']);
  const b = new DataView(await crypto.subtle.sign('HMAC', k, new TextEncoder().encode(`alias-offer\n${member}\n${day}\n${n}`)));
  const digits = String(b.getUint32(4) % 10000).padStart(4, '0');
  return `${FIRST[b.getUint16(0) % FIRST.length]}.${SECOND[b.getUint16(2) % SECOND.length]}.${digits}`;
}
const dayOf = (t) => new Date(t * 1000).toISOString().slice(0, 10);

const read = async (env, key, fallback) => {
  const v = await env.PINNED.get(key);
  if (v === null) return fallback;
  try { return JSON.parse(v); } catch { return fallback; }
};
// every alias of the member's, by their own keys; a listing pages through
async function listOf(env, member) {
  const prefix = `owner:${member}:`;
  const out = [];
  let cursor;
  do {
    const r = await env.PINNED.list({ prefix, cursor });
    out.push(...r.keys.map((k) => k.name.slice(prefix.length)));
    cursor = r.list_complete ? undefined : r.cursor;
  } while (cursor);
  return out;
}

// on, and not past the time it was to turn off
const live = (a, now) => !!a.on && !(a.e && now >= a.e);

// a member's aliases that take mail now, with their labels: what Mail
// offers to send from (inbox.js)
export async function liveAliases(env, member) {
  const now = Math.floor(Date.now() / 1000);
  const forgot = Number((await env.PINNED.get(`forgot:${member}`)) || 0);
  const out = [];
  for (const local of await listOf(env, member)) {
    const a = await read(env, `alias:${local}`, null);
    if (a && a.m === member && !(forgot >= a.t) && live(a, now)) out.push({ address: `${local}@${env.DOMAIN}`, label: a.l || '' });
  }
  return out;
}

// may `member` send as <local>@<domain>: an alias of theirs, on, and not
// forgotten (the mail Worker's DKIM signing, dkim.js)
export async function aliasOwned(env, member, local) {
  if (!LOCAL.test(local || '')) return false;
  const a = await read(env, `alias:${local}`, null);
  if (!a || a.m !== member) return false;
  const [owned, forgot] = await Promise.all([env.PINNED.get(`owner:${member}:${local}`), env.PINNED.get(`forgot:${member}`)]);
  return owned !== null && !(Number(forgot || 0) >= a.t) && live(a, Math.floor(Date.now() / 1000));
}
// a day, in seconds; shorter only in the test (nix/tests/mail-worker.nix)
const dayLen = (env) => {
  const d = Number(env.ALIAS_DAY);
  return Number.isFinite(d) && d > 0 ? d : 86400;
};

function shown(env, local, a, now, seen) {
  const expired = !!(a.on && a.e && now >= a.e);
  return {
    address: `${local}@${env.DOMAIN}`,
    label: a.l || '',
    on: live(a, now),
    made: a.t,
    off: expired ? a.e : a.x || null,
    ends: live(a, now) && a.e ? a.e : null,
    auto: expired,
    by: a.by || null,
    from: seen?.a || [],
    fresh: seen?.p || [],
  };
}

// the site a sender is at: the last two labels of its domain, or three
// under a country's own (example.co.uk); near enough for "who mailed this"
export function siteOf(from) {
  const m = /@([A-Za-z0-9.-]+)>?\s*$/.exec(String(from || '').trim());
  if (!m) return '';
  const d = m[1].toLowerCase().replace(/\.$/, '').split('.').filter(Boolean);
  if (d.length < 2) return '';
  const two = d.length > 2 && d.at(-1).length === 2 && /^(co|com|org|net|ac|gov|edu|ne|or|go)$/.test(d.at(-2));
  return d.slice(two ? -3 : -2).join('.');
}

const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');
const sha = async (s) => hex(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(s)));

// the first of today's offers from n on that no one holds
async function nextFree(env, h, member, n) {
  const today = dayOf(h.now());
  for (; n < OFFERS; n++) {
    const local = await offered(env, member, today, n);
    if (await free(env, h, local)) return { local, n, day: today };
  }
  return null;
}

// an alias made: from an offer, under the cap, and free
async function make(env, h, member, local, label, extra) {
  if ((await listOf(env, member)).length >= MAX) return { status: 409, error: `you have ${MAX} aliases, the most there can be; delete one to make another` };
  if (!(await free(env, h, local))) return { status: 409, error: 'that address was just taken; press Another' };
  const a = { m: member, l: label, on: true, t: h.now(), ...extra };
  await env.PINNED.put(`alias:${local}`, JSON.stringify(a));
  await env.PINNED.put(`owner:${member}:${local}`, '');
  // made as they were leaving: gone with the rest (forgetAliases listed
  // before this was there)
  const forgot = Number((await env.PINNED.get(`forgot:${member}`)) || 0);
  if (forgot && forgot >= a.t - 60) {
    await env.PINNED.put(`alias:${local}`, JSON.stringify({ gone: h.now() }));
    await env.PINNED.delete(`owner:${member}:${local}`);
    return { status: 403, error: 'no such member' };
  }
  return { alias: shown(env, local, a, h.now()) };
}

const ttlOf = (v) => (TTLS.includes(v) ? v : null);

// a local part no one holds: no alias of anyone's, made or deleted, no
// member's own address, no alias of the release's
async function free(env, h, local) {
  if (!LOCAL.test(local)) return false;
  if (Object.prototype.hasOwnProperty.call(h.ALIASES || {}, local)) return false;
  if ((await env.PINNED.get(`alias:${local}`)) !== null) return false;
  return !h.memberName(await h.listsShared(env), local);
}

// Settings' Email tab (aliases.html), behind the member's session
export async function aliasApi(req, env, path, body, me, json, h) {
  const member = me.n;
  if (req.method === 'GET' && path === '/api/aliases') {
    const locals = await listOf(env, member);
    const all = await Promise.all(locals.map(async (l) => [l, await read(env, `alias:${l}`, null), await read(env, `seen:${l}`, null)]));
    const aliases = all.filter(([, a]) => a && a.m === member).map(([l, a, seen]) => shown(env, l, a, h.now(), seen));
    aliases.sort((x, y) => y.made - x.made);
    // a key its passkey or a sign-out everywhere has ended shows as not connected
    const [key, used] = await Promise.all([read(env, `pwkey:${member}`, null), env.PINNED.get(`pwused:${member}`)]);
    const on = key?.i && (await h.holds(member, key.i, key.t));
    return json({ aliases, max: MAX, passwords: on ? { made: Math.floor(key.t / 1000), used: Number(used) || null } : null });
  }
  // the key Passwords makes aliases with: shown once, kept only as its hash.
  // It makes aliases and nothing else, and wants the passkey, as it is one.
  if (req.method === 'POST' && path === '/api/aliases/passwords') {
    if (!(await h.confirmed(body))) return json({ error: 'your passkey was not accepted for this' }, 403);
    const old = await read(env, `pwkey:${member}`, null);
    if (old) await env.PINNED.delete(`pwtok:${old.h}`);
    const key = `cmty_${btoa(String.fromCharCode(...crypto.getRandomValues(new Uint8Array(24)))).replace(/\+/g, '-').replace(/\//g, '_')}`;
    const hash = await sha(key);
    await env.PINNED.put(`pwtok:${hash}`, member);
    // kept to the passkey that made it, as a session is (worker.js holds)
    await env.PINNED.put(`pwkey:${member}`, JSON.stringify({ h: hash, i: body.id, t: Date.now() }));
    await env.PINNED.delete(`pwused:${member}`);
    return json({ key, url: `https://mail.${env.DOMAIN}`, passwords: { made: Math.floor(Date.now() / 1000), used: null } });
  }
  if (req.method === 'POST' && path === '/api/aliases/passwords/delete') {
    const old = await read(env, `pwkey:${member}`, null);
    if (old) {
      await env.PINNED.delete(`pwtok:${old.h}`);
      await env.PINNED.delete(`pwkey:${member}`);
    }
    await env.PINNED.delete(`pwused:${member}`);
    return json({ passwords: null });
  }
  // the first of today's offers from `n` on that no one holds: an alias is
  // made only from one of these, so no one picks an address of their own
  if (req.method === 'POST' && path === '/api/aliases/next') {
    const o = await nextFree(env, h, member, Math.max(0, Number(body?.n) || 0));
    if (o) return json({ address: `${o.local}@${env.DOMAIN}`, n: o.n, day: o.day });
    return json({ error: `that was all ${OFFERS} of today's addresses; make one of them, or come back tomorrow` }, 429);
  }
  if (req.method === 'POST' && path === '/api/aliases') {
    // today's, or yesterday's for a dialog left open over midnight
    const day = String(body?.day || '');
    const n = body?.n;
    if (![dayOf(h.now()), dayOf(h.now() - 86400)].includes(day) || !Number.isInteger(n) || n < 0 || n >= OFFERS) {
      return json({ error: 'that address was not offered to you; press Another' }, 403);
    }
    const ttl = ttlOf(body?.ttl ?? 0);
    if (ttl === null) return json({ error: `turns off after ${TTLS.slice(1).join(', ')} days, or never` }, 400);
    const local = await offered(env, member, day, n);
    const label = String(body?.label || '').trim().slice(0, LABEL);
    const r = await make(env, h, member, local, label, ttl ? { e: h.now() + ttl * dayLen(env) } : {});
    return r.error ? json({ error: r.error }, r.status) : json(r);
  }
  const m = path.match(/^\/api\/aliases\/([a-z0-9.]+)(\/delete)?$/);
  if (req.method === 'POST' && m) {
    const local = m[1];
    const [a, owned] = await Promise.all([read(env, `alias:${local}`, null), env.PINNED.get(`owner:${member}:${local}`)]);
    if (!a || a.m !== member || owned === null) return json({ error: 'no such alias of yours' }, 404);
    if (m[2]) {
      // for good, so the passkey for this one delete: the browser's own
      // prompt, which a page that frames this one can neither hide nor answer
      if (!(await h.confirmed(body))) return json({ error: 'your passkey was not accepted for this' }, 403);
      await env.PINNED.put(`alias:${local}`, JSON.stringify({ gone: h.now() }));
      await env.PINNED.delete(`owner:${member}:${local}`);
      await env.PINNED.delete(`seen:${local}`);
      return json({ gone: true });
    }
    const t = h.now();
    if (typeof body?.label === 'string') a.l = body.label.trim().slice(0, LABEL);
    // the switch: either way, whatever it was to do on its own is done with
    if (typeof body?.on === 'boolean' && body.on !== live(a, t)) {
      a.x = body.on ? undefined : (a.on && a.e && t >= a.e ? a.e : t);
      a.on = body.on;
      a.e = undefined;
    }
    // a time to turn off, for one left on: none for one just switched off
    if (body?.ttl !== undefined && body?.on !== false) {
      const ttl = ttlOf(body.ttl);
      if (ttl === null) return json({ error: `turns off after ${TTLS.slice(1).join(', ')} days, or never` }, 400);
      if (!live(a, t)) return json({ error: 'it is off already' }, 409);
      a.e = ttl ? t + ttl * dayLen(env) : undefined;
    }
    await env.PINNED.put(`alias:${local}`, JSON.stringify(a));
    let seen = await read(env, `seen:${local}`, null);
    // "That's fine": the new sites join the fine ones
    if (body?.fine === true && seen?.p?.length) {
      seen = { ...seen, a: [...new Set([...(seen.a || []), ...seen.p])].slice(-SITES), p: [] };
      await env.PINNED.put(`seen:${local}`, JSON.stringify(seen));
    }
    return json({ alias: shown(env, local, a, t, seen) });
  }
  return json({ error: 'not here' }, 404);
}

// Mail to an address no rule of its own matched (the catch-all): an alias
// that is on goes to its member's confirmed address; anything else bounces.
// Nothing of the message is logged.
export async function aliasMail(message, env, h) {
  const [local, domain] = String(message.to || '').toLowerCase().split('@');
  if (domain !== env.DOMAIN || !LOCAL.test(local || '')) return message.setReject('No such address here');
  const a = await read(env, `alias:${local}`, null);
  if (!a || !a.m || !NAME.test(a.m)) return message.setReject('No such address here');
  const [owned, forgot] = await Promise.all([
    env.PINNED.get(`owner:${a.m}:${local}`),
    env.PINNED.get(`forgot:${a.m}`),
  ]);
  if (owned === null || Number(forgot || 0) >= a.t) return message.setReject('No such address here');
  if (!live(a, Math.floor(Date.now() / 1000))) return message.setReject('This address is turned off');
  const s = h.stateIn(await h.listsShared(env), env, a.m);
  // their inbox, if they have one: locked and held, and forwarded as well
  // if they want it (or if it could not be held)
  const key = await mailKey(env, a.m);
  const held = key ? await holdMail(env, a.m, key, message, local) : false;
  if (key && held && !key.fwd) {
    // kept, and not forwarded
  } else {
    if (!s.email || !s.confirmed) return held ? undefined : message.setReject('This address has nowhere to go yet');
    await message.forward(s.email);
  }
  // which headers Cloudflare adds, and where, for the owner's own aliases:
  // their names alone, to see where its checks of the sender can be read
  if (a.m === OWNER) console.log(`alias: header names ${[...message.headers.keys()].join(' ')}`);
  try {
    await noted(env, local, await read(env, `seen:${local}`, null), siteOf(message.headers.get('from')));
  } catch {
    // sent on already; a site not noted is no more than that
  }
}

// the site it came from, kept if new: the first one is the alias's own, a
// later one is shown as new until the member says it is fine. Written only
// when there is a new one, as KV's writes a day are few.
async function noted(env, local, seen, site) {
  if (!site) return;
  const s = { a: seen?.a || [], p: seen?.p || [] };
  if (s.a.includes(site) || s.p.includes(site) || s.a.length + s.p.length >= SITES) return;
  (s.a.length ? s.p : s.a).push(site);
  await env.PINNED.put(`seen:${local}`, JSON.stringify(s));
}

// Passwords' username generator, as a SimpleLogin server would answer it:
// POST /api/alias/random/new?hostname=<site>, the key in Authentication.
// From the browser extension or the vault's page, so from any origin, and
// with no cookie: the key is all it has, and it can make aliases, no more.
export async function passwordsApi(req, env, url, h) {
  const cors = {
    'access-control-allow-origin': '*',
    'access-control-allow-headers': 'authentication, content-type',
    'access-control-allow-methods': 'POST, OPTIONS',
    'access-control-max-age': '600',
  };
  const out = (v, status = 200) => new Response(JSON.stringify(v), { status, headers: { 'content-type': 'application/json', 'cache-control': 'no-store', ...cors } });
  if (req.method === 'OPTIONS') return new Response(null, { status: 204, headers: cors });
  if (req.method !== 'POST') return out({ error: 'POST' }, 405);
  const key = req.headers.get('authentication') || '';
  const hash = /^cmty_[A-Za-z0-9_-]{32}$/.test(key) ? await sha(key) : '';
  const member = hash ? await env.PINNED.get(`pwtok:${hash}`) : null;
  const rec = member && NAME.test(member) ? await read(env, `pwkey:${member}`, null) : null;
  // a member still, its passkey still theirs, not signed out everywhere since
  if (!rec || rec.h !== hash || !rec.i || !Number.isFinite(rec.t) || Number((await env.PINNED.get(`forgot:${member}`)) || 0) * 1000 >= rec.t || !(await h.holds(member, rec.i, rec.t))) {
    return out({ error: 'that key was disconnected, or is not one of ours' }, 401);
  }
  const label = String(url.searchParams.get('hostname') || '').toLowerCase().replace(/^www\./, '').replace(/[^a-z0-9.-]/g, '').slice(0, LABEL);
  // asked again for the same site: the alias just made, not another
  const recent = `pwsite:${member}:${label}`;
  const was = await env.PINNED.get(recent);
  if (was) {
    const a = await read(env, `alias:${was}`, null);
    if (a && a.m === member && live(a, h.now()) && (await env.PINNED.get(`owner:${member}:${was}`)) !== null) return out({ alias: `${was}@${env.DOMAIN}` }, 201);
  }
  const o = await nextFree(env, h, member, 0);
  if (!o) return out({ error: `that was all ${OFFERS} of today's addresses; come back tomorrow` }, 429);
  const r = await make(env, h, member, o.local, label, { by: 'pw' });
  if (r.error) return out({ error: r.error }, r.status);
  await env.PINNED.put(recent, o.local, { expirationTtl: 600 });
  await env.PINNED.put(`pwused:${member}`, String(h.now()));
  return out({ alias: r.alias.address }, 201);
}

// a member's aliases when they go (leaving.js): every one deleted, its
// address kept from anyone else, and nothing left saying whose it was
export async function forgetAliases(env, member) {
  // first: whatever the listing below misses is dead by this already. A day
  // outlasts any request still running and KV's minute to catch up, and after
  // it nothing says they were here
  await env.PINNED.put(`forgot:${member}`, String(Math.floor(Date.now() / 1000)), { expirationTtl: 86400 });
  for (const local of await listOf(env, member)) {
    const a = await read(env, `alias:${local}`, null);
    if (a && a.m === member) await env.PINNED.put(`alias:${local}`, JSON.stringify({ gone: Math.floor(Date.now() / 1000) }));
    await env.PINNED.delete(`owner:${member}:${local}`);
    await env.PINNED.delete(`seen:${local}`);
  }
  const key = await read(env, `pwkey:${member}`, null);
  if (key) await env.PINNED.delete(`pwtok:${key.h}`);
  await env.PINNED.delete(`pwkey:${member}`);
  await env.PINNED.delete(`pwused:${member}`);
}

// a name an alias holds: no member is given an address under it (setEmail)
export async function aliasHolds(env, name) {
  return LOCAL.test(name) && (await env.PINNED.get(`alias:${name}`)) !== null;
}

// The zone's catch-all sends to this Worker, kept so on the reads already
// made (worker.js sharedState). WORKER is the Worker's own name.
export async function keepCatchAll(env, h) {
  if (!env.WORKER) return;
  const z = await h.zone(env);
  const r = await h.cf(env, 'GET', `/zones/${z.id}/email/routing/rules/catch_all`);
  const act = (r.result?.actions || [])[0];
  if (r.result?.enabled && act?.type === 'worker' && (act.value || [])[0] === env.WORKER) return;
  await h.cf(env, 'PUT', `/zones/${z.id}/email/routing/rules/catch_all`, {
    name: 'Commonty aliases',
    enabled: true,
    matchers: [{ type: 'all' }],
    actions: [{ type: 'worker', value: [env.WORKER] }],
  });
}
