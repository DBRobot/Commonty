// The fleet's zone, at dns.<domain>: the only holder of its DNS token. No
// box has one, so no box can point a name anywhere. It does two things:
//
// The fleet's names. What `*`, the bare name and each public host point at
// is fixed by the release (fleet.js RECORDS, from the gateway's own
// config): the gateway's address on the fleet's network, or a proxied
// CNAME to the tunnel while the door is open. Every five minutes the zone is
// put in step with it. Only an A at that address or a CNAME to that tunnel
// is ever changed; mail, TXT, CAA, anyone else's record: never.
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
// Bindings: DOMAIN, ZONE (plain text, fleet/dns-worker.json); DNS_TOKEN
// (secret: Zone DNS Edit on this zone, nothing more); CF_API (a test's fake).

import { BOXES, NAMES, RECORDS } from './fleet.js';
import { controllerApi, controllerSigned } from './controller.js';

// lego asks again after a failure; a recorded request is good this long
const SKEW = 300;
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

export async function handle(req, env) {
  const url = new URL(req.url);
  // the body is read before any answer: workerd faults a request whose
  // stream is left behind, and the next one with it
  const text = await req.text().catch(() => '');
  const known = ['/api/challenge', '/api/controller'];
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
  const ours = `${tunnel}.cfargotunnel.com`;
  const done = [];
  const have = {};
  for (const r of await dns(env, 'GET', '?per_page=5000')) {
    if (!have[r.name]) have[r.name] = [];
    have[r.name].push(r);
  }

  const want = {
    [zone]: open && bare ? ['CNAME', ours, true] : ['A', address, false],
    [`*.${zone}`]: ['A', address, false],
  };
  for (const h of hosts) want[`${h}.${zone}`] = open ? ['CNAME', ours, true] : null;
  for (const [b, a] of Object.entries(boxes)) want[`*.${b}.${zone}`] = ['A', a, false];

  // a record this Worker made: the A at the gateway's address, or a CNAME to
  // this tunnel
  const mine = (r) => (r.type === 'A' && addresses.has(r.content)) || (r.type === 'CNAME' && r.content === ours);
  const remove = async (name, r, why) => {
    await dns(env, 'DELETE', `/${r.id}`);
    done.push(`removed ${name} ${r.type} (${why})`);
  };

  // a name that was public and no longer is: its proxied record goes, so
  // the wildcard answers with the gateway's address again
  // and a box that has none any more loses its wildcard, if it was ours
  const boxWildcard = new RegExp(`^\\*\\.[a-z0-9-]+\\.${zone.replace(/\./g, '\\.')}$`);
  for (const [name, records] of Object.entries(have)) {
    if (Object.hasOwn(want, name)) continue;
    for (const r of records) if (r.type === 'CNAME' && r.content === ours) await remove(name, r, 'no longer public');
    if (boxWildcard.test(name)) for (const r of records) if (r.type === 'A' && addresses.has(r.content)) await remove(name, r, 'no longer a box with names');
  }
  for (const [name, spec] of Object.entries(want)) {
    const current = have[name] || [];
    if (spec === null) {
      for (const r of current) if (r.type === 'CNAME' && r.content === ours) await remove(name, r, 'not public');
      continue;
    }
    const [type, content, proxied] = spec;
    const body = { type, name, content, proxied, ttl: 1 };
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
      } else if (match[0].content !== content || match[0].proxied !== proxied) {
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

export default {
  fetch: handle,
  async scheduled(_event, env, ctx) {
    ctx.waitUntil(
      converge(env).then((done) => {
        for (const d of done) console.log(d);
      }),
    );
  },
};
