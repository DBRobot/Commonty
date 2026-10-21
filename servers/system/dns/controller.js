// Home controllers' names (docs/dev/home-controller.md): <label>.home.<domain>,
// the label made from the controller's key, so the name says nothing of the
// house and no other controller can be named with it. A controller is
// a household's device, not a box: no release names it, and its name and
// key are given by a member's root, checked here by the boxes' own Rust
// (client/mail, controllerName). No box can sign that, so no box can give a
// controller a name, move it, or get a certificate for it.
//
// What is kept, in the zone itself: _controller.<label>.home.<domain> TXT
// "key=<the controller's ed25519 key>", and <label>.home.<domain> A at the
// controller's address on the fleet's network. Nothing names the member.

import { initSync, controllerName, controllerWord, signedBy } from './dd_mail.js';
import wasm from './dd_mail_bg.wasm';
import { MEMBERS } from './fleet.js';
import { mode, agreedMember, isMember, entryOf, differ } from './identity.js';

let ready = false;
const load = () => {
  if (!ready) {
    initSync({ module: wasm });
    ready = true;
  }
};

const SKEW = 300;
// a household or two each: far past any fleet's, far short of the zone's limit
const MAX_CONTROLLERS = 64;
const LABEL = /^[a-z2-7]{16}$/;
// the fleet's network: Headscale hands out 100.64.0.0/10
const NETWORK = /^100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.([0-9]{1,3})\.([0-9]{1,3})$/;

const fresh = (at) => Number.isInteger(Number(at)) && Math.abs(Math.floor(Date.now() / 1000) - Number(at)) <= SKEW;
const unquote = (s) => String(s).replace(/^"(.*)"$/, '$1');

export const nameOf = (label, domain) => `${label}.home.${domain}`;
const keyName = (label, domain) => `_controller.${label}.home.${domain}`;

// the key this controller registered, or null
export async function keyOf(dns, env, label) {
  const name = keyName(label, env.DOMAIN);
  const found = (await dns(env, 'GET', `?type=TXT&name=${encodeURIComponent(name)}`)).filter((r) => r.name === name);
  const m = found.map((r) => /^key=([A-Za-z0-9+/=]{44})$/.exec(unquote(r.content))).find(Boolean);
  return m ? m[1] : null;
}

async function point(dns, env, label, address) {
  const name = nameOf(label, env.DOMAIN);
  const have = (await dns(env, 'GET', `?name=${encodeURIComponent(name)}`)).filter((r) => r.name === name);
  const body = { type: 'A', name, content: address, proxied: false, ttl: 60 };
  const a = have.find((r) => r.type === 'A');
  if (!a) await dns(env, 'POST', '', body);
  else if (a.content !== address) await dns(env, 'PUT', `/${a.id}`, body);
}

// The member whose root gave `controller` its name `label`, or a throw. Who
// is a member, and their root now, are the control plane's to say
// (IDENTITY_FROM, identity.js); only the word itself is checked here. The
// entry the request brings says whose word it claims to be, and nothing more
// once the agreed state is what counts.
async function memberWord(env, entry, label, controller, sig) {
  const how = mode(env);
  let own, ownWhy;
  if (how !== 'agreed') {
    try {
      own = controllerName(entry, label, controller, sig, env.DOMAIN, MEMBERS);
    } catch (e) {
      ownWhy = String(e);
    }
  }
  let agreed, agreedWhy;
  // who the word claims to be, for the shadow log: the entry's name
  let who = '';
  try {
    who = String(JSON.parse(entry).entry.name || '');
  } catch {
    /* not an entry: the checks say so */
  }
  if (how !== 'replay') {
    try {
      const name = JSON.parse(entry).entry.name;
      const m = await agreedMember(env, name);
      if (!isMember(m)) throw new Error('not a member');
      agreed = controllerWord(entryOf(m), label, controller, sig, env.DOMAIN);
    } catch (e) {
      agreedWhy = String(e.message || e);
    }
  }
  if (how === 'shadow') differ('controller', who, { name: own || null }, { name: agreed || null }, { own: ownWhy ?? null, agreed: agreedWhy ?? null, label });
  if (how === 'agreed') {
    if (agreedWhy) throw new Error(agreedWhy);
    return agreed;
  }
  if (ownWhy) throw new Error(ownWhy);
  return own;
}

// POST /api/controller {label, controller, entry, sig, address, at, own}
//   sig: the member's root over commonty controller name v1\0<name>\0<controller>
//   own: the controller over commonty controller address v1\0<label>\0<address>\0<at>
// The member's word comes every time, its address moved or not, and is
// checked against this release's members: a controller whose member has left
// moves and certifies nothing.
export async function controllerApi(body, dns, env, json) {
  load();
  const { label, address, at, own, controller, entry, sig } = body || {};
  if (typeof label !== 'string' || !LABEL.test(label)) return json({ error: 'not a controller' }, 400);
  if (typeof address !== 'string' || !NETWORK.test(address)) return json({ error: 'not an address on the fleet\'s network' }, 400);
  if (!fresh(at)) return json({ error: 'stale' }, 403);
  if (typeof controller !== 'string' || typeof entry !== 'string' || typeof sig !== 'string') return json({ error: 'missing' }, 400);
  const said = `commonty controller address v1\0${label}\0${address}\0${at}`;
  try {
    const known = await keyOf(dns, env, label);
    if (known && known !== controller) return json({ error: 'that name is another controller\'s' }, 409);
    try {
      await memberWord(env, entry, label, controller, sig);
    } catch (e) {
      return json({ error: String(e.message || e) }, 403);
    }
    if (!signedBy(controller, said, String(own))) return json({ error: 'not signed by that controller' }, 403);
    if (!known) {
      // the zone is the fleet's, and a member's word makes a name in it:
      // no more names than a fleet of households has controllers
      const all = (await dns(env, 'GET', `?type=TXT&per_page=1000`)).filter((r) =>
        /^_controller\.[a-z2-7]{16}\.home\./.test(r.name),
      );
      if (all.length >= MAX_CONTROLLERS) return json({ error: 'this fleet has as many home controllers as it may' }, 429);
      await dns(env, 'POST', '', { type: 'TXT', name: keyName(label, env.DOMAIN), content: `key=${controller}`, ttl: 300 });
    }
    await point(dns, env, label, address);
  } catch (e) {
    return json({ error: String(e.message || e) }, 502);
  }
  return json({ ok: true, name: nameOf(label, env.DOMAIN) });
}

// a challenge for a controller's own name, signed by its own key, with its
// member's word for it, a member still
export async function controllerSigned(dns, env, { controller, at, op, name, value, sig, key, entry, name_sig }) {
  load();
  if (typeof controller !== 'string' || !LABEL.test(controller)) return false;
  if (name !== nameOf(controller, env.DOMAIN)) return false;
  const known = await keyOf(dns, env, controller);
  if (!known || key !== known || typeof entry !== 'string' || typeof name_sig !== 'string') return false;
  try {
    await memberWord(env, entry, controller, known, name_sig);
  } catch {
    return false;
  }
  return signedBy(known, `commonty acme v1\0${controller}\0${at}\0${op}\0${name}\0${value}`, String(sig));
}
