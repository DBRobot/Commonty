// The fleet's zone, at dns.<domain>: the only holder of its DNS token. No
// box has one, so no box can point a name anywhere. It does two things:
//
// The fleet's names. What `*`, the bare name and each public host point at
// is fixed by the release (fleet.js RECORDS, from the gateway's own
// config): the gateway's address on the fleet's network, or a proxied
// CNAME to the tunnel while the door is open. Every five minutes the zone is
// put in step with it. Only an A at that address or a CNAME to that tunnel
// is ever changed; mail, TXT, CAA, anyone else's record: never - but for
// the domain's mail key, below.
//
// Mail. The release's DKIM key (fleet/mail.json dkim, `dd mail dkim`) at
// <selector>._domainkey, so a member's mail sent from their own device is
// known to be the domain's; and a DMARC record when the domain has none.
// Only a record of exactly those names and kinds, and one this Worker made
// (its comment says so) is ever changed.
//
// Certificates. A box proves to Let's Encrypt that it serves a name by a
// TXT record at _acme-challenge.<name>, and only this Worker writes one: a
// box asks with a request its TPM-held key signed (fleet.js BOXES), for a
// name the release says it serves (fleet.js NAMES, from that box's own
// nginx). No other kind of record, no other name, no other box's name.
//
// Home controllers (controller.js): a household's device given a name by a
// member's root, which then asks for its own name's challenges with its own
// key, as a box does with its TPM's.
//
// Failover. Where a name points follows the control plane's agreed state:
// a service with one writer at the box holding its lease, a placed one
// with none at a live box whose heartbeat says it runs it, the gateway's
// names (`*`, the bare name, the public hosts) at the box holding the
// gateway's. Each box has a tunnel of its own, whose secret the box made
// (it asks for the tunnel here, signed); a public name is a CNAME to the
// tunnel of the box it follows. Agreement older than two minutes moves
// nothing: what the names point at stays until the voters agree again.
// A box that takes a lease asks for the names at once (/api/converge,
// signed), so a move does not wait for the cron.
//
// Bindings: DOMAIN, ZONE (plain text, fleet/dns-worker.json); DNS_TOKEN
// (secret: Zone DNS Edit on this zone, nothing more); CONTROL (the control
// plane's relay, whose witness holds the agreed state) or CONTROL_URL (a
// test's fake); ACCOUNT and TUNNEL_TOKEN (secret: Cloudflare Tunnel Edit on
// the account, nothing more) for the boxes' own tunnels; CF_API (a test's
// fake).

import * as fleet from './fleet.js';
import { controllerApi, controllerSigned } from './controller.js';

const { BOXES, NAMES, RECORDS } = fleet;
// a fleet.js from before mail has no MAIL
const MAIL = fleet.MAIL ?? null;

// lego asks again after a failure; a recorded request is good this long
const SKEW = 300;
// agreement older than this moves no name
const FRESH = 120_000;
// a name that follows a lease: how long a resolver may keep it
const MOVING_TTL = 60;
// what Let's Encrypt asks a TXT to hold: base64url of a sha256
const VALUE = /^[A-Za-z0-9_-]{43}$/;

const enc = new TextEncoder();
const unb64 = (s) => Uint8Array.from(atob(String(s).replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));

// r||s, which WebCrypto wants, from the DER a TPM or openssl may give
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
  const out = new Uint8Array(64);
  out.set(part());
  out.set(part(), 32);
  return out;
}

async function signedBy(box, message, sig) {
  try {
    const key = await crypto.subtle.importKey('spki', unb64(BOXES[box]), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['verify']);
    return await crypto.subtle.verify({ name: 'ECDSA', hash: 'SHA-256' }, key, rawSig(unb64(sig)), enc.encode(message));
  } catch {
    return false;
  }
}

const json = (v, status = 200) =>
  new Response(JSON.stringify(v), { status, headers: { 'content-type': 'application/json', 'cache-control': 'no-store' } });

async function dns(env, method, path, body) {
  const r = await fetch(`${env.CF_API || 'https://api.cloudflare.com/client/v4'}/zones/${env.ZONE}/dns_records${path}`, {
    method,
    headers: { authorization: `Bearer ${env.DNS_TOKEN}`, 'content-type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
  });
  const v = await r.json().catch(() => ({}));
  if (!v.success) throw new Error(`cloudflare: ${JSON.stringify(v.errors || r.status)}`);
  return v.result;
}

// Cloudflare may hand a TXT back in quotes
const unquote = (s) => String(s).replace(/^"(.*)"$/, '$1');

// who holds each lease, by the agreed state, when that is fresh: null when
// there is no agreement to go by
export async function holders(env) {
  try {
    let r;
    if (env.CONTROL) r = await env.CONTROL.get(env.CONTROL.idFromName('relay')).fetch('https://control/leases');
    else if (env.CONTROL_URL) r = await fetch(env.CONTROL_URL);
    else return null;
    if (!r.ok) return null;
    const v = await r.json();
    if (!v || typeof v.time !== 'number' || Date.now() - v.time > FRESH) return null;
    const out = {};
    for (const [role, l] of Object.entries(v.leases || {})) {
      if (l && typeof l.holder === 'string' && l.holder && l.expires > v.time) out[role] = l.holder;
    }
    // a box that joined on its own key: its name, and its address on the
    // fleet's network if it is one (100.64.0.0/10)
    const joined = {};
    for (const [name, j] of Object.entries(v.joined || {})) {
      const n = j && j.ipv4;
      if (!/^j[0-9a-f]{10}$/.test(name) || !Number.isInteger(n) || n >>> 22 !== (100 << 2) + 1) continue;
      joined[name] = [n >>> 24, (n >>> 16) & 255, (n >>> 8) & 255, n & 255].join('.');
    }
    Object.defineProperty(out, 'joined', { value: joined });
    // which live boxes run each placed service, by their heartbeats
    const runs = {};
    for (const [svc, bs] of Object.entries(v.runs || {})) {
      if (Array.isArray(bs)) runs[svc] = bs.filter((b) => typeof b === 'string' && b).sort();
    }
    Object.defineProperty(out, 'runs', { value: runs });
    return out;
  } catch {
    return null;
  }
}

async function tunnelApi(env, method, path, body) {
  const r = await fetch(`${env.CF_API || 'https://api.cloudflare.com/client/v4'}/accounts/${env.ACCOUNT}/cfd_tunnel${path}`, {
    method,
    headers: { authorization: `Bearer ${env.TUNNEL_TOKEN}`, 'content-type': 'application/json' },
    body: body ? JSON.stringify(body) : undefined,
  });
  const v = await r.json().catch(() => ({}));
  if (!v.success) throw new Error(`cloudflare: ${JSON.stringify(v.errors || r.status)}`);
  return v.result;
}

const tunnelName = (box) => `commonty-${box}`;

// each box's own tunnel, by its name at Cloudflare
async function tunnels(env) {
  if (!env.TUNNEL_TOKEN || !env.ACCOUNT) return {};
  const out = {};
  for (const t of await tunnelApi(env, 'GET', '?is_deleted=false&per_page=1000')) {
    for (const box of Object.keys(BOXES)) if (t.name === tunnelName(box)) out[box] = t.id;
  }
  return out;
}

// a box's ask, signed with its TPM-held key: [box, at] when it is one
async function signedAsk(body, what, ...parts) {
  const { box, at, sig } = body || {};
  if (typeof box !== 'string' || !Object.hasOwn(BOXES, box)) return [null, json({ error: 'no such box' }, 403)];
  const t = Number(at);
  if (!Number.isInteger(t) || Math.abs(Math.floor(Date.now() / 1000) - t) > SKEW) return [null, json({ error: 'stale' }, 403)];
  if (!(await signedBy(box, [`commonty ${what} v1`, box, at, ...parts].join('\0'), sig))) return [null, json({ error: 'not signed by that box' }, 403)];
  return [box, null];
}

// a box's own tunnel, made with the secret the box made: the box keeps it,
// this Worker never does. Asked again, the old one goes and a new one
// comes: the box lost its secret
async function tunnel(env, body) {
  if (!env.TUNNEL_TOKEN || !env.ACCOUNT) return json({ error: 'no tunnels here yet' }, 404);
  const secret = body && body.secret;
  if (typeof secret !== 'string' || !/^[A-Za-z0-9+/]{43}=$/.test(secret)) return json({ error: 'not a tunnel secret' }, 400);
  const [box, refused] = await signedAsk(body, 'tunnel', secret);
  if (refused) return refused;
  try {
    const old = (await tunnelApi(env, 'GET', `?is_deleted=false&name=${encodeURIComponent(tunnelName(box))}`)).filter(
      (t) => t.name === tunnelName(box),
    );
    // a request is good once: a tunnel made at or after it was asked for
    // stands, so a request seen and sent again cannot put an older
    // secret back in place of the box's
    if (old.some((t) => !(Date.parse(t.created_at) / 1000 < Number(body.at)))) {
      return json({ error: 'a tunnel newer than this request stands' }, 409);
    }
    for (const t of old) {
      await tunnelApi(env, 'DELETE', `/${t.id}/connections`).catch(() => {});
      await tunnelApi(env, 'DELETE', `/${t.id}`);
    }
    const made = await tunnelApi(env, 'POST', '', { name: tunnelName(box), tunnel_secret: secret, config_src: 'local' });
    return json({ id: made.id, account: env.ACCOUNT });
  } catch (e) {
    return json({ error: String(e.message || e) }, 502);
  }
}

export async function handle(req, env) {
  const url = new URL(req.url);
  // the body is read before any answer: workerd faults a request whose
  // stream is left behind, and the next one with it
  const text = await req.text().catch(() => '');
  const known = ['/api/challenge', '/api/controller', '/api/tunnel', '/api/converge'];
  if (!known.includes(url.pathname) || req.method !== 'POST') return json({ error: 'not here' }, 404);
  // an entry rides along with a controller's naming and its challenges
  if (text.length > 65536) return json({ error: 'too long' }, 413);
  let body;
  try {
    body = JSON.parse(text);
  } catch {
    return json({ error: 'not json' }, 400);
  }
  if (url.pathname === '/api/controller') return controllerApi(body, dns, env, json);
  if (url.pathname === '/api/tunnel') return tunnel(env, body);
  if (url.pathname === '/api/converge') {
    const [, refused] = await signedAsk(body, 'converge');
    if (refused) return refused;
    try {
      return json({ changed: (await converge(env)).length });
    } catch (e) {
      return json({ error: String(e.message || e) }, 502);
    }
  }
  const { box, controller, at, op, name, value, sig } = body || {};
  if (op !== 'present' && op !== 'cleanup') return json({ error: 'present or cleanup' }, 400);
  if (typeof value !== 'string' || !VALUE.test(value)) return json({ error: 'not a challenge' }, 400);
  const t = Number(at);
  if (!Number.isInteger(t) || Math.abs(Math.floor(Date.now() / 1000) - t) > SKEW) return json({ error: 'stale' }, 403);
  if (controller !== undefined) {
    let ok = false;
    try {
      ok = await controllerSigned(dns, env, body);
    } catch (e) {
      return json({ error: String(e.message || e) }, 502);
    }
    if (!ok) return json({ error: 'not that controller\'s name, or not signed by it' }, 403);
  } else {
    if (typeof box !== 'string' || !Object.hasOwn(BOXES, box)) return json({ error: 'no such box' }, 403);
    if (typeof name !== 'string' || !(NAMES[box] || []).includes(name)) return json({ error: 'not a name this box serves' }, 403);
    if (!(await signedBy(box, `commonty acme v1\0${box}\0${at}\0${op}\0${name}\0${value}`, sig))) return json({ error: 'not signed by that box' }, 403);
  }

  const fqdn = `_acme-challenge.${name}`;
  try {
    const have = (await dns(env, 'GET', `?type=TXT&name=${encodeURIComponent(fqdn)}&per_page=100`)).filter(
      (r) => r.type === 'TXT' && r.name === fqdn && unquote(r.content) === value,
    );
    if (op === 'present') {
      if (have.length === 0) await dns(env, 'POST', '', { type: 'TXT', name: fqdn, content: value, ttl: 60, comment: `acme ${box || controller}` });
    } else {
      for (const r of have) await dns(env, 'DELETE', `/${r.id}`);
    }
  } catch (e) {
    return json({ error: String(e.message || e) }, 502);
  }
  return json({ ok: true });
}

// The fleet's names, as the release has them (nix/lib/fleet-names.nix;
// what dd-dns did from the gateway before). Returns what it changed.
export async function converge(env) {
  if (!RECORDS) return [];
  const { zone, address, tunnel, hosts, open, bare } = RECORDS;
  // boxes with names of their own: *.<box>.<zone> at each one's address
  const boxes = RECORDS.boxes || {};
  const addresses = new Set([address, ...Object.values(boxes), ...(RECORDS.fleet || [])]);
  // where each name goes: the gateway's box and the boxes holding leases,
  // when the agreed state says so; the release's gateway otherwise. With a
  // control plane to ask and no fresh agreement, the names that move stay
  const follow = RECORDS.follow || {};
  const moving = Boolean(env.CONTROL || env.CONTROL_URL);
  const held = moving ? await holders(env) : null;
  const stay = moving && held === null;
  const own = await tunnels(env);
  const tunnelOf = (box) => (own[box] ? own[box] : box === RECORDS.home || !RECORDS.home ? tunnel : null);
  const gateway = (held && RECORDS.gateway && held[`svc/${RECORDS.gateway}`]) || RECORDS.home;
  const addressOf = (box) =>
    (RECORDS.addresses && box && RECORDS.addresses[box]) || (held && box && held.joined[box]) || address;
  // a name's box: its service's lease holder; for a placed service with no
  // lease, a live box running it, the gateway's first if it is one of them
  // (no name moves while it still runs there); else the gateway's
  const runnerOf = (svc) => {
    const on = (held && held.runs[svc]) || [];
    return on.includes(gateway) ? gateway : on.find((b) => RECORDS.addresses?.[b] || held.joined[b]);
  };
  const targetOf = (h) => (follow[h] && held && (held[`svc/${follow[h]}`] || runnerOf(follow[h]))) || gateway;
  const cnameTo = (box) => {
    const t = tunnelOf(box);
    return t ? `${t}.cfargotunnel.com` : null;
  };
  const tunnelsOurs = new Set([tunnel, ...Object.values(own)].filter(Boolean).map((t) => `${t}.cfargotunnel.com`));
  const ours = cnameTo(gateway) || `${tunnel}.cfargotunnel.com`;
  const done = [];
  const have = {};
  for (const r of await dns(env, 'GET', '?per_page=5000')) {
    if (!have[r.name]) have[r.name] = [];
    have[r.name].push(r);
  }

  const gw = addressOf(gateway);
  const want = {
    [zone]: open && bare ? ['CNAME', ours, true] : ['A', gw, false],
    [`*.${zone}`]: ['A', gw, false],
  };
  for (const h of hosts) {
    const to = cnameTo(targetOf(h));
    want[`${h}.${zone}`] = open && to ? ['CNAME', to, true] : null;
  }
  // a name of a service with one writer, on the fleet's network only: at
  // the address of the box holding its lease
  for (const h of Object.keys(follow)) {
    if (hosts.includes(h)) continue;
    want[`${h}.${zone}`] = ['A', addressOf(targetOf(h)), false, MOVING_TTL];
  }
  for (const [b, a] of Object.entries(boxes)) want[`*.${b}.${zone}`] = ['A', a, false];
  // the boxes that joined on their own keys: their stores' names
  for (const [b, a] of Object.entries((held && held.joined) || {})) want[`*.${b}.${zone}`] = ['A', a, false, MOVING_TTL];
  // no fresh agreement: what moves stays as it is
  const moves = new Set([zone, `*.${zone}`, ...hosts.map((h) => `${h}.${zone}`), ...Object.keys(follow).map((h) => `${h}.${zone}`)]);
  if (stay) for (const name of moves) delete want[name];

  // a record this Worker made: an A at one of the fleet's addresses, or a
  // CNAME to one of its tunnels
  // and an A on the fleet's network under a joined box's name
  const joinedName = new RegExp(`^\\*\\.j[0-9a-f]{10}\\.${zone.replace(/\./g, '\\.')}$`);
  const fleetIP = (ip) => /^100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.\d+\.\d+$/.test(ip);
  const mine = (r) =>
    (r.type === 'A' && (addresses.has(r.content) || (joinedName.test(r.name) && fleetIP(r.content)))) ||
    (r.type === 'CNAME' && tunnelsOurs.has(r.content));
  const remove = async (name, r, why) => {
    await dns(env, 'DELETE', `/${r.id}`);
    done.push(`removed ${name} ${r.type} (${why})`);
  };

  // a name that was public and no longer is: its proxied record goes, so
  // the wildcard answers with the gateway's address again
  // and a box that has none any more loses its wildcard, if it was ours
  const boxWildcard = new RegExp(`^\\*\\.[a-z0-9-]+\\.${zone.replace(/\./g, '\\.')}$`);
  for (const [name, records] of Object.entries(have)) {
    if (Object.hasOwn(want, name) || (stay && moves.has(name))) continue;
    for (const r of records) if (r.type === 'CNAME' && tunnelsOurs.has(r.content)) await remove(name, r, 'no longer public');
    if (boxWildcard.test(name)) for (const r of records) if (r.type === 'A' && mine(r) && !stay) await remove(name, r, 'no longer a box with names');
  }
  for (const [name, spec] of Object.entries(want)) {
    const current = have[name] || [];
    if (spec === null) {
      for (const r of current) if (r.type === 'CNAME' && tunnelsOurs.has(r.content)) await remove(name, r, 'not public');
      continue;
    }
    const [type, content, proxied, ttl = 1] = spec;
    const body = { type, name, content, proxied, ttl };
    const match = current.filter((r) => r.type === type);
    // what stands in the way: a CNAME shares its name with nothing, and an A
    // with no CNAME - but at the zone's own name Cloudflare flattens a CNAME,
    // and mail's records and the like stay beside it. Ours goes; anyone
    // else's is left, and so is the name
    const address = (r) => r.type === 'A' || r.type === 'AAAA' || r.type === 'CNAME';
    const clash = current.filter(
      (r) => r.type !== type && (type === 'CNAME' ? name !== zone || address(r) : r.type === 'CNAME'),
    );
    if (clash.some((r) => !mine(r))) {
      done.push(`left ${name} alone: it has records this Worker did not make`);
      continue;
    }
    // one name Cloudflare refuses does not keep the others from their records
    try {
      for (const r of clash) await remove(name, r, `replaced by ${type}`);
      if (match.length === 0) {
        await dns(env, 'POST', '', body);
        done.push(`added ${name} ${type} ${content}`);
      } else if (match[0].content !== content || match[0].proxied !== proxied || (ttl !== 1 && match[0].ttl !== ttl)) {
        if (!mine(match[0]) && type === 'CNAME') {
          done.push(`left ${name} alone: its CNAME points elsewhere`);
          continue;
        }
        await dns(env, 'PUT', `/${match[0].id}`, body);
        done.push(`updated ${name} ${type} ${content}`);
      }
    } catch (e) {
      done.push(`${name}: ${e.message || e}`);
    }
  }
  return done;
}

const MAIL_NOTE = 'commonty mail';

// the mail key's record and DMARC's, as fleet.js MAIL has them
export async function convergeMail(env) {
  if (!MAIL || !MAIL.zone) return [];
  const done = [];
  const want = [];
  if (MAIL.dkim && /^[a-z0-9-]{1,32}$/.test(MAIL.dkim.selector || '') && /^[A-Za-z0-9+/=]{100,1000}$/.test(MAIL.dkim.p || '')) {
    want.push({ name: `${MAIL.dkim.selector}._domainkey.${MAIL.zone}`, kind: 'v=DKIM1', content: `v=DKIM1; k=rsa; p=${MAIL.dkim.p}`, replace: true });
  }
  if (MAIL.dmarc && /^v=DMARC1;/.test(MAIL.dmarc)) want.push({ name: `_dmarc.${MAIL.zone}`, kind: 'v=DMARC1', content: MAIL.dmarc, replace: false });
  for (const w of want) {
    try {
      const have = (await dns(env, 'GET', `?type=TXT&name=${encodeURIComponent(w.name)}&per_page=100`)).filter((r) => r.type === 'TXT' && r.name === w.name);
      const same = have.filter((r) => unquote(r.content).replace(/" "/g, '').startsWith(w.kind));
      if (same.some((r) => unquote(r.content).replace(/" "/g, '') === w.content)) continue;
      const ours = same.find((r) => (r.comment || '') === MAIL_NOTE);
      if (ours) {
        await dns(env, 'PUT', `/${ours.id}`, { type: 'TXT', name: w.name, content: w.content, ttl: 3600, comment: MAIL_NOTE });
        done.push(`updated ${w.name} TXT`);
      } else if (same.length && !w.replace) {
        // the domain's own DMARC, set by someone: theirs to keep
        continue;
      } else if (same.length) {
        done.push(`left ${w.name} alone: it has a ${w.kind} record this Worker did not make`);
      } else {
        await dns(env, 'POST', '', { type: 'TXT', name: w.name, content: w.content, ttl: 3600, comment: MAIL_NOTE });
        done.push(`added ${w.name} TXT`);
      }
    } catch (e) {
      done.push(`${w.name}: ${e.message || e}`);
    }
  }
  return done;
}

export default {
  fetch: handle,
  async scheduled(_event, env, ctx) {
    ctx.waitUntil(
      Promise.all([converge(env), convergeMail(env)]).then(([done, mail]) => {
        for (const d of [...done, ...mail]) console.log(d);
      }),
    );
  },
};
