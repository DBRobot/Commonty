// Photos in the app's window or the member's browser, from the app's own
// copy of Ente's client (src/local.rs). This page is handed, once, what it
// needs from the app (src/photos.rs, `hand`): the password this device
// keeps, made at random by the app, never by a passkey; and with it, the
// first time, whether to make the account or move one off the password a
// passkey used to make. No address carries any of it, so no history or
// synced history keeps it. It opens the account with our Rust in the page
// (dd_web) straight against museum, seeds what Ente's client reads to be
// signed in, and goes to the gallery. The box serves none of this code; it
// answers museum's calls and keeps the files.

import init, { ente_login, ente_create, ente_rekey } from '/_dd/web/dd_web.js';

const $ = (id) => document.getElementById(id);
const say = (t) => { $('msg').textContent = t; };

function keptFor() {
  try { return JSON.parse(localStorage.getItem('user') || 'null')?.email || null; } catch { return null; }
}

// whatever this origin kept for someone else goes before anything opens
async function wipe() {
  localStorage.clear();
  sessionStorage.clear();
  const dbs = indexedDB.databases ? await indexedDB.databases() : [{ name: 'kv' }, { name: 'files' }];
  await Promise.all(dbs.map((d) => new Promise((resolve) => {
    const r = indexedDB.deleteDatabase(d.name);
    r.onsuccess = r.onerror = r.onblocked = () => resolve();
  })));
  if (window.caches) await Promise.all((await caches.keys()).map((k) => caches.delete(k)));
}

// what Ente's client reads to be signed in, in the shapes it keeps
async function seed(s) {
  localStorage.setItem('user', JSON.stringify({ id: s.userId, email: s.email, token: s.token }));
  localStorage.setItem('keyAttributes', JSON.stringify(s.keyAttributes));
  sessionStorage.setItem('encryptionKey', JSON.stringify(s.sessionKey));
  await new Promise((resolve, reject) => {
    const r = indexedDB.open('kv', 1);
    r.onupgradeneeded = () => { r.result.createObjectStore('kv'); };
    r.onerror = () => reject(r.error);
    r.onsuccess = () => {
      const tx = r.result.transaction('kv', 'readwrite');
      tx.objectStore('kv').put(s.token, 'token');
      tx.oncomplete = () => { r.result.close(); resolve(); };
      tx.onerror = () => reject(tx.error);
    };
  });
  location.replace(next);
}

// the page a link asked for, on this origin only
const to = new URLSearchParams(location.search).get('to');
const next = to && /^\/(?![/\\])/.test(to) ? to : '/gallery';

const post = (path, body) => fetch(path, {
  method: 'POST',
  headers: { 'content-type': 'application/json' },
  body: JSON.stringify(body || {}),
});

// what the app hands this page, once for the door it came through
async function hand() {
  const r = await post('/_dd/app/photos-key');
  return r.ok ? r.json() : null;
}

// the password this page was handed is the account's now
async function keyed(pw) {
  const r = await post('/_dd/app/photos-keyed', { pw });
  if (!r.ok) throw new Error(`the app said: ${await r.text()}`);
}

const message = (err) => String((err && err.message) || err);
const missing = (err) => /404|not found|user not/i.test(message(err));

// An account a passkey made, and no device here holds its old password:
// the app asks the member's browser for it once, then this page is let in
// again and moves it.
function offerMove() {
  say('');
  $('move').hidden = false;
  $('go').onclick = async () => {
    $('go').disabled = true;
    try {
      const r = await post('/_dd/app/photos-move');
      if (!r.ok) throw new Error(await r.text());
      $('move').hidden = true;
      $('wait').hidden = false;
      for (;;) {
        await new Promise((ok) => setTimeout(ok, 2000));
        const s = await (await fetch('/_dd/app/photos-move')).json();
        if (s.ready) break;
        if (!s.waiting) throw new Error('the move did not finish; try again');
      }
      const d = await post('/_dd/app/door', { service: 'photos', path: next });
      if (!d.ok) throw new Error(await d.text());
      location.replace((await d.json()).go);
    } catch (e) {
      $('wait').hidden = true;
      $('move').hidden = false;
      say(`Could not move Photos: ${message(e)}`);
    } finally {
      $('go').disabled = false;
    }
  };
}

async function go() {
  const h = new URLSearchParams(location.hash.slice(1));
  // where the app's pages are, for the bar to go back to (shell.js): only
  // the app's own two addresses
  if (['commonty://localhost', 'http://commonty.localhost'].includes(h.get('app'))) sessionStorage.setItem('dd-app-origin', h.get('app'));
  if (location.hash || location.search) history.replaceState(null, '', location.pathname);
  const r = await post('/_dd/photos/config');
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  const cfg = await r.json();
  if (keptFor() !== cfg.email) {
    await wipe();
  } else if (sessionStorage.getItem('encryptionKey')) {
    location.replace(next);
    return;
  }
  const given = await hand();
  if (!given) throw new Error('open Photos from the Commonty app');
  await init();
  say('Opening your photos…');
  if (given.next) {
    // the account a passkey made, moved to the app's password before
    // anything else; the old one opens nothing after this
    const session = JSON.parse(await ente_rekey(cfg.api, cfg.email, given.pw, given.next));
    await keyed(given.next);
    return seed(session);
  }
  let session;
  try {
    session = JSON.parse(await ente_login(cfg.api, cfg.email, given.pw));
  } catch (err) {
    if (!given.fresh) throw err;
    if (!missing(err)) return offerMove();
    say('Making your photo account…');
    session = JSON.parse(await ente_create(location.origin + cfg.accountApi, cfg.email, given.pw, ''));
  }
  if (given.fresh) await keyed(given.pw);
  await seed(session);
}

go().catch((err) => say(`Could not open Photos: ${message(err)}`));
