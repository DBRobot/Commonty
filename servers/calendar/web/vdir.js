// Calendars and address books in a member's libraries: a folder per
// calendar under Calendars/ and per address book under Contacts/, one .ics
// or .vcf file per event or person, and `displayname` and `color` files
// beside them - the layout vdirsyncer, khal and khard read, so a library
// mounted with rclone opens in them as it is. Their own library first, then
// any another member shares with them. What was read is kept on this
// device (IndexedDB) by version, so a page draws at once and asks the gate
// only for what changed.

import { libraries, list, readText, saveText, trashFrom, mkdir } from './library.js';
import { inApp, appFetch } from './shell.js';

// one store per account, so another account signed in on this device
// never draws what this one kept
let account = '';
export function forAccount(user) {
  if (user !== account) { account = user || ''; dbp = null; }
}
let dbp = null;
function db() {
  dbp ||= new Promise((ok) => {
    if (!account) { ok(null); return; }
    try {
      const r = indexedDB.open(`dd-vdir-${account}`, 1);
      r.onupgradeneeded = () => r.result.createObjectStore('files');
      r.onsuccess = () => ok(r.result);
      r.onerror = () => ok(null);
    } catch { ok(null); }
  });
  return dbp;
}
async function tx(mode, f) {
  const d = await db();
  if (!d) return null;
  return new Promise((ok) => {
    try {
      const t = d.transaction('files', mode);
      const out = f(t.objectStore('files'));
      t.oncomplete = () => ok(out?.result ?? null);
      t.onerror = () => ok(null);
    } catch { ok(null); }
  });
}
const keptGet = (k) => tx('readonly', (s) => s.get(k));
const keptPut = (k, v) => tx('readwrite', (s) => s.put(v, k));
const keptDel = (k) => tx('readwrite', (s) => s.delete(k));

/// everything kept under a prefix, by key
async function keptAll(prefix) {
  const d = await db();
  if (!d) return new Map();
  return new Promise((ok) => {
    const out = new Map();
    try {
      const t = d.transaction('files', 'readonly');
      const range = IDBKeyRange.bound(prefix, prefix + '￿');
      const c = t.objectStore('files').openCursor(range);
      c.onsuccess = () => {
        const cur = c.result;
        if (!cur) return;
        out.set(cur.key, cur.value);
        cur.continue();
      };
      t.oncomplete = () => ok(out);
      t.onerror = () => ok(out);
    } catch { ok(out); }
  });
}

const key = (lib, path) => `${lib.id}/${path}`;

/// The calendars or address books (`root`: 'Calendars' or 'Contacts') in
/// every library this viewer opens: [{ lib, root, path, slug, name, color,
/// owner, writable, mine }]. `draw(list)` gets what was kept first, then
/// what the gate says.
export async function collections(root, draw) {
  const keptList = await keptGet(`collections:${root}`);
  if (keptList && draw) draw(revive(keptList, null));
  const libs = await libraries();
  const out = [];
  await Promise.all(libs.map(async (lib) => {
    let folders = [];
    try { folders = (await list(lib, root)).filter((x) => x.dir); } catch { return; }
    await Promise.all(folders.map(async (f) => {
      const meta = async (n) => {
        try { return (await readText(lib, `${f.path}/${n}`))?.text.trim() || ''; } catch { return ''; }
      };
      const [name, raw] = await Promise.all([meta('displayname'), meta('color')]);
      // another member writes a shared calendar's colour: a colour, or none
      const color = /^#[0-9a-fA-F]{3,8}$/.test(raw) ? raw : '';
      const mine = !lib.reader && !lib.writer;
      out.push({ lib, root, path: f.path, slug: f.name, name: name || f.name, color, owner: lib.owner || '', mine, writable: !lib.reader, readers: lib.readers || [] });
    }));
  }));
  out.sort((a, b) => (a.mine === b.mine ? a.name.localeCompare(b.name) : a.mine ? -1 : 1));
  keptPut(`collections:${root}`, out.map((c) => ({ ...c, lib: c.lib.id })));
  return { list: out, libs };
}

// the kept list names libraries by id; their keys come with the fresh one
function revive(kept, libs) {
  return kept.map((c) => ({ ...c, lib: libs?.find((l) => l.id === c.lib) || { id: c.lib } }));
}

/// The items of a collection, as text, by file name: what was kept at
/// once (`draw`), then the gate's listing, fetching only the files whose
/// version changed, eight at a time. [{ name, path, etag, text }]
export async function items(coll, ext, draw) {
  const prefix = key(coll.lib, coll.path + '/');
  const kept = await keptAll(prefix);
  const keptItems = [...kept.entries()].map(([k, v]) => ({ name: k.slice(prefix.length), path: `${coll.path}/${k.slice(prefix.length)}`, ...v }));
  if (draw && keptItems.length) draw(keptItems);
  if (!coll.lib.key) return keptItems;
  const listed = (await list(coll.lib, coll.path)).filter((x) => !x.dir && x.name.toLowerCase().endsWith(ext));
  const out = [];
  const todo = [];
  for (const f of listed) {
    const k = key(coll.lib, f.path);
    const was = kept.get(k);
    if (was && f.etag && was.etag === f.etag) out.push({ name: f.name, path: f.path, ...was });
    else todo.push(f);
  }
  const workers = Array.from({ length: Math.min(8, todo.length) }, async () => {
    while (todo.length) {
      const f = todo.shift();
      try {
        const got = await readText(coll.lib, f.path);
        if (!got) continue;
        const v = { etag: got.etag || f.etag, text: got.text };
        keptPut(key(coll.lib, f.path), v);
        out.push({ name: f.name, path: f.path, ...v });
      } catch { /* left for next time */ }
    }
  });
  await Promise.all(workers);
  // gone from the gate: gone from here
  const names = new Set(listed.map((f) => key(coll.lib, f.path)));
  for (const k of kept.keys()) if (!names.has(k)) keptDel(k);
  return out;
}

/// An item saved from the version it was read at (`etag`, or 'new'). The
/// new version, or { changed: { etag, text } | null } when another device
/// saved first: what is there now, for the page to show.
export async function save(coll, name, text, etag) {
  const path = `${coll.path}/${name}`;
  const r = await saveText(coll.lib, path, text, etag);
  if (r.changed) {
    const now = await readText(coll.lib, path);
    if (now) keptPut(key(coll.lib, path), now);
    return { changed: now };
  }
  const v = { etag: r.etag, text };
  keptPut(key(coll.lib, path), v);
  return { saved: { name, path, ...v } };
}

/// An item to the trash, from its version; false when it changed first.
export async function remove(coll, name, etag) {
  const path = `${coll.path}/${name}`;
  const ok = await trashFrom(coll.lib, path, etag);
  if (ok) keptDel(key(coll.lib, path));
  return ok;
}

/// A new calendar or address book in the viewer's own library.
export async function make(lib, root, name, color) {
  const slug = (name.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/^-|-$/g, '') || 'list') + '-' + Math.random().toString(36).slice(2, 6);
  const path = `${root}/${slug}`;
  await mkdir(lib, root);
  await mkdir(lib, path);
  await saveText(lib, `${path}/displayname`, name + '\n', null);
  if (color) await saveText(lib, `${path}/color`, color + '\n', null);
  return { lib, root, path, slug, name, color, owner: lib.owner || '', mine: true, writable: true };
}

/// A calendar's or address book's name or colour changed.
export async function rename(coll, name, color) {
  await saveText(coll.lib, `${coll.path}/displayname`, name + '\n', null);
  if (color !== undefined) await saveText(coll.lib, `${coll.path}/color`, color + '\n', null);
}

/// The own library's first calendar or address book, made if it has none.
export async function first(libs, root, list, name) {
  const own = libs.find((l) => !l.reader && !l.writer);
  if (!own) return null;
  const mine = list.find((c) => c.lib.id === own.id);
  if (mine) return mine;
  const slug = root === 'Calendars' ? 'personal' : 'contacts';
  const path = `${root}/${slug}`;
  await mkdir(own, root);
  await mkdir(own, path);
  await saveText(own, `${path}/displayname`, name + '\n', null);
  return { lib: own, root, path, slug, name, color: '', owner: own.owner || '', mine: true, writable: true };
}

/// A file name for a new item: its UID where that is a plain name, else
/// a fresh one, as CalDAV programs name them.
export function fileName(uid, ext) {
  return /^[A-Za-z0-9@._-]{1,120}$/.test(uid) ? `${uid}${ext}` : `${crypto.randomUUID?.() || Date.now()}${ext}`;
}

/// The Other apps button, in the app's window: where Thunderbird, GNOME's
/// and Apple's calendars and contacts find these on this computer, and the
/// password they sign in with (app/src/dav.rs). `which`: 'calendars' or
/// 'contacts'.
export function otherApps(button, dialog, which) {
  if (!inApp) return;
  button.hidden = false;
  button.addEventListener('click', async () => {
    const h = dialog.querySelector('h2');
    const body = dialog.querySelector('[data-body]');
    const acts = dialog.querySelector('[data-actions]');
    h.textContent = which === 'calendars' ? 'Your calendars in other apps' : 'Your contacts in other apps';
    body.replaceChildren();
    acts.replaceChildren();
    let d;
    try {
      const r = await appFetch('/_dd/app/dav');
      d = await r.json();
    } catch (e) { d = { error: e.message }; }
    const row = (label, value) => {
      const v = document.createElement('code');
      v.textContent = value;
      const b = document.createElement('button');
      b.type = 'button';
      b.className = 'quiet small';
      b.textContent = 'Copy';
      b.addEventListener('click', async () => {
        try { await navigator.clipboard.writeText(value); b.textContent = 'Copied'; } catch { b.textContent = 'Select it'; }
        setTimeout(() => { b.textContent = 'Copy'; }, 1600);
      });
      const l = document.createElement('div');
      l.className = 'dav-row';
      const t = document.createElement('span');
      t.textContent = label;
      l.append(t, v, b);
      return l;
    };
    if (d.error) {
      const p = document.createElement('p');
      p.textContent = `Not available on this computer: ${d.error}`;
      body.append(p);
    } else {
      const p = document.createElement('p');
      p.textContent = which === 'calendars'
        ? 'In your calendar app, add a CalDAV account with these. It works while Commonty runs on this computer.'
        : 'In your contacts app, add a CardDAV account with these. It works while Commonty runs on this computer.';
      body.append(p, row('Address', which === 'calendars' ? d.calendars : d.contacts), row('Name', d.user), row('Password', d.password));
    }
    const close = document.createElement('button');
    close.type = 'button';
    close.textContent = 'Done';
    close.addEventListener('click', () => dialog.close());
    acts.append(close);
    dialog.onclose = null;
    dialog.showModal();
  });
}
