// Backups: the disks a member has saved with `dd image push`. Each is a
// snapshot in a restic archive in their own folder on the box, encrypted
// on the machine that made it. The archive's password is sealed to their
// passkeys beside it (dd-passkeys.json, `dd image passkey`), so this page
// opens it here: the box only ever hands over ciphertext.

import init, { Archive } from '/_dd/web/dd_web.js';
import { passkeySecret, human } from './library.js';
import { requestOptions, assertion, post } from './webauthn.js';
import { inApp, me, pageConfig, entryOf } from './shell.js';

const $ = (id) => document.getElementById(id);

// The archives are on the files site, and the Backups tab reads them from
// there: a page that reached the public side through the tunnel may not ask
// the private network for anything, so Settings moves there for this tab.
export const FILES = 'files.' + location.hostname.split('.').slice(-2).join('.');
const ROOT = '/images/';

function say(text, html) {
  $('msg').hidden = false;
  $('list').hidden = true;
  if (html) $('msg').innerHTML = html;
  else $('msg').textContent = text;
}

async function listing(dir) {
  const r = await fetch(ROOT + dir);
  if (r.status === 404) return [];
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  return r.json();
}

async function bytes(path, from, to) {
  const headers = to === undefined ? {} : { range: `bytes=${from}-${to}` };
  const r = await fetch(ROOT + path, { headers, signal: bytes.signal });
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

async function must(r) {
  if (!r.ok && r.status !== 404) throw new Error(`the box said ${r.status}`);
}

const json = (b) => JSON.parse(new TextDecoder().decode(b));

// what this page knows of the archive once it is open
let arch = null;
let packPath = (id) => `data/${id}`;
let blobs = new Map(); // blob id -> { pack, offset, length, ulen }

async function readIndex() {
  const files = (await listing('index/')).filter((e) => e.type === 'file');
  const all = await Promise.all(files.map(async (f) => ({ id: f.name, index: json(arch.file(await bytes('index/' + f.name))) })));
  blobs = new Map();
  for (const { index } of all) {
    for (const p of index.packs || []) {
      for (const b of p.blobs) blobs.set(b.id, { pack: p.id, offset: b.offset, length: b.length, ulen: b.uncompressed_length });
    }
  }
  return all;
}

async function readBlob(id) {
  const b = blobs.get(id);
  if (!b) throw new Error('a piece of this image is missing from the archive');
  const sealed = await bytes(packPath(b.pack), b.offset, b.offset + b.length - 1);
  return arch.blob(sealed, b.ulen !== undefined);
}

async function readSnapshots() {
  const files = (await listing('snapshots/')).filter((e) => e.type === 'file');
  const snaps = await Promise.all(files.map(async (f) => ({ id: f.name, ...json(arch.file(await bytes('snapshots/' + f.name))) })));
  return snaps.sort((a, b) => Date.parse(b.time) - Date.parse(a.time));
}

async function open(user) {
  await init();
  const top = await listing('');
  if (!top.some((e) => e.name === 'config')) return false;
  const sealedText = await (await fetch(ROOT + 'dd-passkeys.json')).text().catch(() => '');
  let sealed = {};
  try { sealed = JSON.parse(sealedText); } catch { /* none yet */ }
  const [cfg, entry] = await Promise.all([pageConfig(), entryOf(user)]);
  const passkeys = (entry.entry.passkeys || []).filter((p) => sealed[p.id]);
  if (!passkeys.length) {
    say('This browser cannot open your images yet.');
    return null;
  }
  const { secret, id } = await passkeySecret(cfg, passkeys);
  const keyFile = (await listing('keys/')).find((e) => e.type === 'file');
  arch = new Archive(sealedText, id, secret, new TextDecoder().decode(await bytes('keys/' + keyFile.name)));
  // restic files packs under two hex characters of their name; ours are flat
  const data = await listing('data/');
  if (data.some((e) => e.type === 'directory')) packPath = (p) => `data/${p.slice(0, 2)}/${p}`;
  return data;
}

function saved(s) {
  return 'saved ' + new Date(s.time).toLocaleString(undefined, { day: 'numeric', month: 'short', year: 'numeric', hour: 'numeric', minute: '2-digit' });
}

const size = (s) => s.summary?.total_bytes_processed;
const DISK = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><rect x="3" y="5" width="18" height="14" rx="2"/><path d="M3 15h18"/><circle cx="17" cy="17" r=".9" fill="currentColor"/></svg>';

let snaps = [];
let indexed = Promise.resolve();
let onBox = 0;

function total() {
  $('total').textContent = snaps.length
    ? `${snaps.length} ${snaps.length === 1 ? 'image' : 'images'} · ${human(onBox)} on the box`
    : '';
}

function render() {
  total();
  if (!snaps.length) {
    say('No disk images yet.');
    return;
  }
  $('msg').hidden = true;
  $('list').hidden = false;
  $('list').replaceChildren(...snaps.map(row));
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function row(s) {
  const li = el('li', 'bk-item');
  const icon = el('span', 'bk-icon');
  icon.innerHTML = DISK;
  const text = el('div');
  const meta = el('div', 'bk-meta');
  const rest = () => {
    meta.className = 'bk-meta';
    meta.textContent = [size(s) !== undefined ? human(size(s)) : null, saved(s)].filter(Boolean).join(' · ');
  };
  rest();
  text.append(el('div', 'bk-name', s.label || s.id.slice(0, 8)), meta);
  const actions = el('div', 'bk-actions');
  const down = el('button', '', 'Download');
  const del = el('button', 'danger', 'Delete');
  actions.append(down, del);
  li.append(icon, text, actions);
  const ui = { li, icon, meta, actions, down, del, rest };
  down.onclick = () => download(s, ui).catch((e) => failRow(ui, e));
  del.onclick = () => ask(s, ui);
  return li;
}

function failRow(ui, e) {
  ui.icon.className = 'bk-icon';
  ui.icon.innerHTML = DISK;
  ui.actions.replaceChildren(ui.down, ui.del);
  ui.meta.className = 'bk-meta bad';
  ui.meta.textContent = String(e.message || e);
}

function ring(ui, frac) {
  const pct = Math.floor(frac * 100);
  if (!ui.icon.classList.contains('bk-ring')) {
    ui.icon.className = 'bk-icon bk-ring';
    ui.icon.setAttribute('role', 'progressbar');
    ui.icon.setAttribute('aria-valuemin', '0');
    ui.icon.setAttribute('aria-valuemax', '100');
    ui.icon.innerHTML = '<svg viewBox="0 0 40 40"><circle class="track" cx="20" cy="20" r="17"/><circle class="fill" cx="20" cy="20" r="17" pathLength="100" stroke-dasharray="0 100"/></svg><b>0%</b>';
  }
  ui.icon.setAttribute('aria-valuenow', String(pct));
  ui.icon.setAttribute('aria-label', `Downloading, ${pct}%`);
  ui.icon.querySelector('.fill').setAttribute('stroke-dasharray', `${pct} 100`);
  ui.icon.querySelector('b').textContent = `${pct}%`;
}

function left(secs) {
  if (!isFinite(secs)) return '';
  if (secs < 90) return ' · under 2 min left';
  if (secs < 5400) return ` · about ${Math.round(secs / 60)} min left`;
  return ` · about ${(secs / 3600).toFixed(1)} h left`;
}

// Pieces next to each other in one pack are fetched as one range, a few
// ranges at a time, and written to the file in order.
const RUN = 32 * 1024 * 1024;
const AHEAD = 4;

function runs(content) {
  const out = [];
  let cur = null;
  for (const id of content) {
    const b = blobs.get(id);
    if (!b) throw new Error('a piece of this image is missing from the archive');
    if (cur && cur.pack === b.pack && cur.end === b.offset && cur.end - cur.start + b.length <= RUN) {
      cur.parts.push(b);
      cur.end += b.length;
    } else {
      cur = { pack: b.pack, start: b.offset, end: b.offset + b.length, parts: [b] };
      out.push(cur);
    }
  }
  return out;
}

async function fetchRun(r) {
  const buf = await bytes(packPath(r.pack), r.start, r.end - 1);
  return r.parts.map((b) => arch.blob(buf.subarray(b.offset - r.start, b.offset - r.start + b.length), b.ulen !== undefined));
}

async function download(s, ui) {
  await indexed;
  const tree = json(await readBlob(s.tree));
  const files = (tree.nodes || []).filter((n) => n.type === 'file');
  if (files.length !== 1 || tree.nodes.length !== 1) throw new Error('this archive holds more than one file');
  const node = files[0];
  let sink = null;
  if (window.showSaveFilePicker) {
    try {
      sink = await (await window.showSaveFilePicker({ suggestedName: node.name })).createWritable();
    } catch (e) {
      if (e.name === 'AbortError') return;
      throw e;
    }
  } else if (node.size > 2e9) {
    throw new Error('this browser cannot save a file this large; Chrome or Edge can');
  }
  const stop = new AbortController();
  bytes.signal = stop.signal;
  const cancel = el('button', 'quiet', 'Cancel');
  cancel.onclick = () => stop.abort();
  ui.actions.replaceChildren(cancel);
  ring(ui, 0);
  const pieces = [];
  const started = performance.now();
  let done = 0;
  const pending = [];
  try {
    const list = runs(node.content || []);
    for (let i = 0; i < list.length; i++) {
      while (pending.length < AHEAD && i + pending.length < list.length) pending.push(fetchRun(list[i + pending.length]));
      for (const plain of await pending.shift()) {
        if (sink) await sink.write(plain);
        else pieces.push(new Blob([plain]));
        done += plain.length;
      }
      const secs = (performance.now() - started) / 1000;
      ring(ui, node.size ? done / node.size : 1);
      ui.meta.className = 'bk-meta';
      ui.meta.textContent = `Downloading · ${human(done)} of ${human(node.size)}` + (secs > 5 ? left((node.size - done) / (done / secs)) : '');
    }
    if (sink) await sink.close();
    else {
      const url = URL.createObjectURL(new Blob(pieces));
      const a = document.createElement('a');
      a.href = url;
      a.download = node.name;
      a.click();
      URL.revokeObjectURL(url);
    }
    ui.icon.className = 'bk-icon';
    ui.icon.innerHTML = DISK;
    ui.actions.replaceChildren(ui.down, ui.del);
    ui.rest();
  } catch (e) {
    for (const p of pending) p.catch(() => {});
    if (sink) await sink.abort().catch(() => {});
    if (stop.signal.aborted) {
      failRow(ui, '');
      ui.rest();
      return;
    }
    throw e;
  } finally {
    bytes.signal = undefined;
  }
}

// The delete: the passkey first (the box refuses a DELETE under /images/
// from a browser that has not shown one in five minutes), then the
// snapshot, then every pack nothing left uses, and the index without them.

function ask(s, ui) {
  $('ask-h').textContent = `Delete ${s.label || s.id.slice(0, 8)}?`;
  $('ask-size').textContent = size(s) !== undefined
    ? `This removes the ${human(size(s))} image from Commonty for good.`
    : 'This removes the image from Commonty for good.';
  $('ask-msg').hidden = true;
  $('ask-yes').disabled = false;
  $('ask-no').onclick = () => $('ask').close();
  $('ask-yes').onclick = async () => {
    $('ask-yes').disabled = true;
    try {
      const who = await me();
      const start = await post('/_dd/login/start', { username: who.user });
      const { publicKey, ceremony } = await start.json();
      const cred = await navigator.credentials.get({ publicKey: requestOptions(publicKey) });
      await post('/_dd/login/finish', assertion(cred), { 'x-dd-ceremony': ceremony });
    } catch (e) {
      $('ask-msg').textContent = 'Your passkey was not confirmed: ' + (e.message || e);
      $('ask-msg').hidden = false;
      $('ask-yes').disabled = false;
      return;
    }
    $('ask').close();
    ui.actions.replaceChildren();
    remove(s, ui).catch((e) => failRow(ui, e));
  };
  $('ask').showModal();
}

const now = () => Math.floor(Date.now() / 1000);

async function current(name) {
  const r = await fetch(ROOT + name, { cache: 'no-store' });
  if (r.status === 404) return false;
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  return ((await r.json()).until || 0) > now();
}

const markBusy = () => fetch(ROOT + 'dd-deleting', { method: 'PUT', body: JSON.stringify({ until: now() + 300 }) });

// what a tree and everything under it use, trees included
async function used(tree, into) {
  if (into.has(tree)) return;
  into.add(tree);
  for (const n of json(await readBlob(tree)).nodes || []) {
    for (const c of n.content || []) into.add(c);
    if (n.subtree) await used(n.subtree, into);
  }
}

async function inParallel(items, n, each) {
  let i = 0;
  await Promise.all(Array.from({ length: n }, async () => {
    while (i < items.length) await each(items[i++]);
  }));
}

async function remove(s, ui) {
  const busy = 'A push to your images is running. Delete this when it is done.';
  if (await current('dd-pushing')) throw new Error(busy);
  await must(await markBusy());
  // both may have looked at once: this side backs off
  if (await current('dd-pushing')) {
    await fetch(ROOT + 'dd-deleting', { method: 'DELETE' });
    throw new Error(busy);
  }
  const beat = setInterval(markBusy, 60000);
  try {
    ui.meta.className = 'bk-meta';
    ui.meta.textContent = 'Deleting…';
    const r = await fetch(ROOT + 'snapshots/' + s.id, { method: 'DELETE' });
    if (r.status === 403) throw new Error('the box wants your passkey again; delete once more');
    await must(r);
    snaps = snaps.filter((x) => x.id !== s.id);
    ui.li.remove();
    if (!snaps.length) render();

    // read again: what is on the box now is what counts
    const indexes = await readIndex();
    const keep = new Set();
    for (const left of await readSnapshots()) await used(left.tree, keep);
    const kept = new Map();
    const gone = new Set();
    for (const { index } of indexes) {
      for (const p of index.packs || []) {
        if (p.blobs.some((b) => keep.has(b.id))) kept.set(p.id, p);
        else gone.add(p.id);
      }
      for (const p of index.packs_to_delete || []) gone.add(p.id);
    }
    const keptIds = new Set(kept.keys());
    // and packs no index names: left by a delete that stopped half way
    const data = await listing('data/');
    const onDisk = data.some((e) => e.type === 'directory')
      ? (await Promise.all(data.filter((e) => e.type === 'directory').map((d) => listing('data/' + d.name + '/')))).flat()
      : data;
    for (const f of onDisk) if (f.type === 'file' && !keptIds.has(f.name)) gone.add(f.name);
    for (const id of keptIds) gone.delete(id);

    if (kept.size) {
      const stored = arch.seal_file(new TextEncoder().encode(JSON.stringify({ packs: [...kept.values()] })));
      await must(await fetch(ROOT + 'index/' + Archive.id_of(stored), { method: 'PUT', body: stored }));
    }
    let freed = 0;
    const sizes = new Map(onDisk.map((f) => [f.name, f.size || 0]));
    const del = async (path) => {
      const r = await fetch(ROOT + path, { method: 'DELETE' });
      if (r.status === 403) throw new Error('the box wants your passkey again; delete another image or this page again to finish');
      await must(r);
    };
    await inParallel(indexes.map((i) => 'index/' + i.id), 4, del);
    const packs = [...gone];
    let n = 0;
    await inParallel(packs, 8, async (id) => {
      await del(packPath(id));
      freed += sizes.get(id) || 0;
      n++;
      if (snaps.length) $('total').textContent = `Freeing space · ${n} of ${packs.length}`;
    });
    onBox = Math.max(0, onBox - freed);
    total();
  } finally {
    clearInterval(beat);
    await fetch(ROOT + 'dd-deleting', { method: 'DELETE' }).catch(() => {});
  }
}

let started = false;

/// the Backups tab, the first time it is shown
export async function start() {
  if (started) return;
  started = true;
  try {
    await show();
  } catch (e) {
    say(String(e.message || e));
  }
}

async function show() {
  if (inApp) {
    say('', `Your disk images open in a browser, with your passkey: <a href="https://${FILES}/_dd/settings#backups">${FILES}/_dd/settings</a>`);
    return;
  }
  const who = await me();
  const data = await open(who.user);
  if (data === null) return;
  if (data === false) {
    say('No disk images yet.');
    return;
  }
  onBox = data.reduce((n, e) => n + (e.size || 0), 0);
  if (data.some((e) => e.type === 'directory')) {
    const inner = await Promise.all(data.filter((e) => e.type === 'directory').map((d) => listing('data/' + d.name + '/')));
    onBox = inner.flat().reduce((n, e) => n + (e.size || 0), 0);
  }
  // the list needs only the snapshots; the index is for opening one
  indexed = readIndex();
  indexed.catch(() => {});
  snaps = await readSnapshots();
  render();
}

