// Photos: a person's ente account, opened by their passkey. The page asks
// the passkey for its PRF secret; our Rust in the browser (dd_web) makes
// or opens the ente account with it and hands ente's app a signed-in
// session. The master key never leaves this tab. The demo has no passkey:
// its password comes with the config, for that session only.

import init, { ente_login, ente_create, ente_adopt } from '/_dd/web/dd_web.js';
import { say } from './webauthn.js';
import { passkeySecret, photosConfig } from './photos-passkey.js';

// no account under this passkey yet: link one made before, or start fresh
function askToLink(cfg, password) {
  return new Promise((resolve, reject) => {
    const panel = document.getElementById('link');
    panel.hidden = false;
    say('');
    document.getElementById('adopt').onclick = async () => {
      try {
        say('Linking…');
        panel.hidden = true;
        const email = document.getElementById('le').value.trim();
        const old = document.getElementById('lp').value;
        resolve(JSON.parse(await ente_adopt(cfg.api, email, old, cfg.email, password, cfg.code)));
      } catch (e) { reject(e); }
    };
    document.getElementById('fresh').onclick = async () => {
      try {
        say('Making your photo account…');
        panel.hidden = true;
        resolve(JSON.parse(await ente_create(cfg.api, cfg.email, password, cfg.code)));
      } catch (e) { reject(e); }
    };
  });
}

// what ente's web app reads to be signed in, in the shapes it keeps
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
  // the gallery itself: the app's front page would draw its own sign-in
  // on the way there
  location.replace('/gallery');
}

// Whatever ente's app kept from someone else: gone, before anything else
// happens. The app trusts its own storage, so a session left behind on a
// shared machine would open the previous person's library to the next,
// and a failed sign-in below must land on nothing, not on it. The same
// person's own list and pictures stay: without them every visit fetched
// and unlocked the whole library again. Signing out clears it all
// (/_dd/photos/forget).
function keptFor() {
  try { return JSON.parse(localStorage.getItem('user') || 'null')?.email || null; } catch { return null; }
}

async function wipe() {
  localStorage.clear();
  sessionStorage.clear();
  const dbs = indexedDB.databases ? await indexedDB.databases() : [{ name: 'kv' }, { name: 'files' }];
  await Promise.all(dbs.map(d => new Promise(resolve => {
    const r = indexedDB.deleteDatabase(d.name);
    r.onsuccess = r.onerror = r.onblocked = () => resolve();
  })));
  if (window.caches) await Promise.all((await caches.keys()).map(k => caches.delete(k)));
}

async function go() {
  try {
    const user = document.querySelector('.card').dataset.user;
    const cfg = await photosConfig();
    if (keptFor() !== cfg.email) {
      await wipe();
    } else if (sessionStorage.getItem('encryptionKey')) {
      // this window opened the library already: straight back in
      location.replace('/gallery');
      return;
    }

    const password = cfg.password || await passkeySecret(user, cfg);
    await init();
    say('Opening your photos…');

    let session;
    try {
      session = JSON.parse(await ente_login(cfg.api, cfg.email, password));
    } catch (err) {
      // the wasm throws an object; its text is in message
      if (!/404|not found|user not/i.test(String((err && err.message) || err))) throw err;
      session = await askToLink(cfg, password);
    }
    await seed(session);
  } catch (err) {
    say('Could not open Photos: ' + ((err && err.message) || err));
  }
}

go();
