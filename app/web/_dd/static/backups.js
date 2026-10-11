// Backups: the disks a member has saved with `dd image push`. Each is a
// snapshot in a restic archive in their own folder on the box, encrypted
// on the machine that made it. The archive's password is sealed to their
// devices beside it (dd-devices.json), and the member's app opens it with
// this device's key and hands it to this page: the box only ever hands
// over ciphertext. So the tab works where the app serves Settings - its
// window, or a browser it let in - and nowhere else.

import init, { Archive } from '/_dd/web/dd_web.js';
import { human } from './library.js';
import { inApp, servedByApp, appFetch, me } from './shell.js';

const $ = (id) => document.getElementById(id);

const ROOT = '/_dd/images/';

// Where the list is drawn: Backups' own tab, and the Leave tab's "Take your
// things with you", which lists the same images to download and none to
// delete. One archive, opened once, behind both.
const views = [];
const tab = () => ({ msg: $('msg'), list: $('list'), total: $('total'), delete: true });

function say(text, html) {
  for (const v of views) {
    v.msg.hidden = false;
    v.list.hidden = true;
    if (html) v.msg.innerHTML = html;
    else v.msg.textContent = text;
  }
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

// six at a time, as a browser would fetch a page's own files: an archive's
// hundreds of index files do not all go out at once
async function each(items, fn, n = 6) {
  const out = new Array(items.length);
  let next = 0;
  const worker = async () => {
    while (next < items.length) {
      const i = next++;
      out[i] = await fn(items[i]);
    }
  };
  await Promise.all(Array.from({ length: Math.min(n, items.length) }, worker));
  return out;
}

async function readIndex() {
  const files = (await listing('index/')).filter((e) => e.type === 'file');
  const all = await each(files, async (f) => ({ id: f.name, index: json(arch.file(await bytes('index/' + f.name))) }));
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
  const snaps = await each(files, async (f) => ({ id: f.name, ...json(arch.file(await bytes('snapshots/' + f.name))) }));
  return snaps.sort((a, b) => Date.parse(b.time) - Date.parse(a.time));
}

async function open() {
  await init();
  const top = await listing('');
  if (!top.some((e) => e.name === 'config')) return false;
  const r = inApp ? await appFetch('/_dd/app/archive') : await fetch('/_dd/app/archive');
  if (r.status === 404) {
    say('Your disk images do not open on this device yet. Save one again from the computer that made them, and they will.');
    return null;
  }
  if (!r.ok) throw new Error(`the app said ${r.status}: ${await r.text()}`);
  const { password } = await r.json();
  const keyFile = (await listing('keys/')).find((e) => e.type === 'file');
  arch = new Archive(password, new TextDecoder().decode(await bytes('keys/' + keyFile.name)));
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
  for (const v of views) {
    if (!v.total) continue;
    v.total.textContent = snaps.length
      ? `${snaps.length} ${snaps.length === 1 ? 'image' : 'images'} · ${human(onBox)} on the box`
      : '';
  }
}

function render() {
  total();
  if (!snaps.length) {
    say('No disk images yet.');
    return;
  }
  for (const v of views) draw(v);
}

function draw(v) {
  v.msg.hidden = true;
  v.list.hidden = false;
  v.list.replaceChildren(...snaps.map((s) => row(s, v.delete)));
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function row(s, deletable = true) {
  const li = el('li', 'bk-item');
  li.dataset.id = s.id;
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
  const name = s.label || s.id.slice(0, 8);
  const down = el('button', '', 'Download');
  down.setAttribute('aria-label', `Download ${name}`);
  const del = deletable ? el('button', 'danger', 'Delete') : null;
  del?.setAttribute('aria-label', `Delete ${name}`);
  actions.append(...[down, del].filter(Boolean));
  li.append(icon, text, actions);
  const ui = { li, icon, meta, actions, down, del, rest };
  down.onclick = () => download(s, ui).catch((e) => failRow(ui, e));
  if (del) del.onclick = () => ask(s, ui);
  return li;
}

function failRow(ui, e) {
  ui.icon.className = 'bk-icon';
  ui.icon.innerHTML = DISK;
  ui.actions.replaceChildren(...[ui.down, ui.del].filter(Boolean));
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
    ui.actions.replaceChildren(...[ui.down, ui.del].filter(Boolean));
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

// The delete: the snapshot, then every pack nothing left uses, and the
// index without them. The app sends it with this device's token, which
// the box takes without a passkey shown again.

function ask(s, ui) {
  $('ask-h').textContent = `Delete ${s.label || s.id.slice(0, 8)}?`;
  $('ask-size').textContent = size(s) !== undefined
    ? `The ${human(size(s))} image goes to the trash, and is gone for good 30 days later, or now if you tick the box.`
    : 'The image goes to the trash, and is gone for good 30 days later, or now if you tick the box.';
  $('ask-msg').hidden = true;
  $('ask-yes').disabled = false;
  $('ask-forever').checked = false;
  $('ask-no').onclick = () => $('ask').close();
  $('ask-yes').onclick = () => {
    const forever = $('ask-forever').checked;
    $('ask-yes').disabled = true;
    $('ask').close();
    ui.actions.replaceChildren();
    remove(s, ui)
      .then(() => (forever ? emptyTrash() : null))
      .catch((e) => failRow(ui, e));
  };
  $('ask').showModal();
}

// the images' trash, for good and at once: a DELETE of trash itself
async function emptyTrash() {
  const r = await fetch(ROOT + 'trash', { method: 'DELETE' });
  if (r.status === 403) throw new Error('the box refused; open this page again from the app');
  if (!r.ok) throw new Error(`the box said ${r.status}`);
}

function askEmpty() {
  const d = $('bk-empty-ask');
  $('bk-empty-msg').hidden = true;
  $('bk-empty-yes').disabled = false;
  $('bk-empty-no').onclick = () => d.close();
  $('bk-empty-yes').onclick = async () => {
    $('bk-empty-yes').disabled = true;
    try {
      await emptyTrash();
      d.close();
      $('bk-empty').textContent = 'Emptied';
      $('bk-empty').disabled = true;
    } catch (e) {
      $('bk-empty-msg').textContent = String(e.message || e);
      $('bk-empty-msg').hidden = false;
      $('bk-empty-yes').disabled = false;
    }
  };
  d.showModal();
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
    await must(r);
    snaps = snaps.filter((x) => x.id !== s.id);
    for (const v of views) v.list.querySelector(`[data-id="${CSS.escape(s.id)}"]`)?.remove();
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
      if (r.status === 403) throw new Error('the box refused; open this page again from the app to finish');
      await must(r);
    };
    await inParallel(indexes.map((i) => 'index/' + i.id), 4, del);
    const packs = [...gone];
    let n = 0;
    await inParallel(packs, 8, async (id) => {
      await del(packPath(id));
      freed += sizes.get(id) || 0;
      n++;
      if (snaps.length) for (const v of views) if (v.total) v.total.textContent = `Freeing space · ${n} of ${packs.length}`;
    });
    onBox = Math.max(0, onBox - freed);
    total();
  } finally {
    clearInterval(beat);
    await fetch(ROOT + 'dd-deleting', { method: 'DELETE' }).catch(() => {});
  }
}

let started = null;
let shown = false;

/// the Backups tab, the first time it is shown
export async function start() {
  if (inApp || servedByApp) {
    $('bk-trash').hidden = false;
    $('bk-empty').onclick = askEmpty;
  }
  return images(tab());
}

/// The same images drawn into another place too (`view`: its msg and list
/// elements, a total if it has one, and whether a row may be deleted)
export async function images(view) {
  if (!views.some((v) => v.list === view.list)) {
    views.push(view);
    if (shown && snaps.length) draw(view);
    else if (shown) {
      view.msg.hidden = false;
      view.msg.innerHTML = views[0].msg.innerHTML;
    }
    total();
  }
  if (!started) {
    started = show()
      .catch((e) => say(String(e.message || e)))
      .finally(() => { shown = true; });
  }
  return started;
}

async function show() {
  if (!inApp && !servedByApp) {
    say('', 'Your disk images open through the Commonty app, which holds their key on your device. <a href="commonty://open?service=site&amp;path=%2F_dd%2Fsettings%23backups">Open them in the app</a> · <a href="/_dd/download">Get the app</a>');
    return;
  }
  await me();
  const data = await open();
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

