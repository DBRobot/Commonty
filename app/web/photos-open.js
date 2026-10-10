// Photos in the app's own window, from the app's own copy of Ente's client
// (src/local.rs): the app put this device's photo password after the # of
// this page's address, the part no server is sent. It opens the account
// with our Rust in the page (dd_web) straight against museum, seeds what
// Ente's client reads to be signed in, and goes to the gallery. The box
// serves none of this code; it answers museum's calls and keeps the files.

import init, { ente_login } from '/_dd/web/dd_web.js';

const say = (t) => { document.getElementById('msg').textContent = t; };

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
  location.replace('/gallery');
}

async function go() {
  const h = new URLSearchParams(location.hash.slice(1));
  const password = h.get('pw');
  // where the app's pages are, for the bar to go back to (shell.js): only
  // the app's own two addresses
  if (['commonty://localhost', 'http://commonty.localhost'].includes(h.get('app'))) sessionStorage.setItem('dd-app-origin', h.get('app'));
  if (location.hash) history.replaceState(null, '', location.pathname);
  const r = await fetch('/_dd/photos/config', { method: 'POST' });
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  const cfg = await r.json();
  if (keptFor() !== cfg.email) {
    await wipe();
  } else if (sessionStorage.getItem('encryptionKey')) {
    location.replace('/gallery');
    return;
  }
  if (!password) throw new Error('open Photos from the Commonty app');
  await init();
  say('Opening your photos…');
  await seed(JSON.parse(await ente_login(cfg.api, cfg.email, password)));
}

go().catch((err) => say('Could not open Photos: ' + ((err && err.message) || err)));
