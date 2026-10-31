// Sign-in for every box (identity::session; servers/gate/src/sso.rs).
//
// /         sign in, or say who is signed in here
// /go       a box's hand-off: ?host=<one of its names>&nonce=<its challenge>
// /out      forget this browser's sign-in: ?then=<where to go after>
//
// Signing in makes a key in this browser that no page can read out
// (WebCrypto, kept in IndexedDB on this origin alone), and the passkey signs
// that the key speaks for this person until the sign-in ends. A box's
// hand-off is that key signing the box's name and the box's challenge.

const DOMAIN = location.hostname.replace(/^id\./, '');
const LIFE = 8 * 3600;
const $ = (id) => document.getElementById(id);
const enc = new TextEncoder();
const b64u = (buf) => btoa(String.fromCharCode(...new Uint8Array(buf))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const unb64u = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));
const now = () => Math.floor(Date.now() / 1000);

// a name of this fleet, and nothing that could carry more
const ours = (h) => /^[a-z0-9.-]{1,253}$/.test(h) && (h === DOMAIN || h.endsWith(`.${DOMAIN}`)) && !h.includes('..');

function show(which) {
  for (const id of ['signin', 'busy', 'done', 'gone']) $(id).hidden = id !== which;
  $('ways').hidden = which !== 'signin';
}
const say = (t) => { $('msg').textContent = t; };

// --- the sign-in kept in this browser: the key, and the passkey's word for it
const db = () => new Promise((ok, fail) => {
  const r = indexedDB.open('commonty-id', 1);
  r.onupgradeneeded = () => r.result.createObjectStore('s');
  r.onsuccess = () => ok(r.result);
  r.onerror = () => fail(r.error);
});
async function store(mode, f) {
  const d = await db();
  return new Promise((ok, fail) => {
    const t = d.transaction('s', mode);
    const req = f(t.objectStore('s'));
    t.oncomplete = () => ok(req?.result);
    t.onerror = () => fail(t.error);
  });
}
const kept = () => store('readonly', (s) => s.get('me')).catch(() => null);
const keep = (v) => store('readwrite', (s) => s.put(v, 'me'));
const forget = () => store('readwrite', (s) => s.delete('me')).catch(() => null);

async function current() {
  const me = await kept();
  return me?.d && me.d.exp > now() + 60 ? me : null;
}

// --- signing in: the passkey, over a key made here
async function signIn(user, asking) {
  // the passkeys on file for this name, as the box that sent us here has them
  const r = await fetch(`https://${asking}/_dd/sso/creds?user=${encodeURIComponent(user)}`, { credentials: 'omit' });
  const { ids } = r.ok ? await r.json() : { ids: [] };
  if (!ids.length) throw new Error('no passkey on file for that name');
  const keys = await crypto.subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
  const key = b64u(await crypto.subtle.exportKey('spki', keys.publicKey));
  const sid = b64u(crypto.getRandomValues(new Uint8Array(18)));
  const iat = now();
  const exp = iat + LIFE;
  const challenge = await crypto.subtle.digest('SHA-256', enc.encode(`commonty session v1\0${user}\0${key}\0${sid}\0${iat}\0${exp}`));
  const cred = await navigator.credentials.get({
    publicKey: {
      challenge,
      rpId: DOMAIN,
      allowCredentials: ids.map((id) => ({ type: 'public-key', id: unb64u(id) })),
      userVerification: 'preferred',
      timeout: 120000,
    },
  });
  const a = cred.response;
  const d = {
    user, passkey: b64u(cred.rawId), key, sid, iat, exp,
    assertion: { authenticatorData: b64u(a.authenticatorData), clientDataJSON: b64u(a.clientDataJSON), signature: b64u(a.signature) },
  };
  await keep({ keys, d });
  return { keys, d };
}

// --- a box's hand-off: its name and its challenge, signed with the key, and
// posted to that name (never anywhere a parameter says: the signature names
// the host, and only that host may receive it)
const handed = async (me, host, nonce) => ({
  d: b64u(enc.encode(JSON.stringify(me.d))),
  sig: b64u(await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, me.keys.privateKey, enc.encode(`commonty handoff v1\0${host}\0${nonce}`))),
  nonce,
});

// The box's other names, all at once and in the background, so that going to
// one finds a session there. Never holds the browser up for long, and a name
// that does not answer (off the network from here) makes its own trip later.
const WARM_MS = 700;
async function warm(me, host) {
  const stop = AbortSignal.timeout(WARM_MS);
  const done = new Set(me.hosts || []);
  const tried = new Set(me.tried || []);
  const r = await fetch(`https://${host}/_dd/sso/names`, { credentials: 'omit', signal: stop });
  const { names } = r.ok ? await r.json() : { names: [] };
  const fresh = names.filter((n) => ours(n.host) && !done.has(n.host) && !tried.has(n.host));
  // once for each sign-in: one out of reach from here should not cost this wait again
  me.tried = [...tried, ...fresh.map((n) => n.host)];
  await Promise.allSettled(fresh.map(async (n) => {
    const body = new URLSearchParams({ ...(await handed(me, n.host, n.nonce)), warm: 'true' });
    await fetch(`https://${n.host}/_dd/sso`, { method: 'POST', mode: 'no-cors', credentials: 'include', keepalive: true, body, signal: stop });
    done.add(n.host);
  }));
  me.hosts = [...done];
}

async function handOff(me, host, nonce) {
  await warm(me, host).catch(() => {});
  // the names it holds a session on, for signing out of them
  me.hosts = [...new Set([...(me.hosts || []), host])];
  await keep(me).catch(() => {});
  const f = document.createElement('form');
  f.method = 'POST';
  f.action = `https://${host}/_dd/sso`;
  for (const [k, v] of Object.entries(await handed(me, host, nonce))) {
    const i = document.createElement('input');
    i.type = 'hidden';
    i.name = k;
    i.value = v;
    f.append(i);
  }
  document.body.append(f);
  f.submit();
}

async function main() {
  const q = new URLSearchParams(location.search);
  const home = `https://home.${DOMAIN}`;
  $('brand').href = `${home}/_dd/home`;
  $('app').href = `${home}/_dd/download`;
  $('join').href = `${home}/_dd/join`;
  $('demo').href = `${home}/_dd/demo`;
  $('home').href = `${home}/`;
  $('again').href = '/';
  try { $('u').value = localStorage.getItem('dd_user') || ''; } catch {}

  if (location.pathname === '/out') {
    const hosts = (await kept())?.hosts || [];
    const stop = AbortSignal.timeout(1000);
    await Promise.allSettled(hosts.map((h) => fetch(`https://${h}/_dd/sso/out`, { method: 'POST', mode: 'no-cors', credentials: 'include', signal: stop })));
    await forget();
    const then = q.get('then') || '';
    let to = null;
    try { const u = new URL(then); if (u.protocol === 'https:' && ours(u.hostname)) to = u.href; } catch {}
    if (to) location.replace(to);
    else show('gone');
    return;
  }

  const host = (q.get('host') || '').toLowerCase();
  const nonce = q.get('nonce') || '';
  const handing = location.pathname === '/go' && ours(host) && /^[A-Za-z0-9_-]{8,64}$/.test(nonce);

  const me = await current();
  // again: the box would not take the sign-in kept here (ended, or a passkey
  // it no longer holds), so it does not get the same one
  if (handing && me && q.get('again') !== '1') {
    show('busy');
    return handOff(me, host, nonce);
  }
  if (me && !handing) {
    $('who').textContent = me.d.user;
    show('done');
  } else {
    show('signin');
  }
  $('out').onclick = async () => { location.href = '/out'; };
  $('go').onclick = async () => {
    const user = $('u').value.trim().toLowerCase();
    if (!user) return say('Your username, please.');
    try { localStorage.setItem('dd_user', user); } catch {}
    say('…');
    try {
      const fresh = await signIn(user, handing ? host : `home.${DOMAIN}`);
      if (handing) { show('busy'); return handOff(fresh, host, nonce); }
      location.href = `${home}/`;
    } catch (e) {
      say(`Sign-in failed: ${e.message}`);
    }
  };
}

main();
