// A member's own aliases: <words>.<digits>@<domain>, one for each site
// they give an address to. Mail to one reaches the email() handler here
// through the zone's catch-all rule - the members' and the release's
// aliases keep their own rules, which come first - and goes on to the
// member's confirmed address, or bounces if the alias is off or gone. Only
// this Worker knows which aliases are whose; no box is ever told.
//
// KV (PINNED): alias:<local> {m, l, on, t, x} (member, label, on, made, off
// since); owner:<member>:<local>, one key per alias, so no write ever
// rewrites a list another request is changing; forgot:<member>, when they
// left, for a day. A deleted
// alias stays as alias:<local> {gone}, with no member, so its address is
// never given out again. Mail goes on only while both alias:<local> and its
// owner key say so: a change racing a delete can write the first back, never
// the second, and a leaver's aliases made before they left stay dead even
// if a listing missed one.

const MAX = 100;
// the addresses a member is offered in a day, the same ten however often
// they ask: no one presses Another until an address they like comes up,
// and no more than ten a day are made, as every one is kept for good
const OFFERS = 10;
const LABEL = 60;
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

const shown = (env, local, a) => ({ address: `${local}@${env.DOMAIN}`, label: a.l || '', on: !!a.on, made: a.t, off: a.x || null });

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
    const all = await Promise.all(locals.map(async (l) => [l, await read(env, `alias:${l}`, null)]));
    const aliases = all.filter(([, a]) => a && a.m === member).map(([l, a]) => shown(env, l, a));
    aliases.sort((x, y) => y.made - x.made);
    return json({ aliases, max: MAX });
  }
  // the first of today's offers from `n` on that no one holds: an alias is
  // made only from one of these, so no one picks an address of their own
  if (req.method === 'POST' && path === '/api/aliases/next') {
    const day = dayOf(h.now());
    for (let n = Math.max(0, Number(body?.n) || 0); n < OFFERS; n++) {
      const local = await offered(env, member, day, n);
      if (await free(env, h, local)) return json({ address: `${local}@${env.DOMAIN}`, n, day });
    }
    return json({ error: `that was all ${OFFERS} of today's addresses; make one of them, or come back tomorrow` }, 429);
  }
  if (req.method === 'POST' && path === '/api/aliases') {
    // today's, or yesterday's for a dialog left open over midnight
    const day = String(body?.day || '');
    const n = body?.n;
    if (![dayOf(h.now()), dayOf(h.now() - 86400)].includes(day) || !Number.isInteger(n) || n < 0 || n >= OFFERS) {
      return json({ error: 'that address was not offered to you; press Another' }, 403);
    }
    const local = await offered(env, member, day, n);
    const label = String(body?.label || '').trim().slice(0, LABEL);
    if ((await listOf(env, member)).length >= MAX) return json({ error: `you have ${MAX} aliases, the most there can be; delete one to make another` }, 409);
    if (!(await free(env, h, local))) return json({ error: 'that address was just taken; press Another' }, 409);
    const a = { m: member, l: label, on: true, t: h.now() };
    await env.PINNED.put(`alias:${local}`, JSON.stringify(a));
    await env.PINNED.put(`owner:${member}:${local}`, '');
    return json({ alias: shown(env, local, a) });
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
      return json({ gone: true });
    }
    if (typeof body?.label === 'string') a.l = body.label.trim().slice(0, LABEL);
    if (typeof body?.on === 'boolean' && body.on !== a.on) {
      a.on = body.on;
      a.x = a.on ? undefined : h.now();
    }
    await env.PINNED.put(`alias:${local}`, JSON.stringify(a));
    return json({ alias: shown(env, local, a) });
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
  if (!a.on) return message.setReject('This address is turned off');
  const s = h.stateIn(await h.listsShared(env), env, a.m);
  if (!s.email || !s.confirmed) return message.setReject('This address has nowhere to go yet');
  await message.forward(s.email);
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
  }
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
