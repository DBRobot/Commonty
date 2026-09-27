// Files: the Files folder of the member's library, in the browser. Opening
// the library and speaking to the gate is library.js; this page is the
// folders, their contents, a preview, uploads, and the trash. Every name
// and byte is turned over here: the gate moves ciphertext.

import { unlock, list, download as fetchTo, fetchPlain, put, trash, mkdir, move, trashed, restore, human } from './library.js';
import { me } from './shell.js';

const $ = (id) => document.getElementById(id);
const { user } = await me();
const ROOT = 'Files';
// a preview opens the whole file in the tab: past this, Download
const PREVIEW_MAX = 64 * 1024 * 1024;

let lib = null;
let place = 'files';             // or 'trash'
let here = '';                   // the folder under ROOT, plain
let items = [];
let sort = { key: 'name', up: true };
let view = 'list';
const chosen = new Set();        // paths ticked
try { view = localStorage.getItem('dd-files-view') || 'list'; } catch { /* private window */ }

const el = (tag, props = {}, ...kids) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else if (k.startsWith('on')) e[k] = v;
    else e.setAttribute(k, v);
  }
  e.append(...kids);
  return e;
};
const under = (dir) => `${ROOT}${dir ? '/' + dir : ''}`;
const inside = (path) => path.slice(ROOT.length + 1);

// ---- what a file is, by its name

const KINDS = {
  folder: ['#2F6FD6', '<path d="M2 5h6l2 2h8v9H2z"/>'],
  image: ['#B4457A', '<rect x="3" y="4" width="14" height="12" rx="1"/><circle cx="8" cy="8.5" r="1.5"/><path d="M3 14l4-4 3 3 3-3 4 4"/>'],
  pdf: ['#C4462F', '<path d="M5 2h7l4 4v12H5z"/><path d="M12 2v4h4M7 11h6M7 14h4"/>'],
  doc: ['#2F6FD6', '<path d="M5 2h7l4 4v12H5z"/><path d="M12 2v4h4M7 10h6M7 13h6M7 16h4"/>'],
  sheet: ['#1D7A55', '<path d="M5 2h7l4 4v12H5z"/><path d="M7 10h7M7 13h7M10 9v7"/>'],
  video: ['#6E47A8', '<rect x="2" y="5" width="12" height="10" rx="1"/><path d="M14 9l4-2v6l-4-2"/>'],
  audio: ['#C9721F', '<path d="M8 15V5l9-2v10"/><circle cx="6" cy="15" r="2"/><circle cx="15" cy="13" r="2"/>'],
  zip: ['#46557F', '<path d="M5 2h10v16H5z"/><path d="M10 2v2M10 6v2M10 10v2M9 13h2v3H9z"/>'],
  text: ['#525A55', '<path d="M5 2h7l4 4v12H5z"/><path d="M12 2v4h4M7 10h6M7 13h6"/>'],
  file: ['#525A55', '<path d="M5 2h7l4 4v12H5z"/><path d="M12 2v4h4"/>'],
};
const EXT = {
  image: 'jpg jpeg png gif webp avif bmp svg heic heif tif tiff',
  pdf: 'pdf',
  doc: 'doc docx odt rtf pages',
  sheet: 'xls xlsx ods csv numbers',
  video: 'mp4 webm mov mkv avi m4v',
  audio: 'mp3 m4a ogg oga opus wav flac aac',
  zip: 'zip tar gz tgz bz2 xz 7z rar zst',
  text: 'txt md log json yaml yml toml ini xml html css js ts py rs go sh c h',
};
function kind(it) {
  if (it.dir) return 'folder';
  const ext = (it.name.split('.').pop() || '').toLowerCase();
  for (const [k, list] of Object.entries(EXT)) if (list.split(' ').includes(ext)) return k;
  return 'file';
}
// what a browser can show by itself, and how
function shows(it) {
  const ext = (it.name.split('.').pop() || '').toLowerCase();
  if (['jpg', 'jpeg', 'png', 'gif', 'webp', 'avif', 'bmp', 'svg'].includes(ext)) return 'image';
  if (ext === 'pdf') return 'pdf';
  if (['mp4', 'webm', 'm4v'].includes(ext)) return 'video';
  if (['mp3', 'm4a', 'ogg', 'oga', 'opus', 'wav', 'flac', 'aac'].includes(ext)) return 'audio';
  if (EXT.text.split(' ').includes(ext) || ext === 'csv') return 'text';
  return null;
}
const MIME = { svg: 'image/svg+xml', pdf: 'application/pdf', png: 'image/png', gif: 'image/gif', webp: 'image/webp', avif: 'image/avif' };

function icon(it, cls = 'ic') {
  const [c, d] = KINDS[kind(it)];
  const s = el('span', { class: cls });
  s.style.setProperty('--c', c);
  s.innerHTML = `<svg viewBox="0 0 20 20" aria-hidden="true">${d}</svg>`;
  return s;
}

const when = (d) => {
  if (!d) return '—';
  const t = d instanceof Date ? d : new Date(d);
  if (Number.isNaN(t.getTime())) return '—';
  const days = (Date.now() - t) / 864e5;
  if (days < 1 && t.getDate() === new Date().getDate()) return `Today ${t.toTimeString().slice(0, 5)}`;
  if (days < 2) return 'Yesterday';
  return t.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: days > 300 ? 'numeric' : undefined });
};

// ---- the folder

function crumbs() {
  const nav = $('crumbs');
  nav.replaceChildren();
  if (place === 'trash') {
    nav.append(el('span', { class: 'here', text: 'Trash' }));
    return;
  }
  const parts = here ? here.split('/') : [];
  const add = (label, to, last) => nav.append(last ? el('span', { class: 'here', text: label }) : el('button', { type: 'button', text: label, onclick: () => show(to) }));
  add('My files', '', !parts.length);
  parts.forEach((p, i) => {
    nav.append(el('span', { class: 'sep', text: '/' }));
    add(p, parts.slice(0, i + 1).join('/'), i === parts.length - 1);
  });
}

function sorted() {
  const q = $('search').value.trim().toLowerCase();
  const out = items.filter((it) => !q || it.name.toLowerCase().includes(q));
  const k = sort.key;
  out.sort((a, b) => {
    if (a.dir !== b.dir) return a.dir ? -1 : 1;
    const x = k === 'size' ? a.size - b.size : k === 'modified' ? (Date.parse(a.modified) || 0) - (Date.parse(b.modified) || 0) : a.name.localeCompare(b.name, undefined, { numeric: true });
    return sort.up ? x : -x;
  });
  return out;
}

function render() {
  crumbs();
  const list = sorted();
  const trashy = place === 'trash';
  document.querySelector('[data-sort="modified"]').firstChild.textContent = trashy ? 'Trashed' : 'Modified';
  $('actions').hidden = !!lib.reader || trashy;
  for (const b of document.querySelectorAll('[data-sort]')) {
    b.querySelector('.arrow')?.remove();
    if (b.dataset.sort === sort.key) b.append(el('span', { class: 'arrow', text: sort.up ? ' ↑' : ' ↓' }));
  }
  $('rows').replaceChildren(...list.map((it) => {
    const tick = el('input', { type: 'checkbox', 'aria-label': `Select ${it.name}` });
    tick.checked = chosen.has(it.path);
    tick.hidden = trashy || lib.reader;
    tick.onchange = () => { tick.checked ? chosen.add(it.path) : chosen.delete(it.path); selection(); };
    const name = el('div', { class: 'name' }, icon(it), el('span', { text: it.name }));
    if (trashy) name.append(el('span', { class: 'from', text: `from ${folderOf(it.path)}` }));
    const tr = el('tr', { 'aria-selected': String(chosen.has(it.path)) },
      el('td', { class: 'chk' }, tick),
      el('td', {}, name),
      el('td', { class: 'mono wide-only', text: trashy ? when(it.when) : when(it.modified) }),
      el('td', { class: 'mono wide-only', text: it.dir ? '—' : human(it.size) }),
      el('td', { class: 'act' }, trashy
        ? (lib.reader ? '' : el('button', { class: 'quiet small', type: 'button', text: 'Restore', onclick: (e) => { e.stopPropagation(); back(it); } }))
        : el('button', { class: 'more', type: 'button', 'aria-label': `More for ${it.name}`, text: '⋯', onclick: (e) => { e.stopPropagation(); menu(it, e.currentTarget); } })));
    tr.onclick = (e) => { if (!e.target.closest('input,button')) open(it); };
    return tr;
  }));
  $('tiles').replaceChildren(...list.map((it) => {
    const t = el('button', { type: 'button', class: 'tile', 'aria-selected': String(chosen.has(it.path)) },
      el('div', { class: 'thumb' }, icon(it, 'big')),
      el('b', { text: it.name }),
      el('span', { text: it.dir ? 'Folder' : `${human(it.size)} · ${when(trashy ? it.when : it.modified)}` }));
    t.style.setProperty('--c', KINDS[kind(it)][0]);
    t.onclick = () => open(it);
    return t;
  }));
  $('table').hidden = view !== 'list';
  $('tiles').hidden = view !== 'grid';
  for (const b of document.querySelectorAll('[data-view]')) b.setAttribute('aria-pressed', String(b.dataset.view === view));
  for (const b of document.querySelectorAll('[data-place]')) b.setAttribute('aria-current', String(b.dataset.place === place));
  $('empty').hidden = list.length > 0;
  $('empty').textContent = trashy ? 'The trash is empty. Things put here go for good after 90 days.' : ($('search').value ? 'Nothing here by that name.' : 'Nothing here yet. Drop files anywhere on the page, or use Upload.');
  $('foot').textContent = trashy ? 'Trashed files stay for 90 days, then the box lets them go for good.' : 'Names and contents are encrypted in this tab. Drop files anywhere on the page to upload them here.';
  $('all').hidden = trashy || lib.reader;
  selection();
}

const folderOf = (path) => {
  const parts = path.split('/');
  parts.pop();
  return parts.length ? parts.join(' / ') : 'the top';
};

function selection() {
  $('selbar').hidden = !chosen.size;
  $('selcount').textContent = `${chosen.size} selected`;
  $('all').checked = chosen.size > 0 && chosen.size === items.length;
}

async function show(dir) {
  place = 'files';
  here = dir;
  chosen.clear();
  $('search').value = '';
  items = await list(lib, under(dir));
  render();
}

async function showTrash() {
  place = 'trash';
  chosen.clear();
  $('search').value = '';
  items = (await trashed(lib)).map((t) => ({ ...t, dir: false, modified: '' }));
  render();
}

const reload = () => (place === 'trash' ? showTrash() : show(here));

// ---- a question in a small window: a name, or a folder

function ask({ title, text = '', value = null, folders = false, ok = 'OK' }) {
  const d = $('ask');
  $('ask-title').textContent = title;
  $('ask-text').textContent = text;
  $('ask-ok').textContent = ok;
  const input = $('ask-input');
  input.hidden = value === null;
  input.value = value ?? '';
  const pickList = $('ask-list');
  pickList.hidden = !folders;
  let at = here;
  const drawFolders = async () => {
    const sub = (await list(lib, under(at))).filter((i) => i.dir);
    pickList.replaceChildren(
      el('div', { class: 'ask-where', text: `Into: My files${at ? ' / ' + at.split('/').join(' / ') : ''}` }),
      ...(at ? [el('button', { type: 'button', class: 'ask-row', text: '↑ Up a folder', onclick: () => { at = at.split('/').slice(0, -1).join('/'); drawFolders(); } })] : []),
      ...sub.map((f) => el('button', { type: 'button', class: 'ask-row', text: `📁 ${f.name}`, onclick: () => { at = inside(f.path); drawFolders(); } })),
    );
  };
  if (folders) drawFolders();
  return new Promise((resolve) => {
    d.onclose = () => resolve(d.returnValue === 'ok' ? (folders ? at : input.value.trim()) : null);
    d.showModal();
    if (value !== null) { input.focus(); input.select(); }
  });
}

// ---- what can be done to a file

function menu(it, anchor) {
  document.querySelector('.popmenu')?.remove();
  const m = el('div', { class: 'popmenu', role: 'menu' });
  const add = (label, fn, danger) => m.append(el('button', { type: 'button', role: 'menuitem', class: danger ? 'danger' : '', text: label, onclick: () => { m.remove(); fn(); } }));
  if (it.dir) add('Open', () => open(it));
  else add('Download', () => download(it));
  if (!lib.reader) {
    add('Rename', () => rename(it));
    add('Move', () => relocate([it]));
    add('Move to trash', () => toTrash([it]), true);
  }
  document.body.append(m);
  const r = anchor.getBoundingClientRect();
  m.style.top = `${r.bottom + scrollY + 4}px`;
  m.style.left = `${Math.max(8, r.right + scrollX - 190)}px`;
  setTimeout(() => addEventListener('click', () => m.remove(), { once: true }));
}

async function rename(it) {
  const name = await ask({ title: `Rename ${it.dir ? 'folder' : 'file'}`, value: it.name, ok: 'Rename' });
  if (!name || name === it.name) return;
  if (name.includes('/')) return note('A name cannot have a / in it.');
  try {
    await move(lib, it.path, `${folderPath(it.path)}/${name}`);
  } catch (e) {
    return note(/412/.test(e.message) ? `Something called ${name} is already there.` : e.message);
  }
  closePreview();
  reload();
}

const folderPath = (path) => path.split('/').slice(0, -1).join('/');

async function relocate(list) {
  const to = await ask({ title: list.length > 1 ? `Move ${list.length} things` : `Move ${list[0].name}`, folders: true, ok: 'Move here' });
  if (to === null) return;
  const failed = [];
  for (const it of list) {
    const dest = `${under(to)}/${it.name}`;
    if (dest === it.path || dest.startsWith(it.path + '/')) continue;
    try { await move(lib, it.path, dest); } catch { failed.push(it.name); }
  }
  if (failed.length) note(`Not moved, something by that name is already there: ${failed.join(', ')}`);
  chosen.clear();
  closePreview();
  reload();
}

async function toTrash(list) {
  for (const it of list) await trash(lib, it.path);
  chosen.clear();
  closePreview();
  reload();
}

async function back(it) {
  let to = it.path;
  for (let n = 1; ; n++) {
    try {
      await restore(lib, it, to);
      break;
    } catch (e) {
      if (!/412/.test(e.message) || n > 20) return note(e.message);
      const dot = it.name.lastIndexOf('.');
      const stem = dot > 0 ? it.name.slice(0, dot) : it.name;
      const ext = dot > 0 ? it.name.slice(dot) : '';
      to = `${folderPath(it.path)}/${stem} (restored${n > 1 ? ' ' + n : ''})${ext}`;
    }
  }
  showTrash();
}

function note(text) {
  $('msg').textContent = text;
  $('msg').hidden = false;
  clearTimeout(note.t);
  note.t = setTimeout(() => { $('msg').hidden = true; }, 6000);
}

// ---- preview

let shown = null;               // the object url on screen

function closePreview() {
  $('preview').hidden = true;
  if (shown) URL.revokeObjectURL(shown);
  shown = null;
  $('pv-show').replaceChildren();
}

async function open(it) {
  if (it.dir) return show(inside(it.path));
  closePreview();
  $('pv-name').textContent = it.name;
  $('pv-meta').replaceChildren(
    el('dt', { text: 'Size' }), el('dd', { text: human(it.size) }),
    el('dt', { text: place === 'trash' ? 'Trashed' : 'Modified' }), el('dd', { text: when(place === 'trash' ? it.when : it.modified) }),
    el('dt', { text: 'Where' }), el('dd', { text: `My files${folderPath(it.path).length > ROOT.length ? ' / ' + folderPath(inside(it.path)).split('/').join(' / ') : ''}` }),
    el('dt', { text: 'Stored' }), el('dd', { text: 'Encrypted; the box cannot read it' }));
  const acts = $('pv-actions');
  acts.replaceChildren();
  if (place === 'trash') {
    if (!lib.reader) acts.append(el('button', { type: 'button', text: 'Restore', onclick: () => { back(it); closePreview(); } }));
  } else {
    acts.append(el('button', { type: 'button', text: 'Download', onclick: () => download(it) }));
    if (!lib.reader) {
      acts.append(
        el('button', { type: 'button', class: 'quiet', text: 'Rename', onclick: () => rename(it) }),
        el('button', { type: 'button', class: 'quiet', text: 'Move', onclick: () => relocate([it]) }),
        el('button', { type: 'button', class: 'quiet', text: 'Trash', onclick: () => toTrash([it]) }));
    }
  }
  $('preview').hidden = false;
  const box = $('pv-show');
  const how = shows(it);
  if (!how) {
    box.append(el('div', { class: 'pv-none' }, icon(it, 'big'), el('p', { class: 'note', text: 'No preview for this kind of file. Download it to open it.' })));
    return;
  }
  if (it.size > PREVIEW_MAX) {
    box.append(el('div', { class: 'pv-none' }, icon(it, 'big'), el('p', { class: 'note', text: `${human(it.size)} is too big to preview here. Download it to open it.` })));
    return;
  }
  box.append(el('p', { class: 'note', text: 'Unlocking…' }));
  try {
    const plain = place === 'trash' ? null : await fetchPlain(lib, it.path);
    if (!plain) { box.replaceChildren(el('p', { class: 'note', text: 'Restore it to preview it.' })); return; }
    if (!$('preview').hidden && $('pv-name').textContent !== it.name) return;
    const ext = (it.name.split('.').pop() || '').toLowerCase();
    if (how === 'text') {
      const t = new TextDecoder().decode(plain.slice(0, 64 * 1024));
      box.replaceChildren(el('pre', { class: 'pv-text', text: t + (plain.length > 64 * 1024 ? '\n…' : '') }));
      return;
    }
    shown = URL.createObjectURL(new Blob([plain], { type: MIME[ext] || '' }));
    const media = how === 'image' ? el('img', { src: shown, alt: it.name })
      : how === 'pdf' ? el('iframe', { src: shown, title: it.name })
      : how === 'video' ? el('video', { src: shown, controls: '' })
      : el('audio', { src: shown, controls: '' });
    media.className = `pv-${how}`;
    box.replaceChildren(media);
  } catch (e) {
    box.replaceChildren(el('p', { class: 'note', text: e.message }));
  }
}

// ---- uploads and downloads, in one tray

const tray = { rows: 0, busy: 0 };

function trayRow(name, verb) {
  $('uploads').hidden = false;
  tray.rows++;
  tray.busy++;
  const bar = el('i');
  const state = el('span', { text: verb });
  const row = el('div', { class: 'up-row' }, el('div', { class: 'top' }, el('span', { text: name }), state), el('div', { class: 'track' }, bar));
  $('up-rows').prepend(row);
  const title = () => { $('up-title').textContent = tray.busy ? `Working on ${tray.busy} file${tray.busy === 1 ? '' : 's'}` : 'All done'; };
  title();
  return {
    step: (d, t, what) => { bar.style.width = `${t ? Math.floor((100 * d) / t) : 100}%`; state.textContent = `${what} · ${t ? Math.floor((100 * d) / t) : 100}%`; },
    done: (text) => { tray.busy--; bar.style.width = '100%'; state.textContent = text; title(); },
    fail: (text) => { tray.busy--; row.classList.add('failed'); state.textContent = text; title(); },
  };
}

async function download(it) {
  const job = trayRow(it.name, 'Fetching');
  try {
    await fetchTo(lib, it, (d, t) => job.step(d, t, 'Unlocking'));
    job.done('Saved');
  } catch (e) {
    job.fail(e.message);
  }
}

async function upload(files) {
  if (lib.reader || place === 'trash') return;
  const into = under(here);
  for (const f of files) {
    const job = trayRow(f.name, 'Waiting');
    try {
      await put(lib, `${into}/${f.name}`, f, (d, t) => job.step(d, t, 'Encrypting and sending'));
      job.done(`Done · ${human(f.size)}`);
    } catch (e) {
      job.fail(e.message);
    }
  }
  if (place === 'files' && into === under(here)) show(here);
}

// ---- starting

async function start() {
  const r = await unlock(user);
  if (r.none) {
    $('msg').textContent = 'No library yet. `dd library new` makes one on the machine that holds your key.';
    return;
  }
  if (r.link) {
    $('msg').hidden = true;
    $('link').hidden = false;
    return;
  }
  lib = r.ok;
  $('msg').hidden = true;
  if (lib.reader) $('lead').textContent = 'You are looking at the demo’s files. A member sees their own here, and only they can read them.';
  $('drive').hidden = false;
  $('actions').hidden = !!lib.reader;
  $('trashplace').hidden = !!lib.reader;
  $('up').onclick = () => $('picker').click();
  $('picker').onchange = () => { upload([...$('picker').files]); $('picker').value = ''; };
  $('newfolder').onclick = async () => {
    const name = await ask({ title: 'New folder', value: '', ok: 'Make it' });
    if (!name) return;
    if (name.includes('/')) return note('A name cannot have a / in it.');
    await mkdir(lib, ROOT);
    await mkdir(lib, `${under(here)}/${name}`);
    show(here);
  };
  for (const b of document.querySelectorAll('[data-place]')) b.onclick = () => (b.dataset.place === 'trash' ? showTrash() : show(''));
  for (const b of document.querySelectorAll('[data-view]')) b.onclick = () => { view = b.dataset.view; try { localStorage.setItem('dd-files-view', view); } catch { /* */ } render(); };
  for (const b of document.querySelectorAll('[data-sort]')) b.onclick = () => { sort = { key: b.dataset.sort, up: sort.key === b.dataset.sort ? !sort.up : true }; render(); };
  $('search').oninput = () => render();
  $('all').onchange = () => { chosen.clear(); if ($('all').checked) for (const it of items) chosen.add(it.path); render(); };
  const picked = () => items.filter((it) => chosen.has(it.path));
  $('sel-down').onclick = async () => { for (const it of picked()) if (!it.dir) await download(it); };
  $('sel-move').onclick = () => relocate(picked());
  $('sel-trash').onclick = () => toTrash(picked());
  $('pv-close').onclick = closePreview;
  $('up-close').onclick = () => { $('uploads').hidden = true; };
  addEventListener('keydown', (e) => { if (e.key === 'Escape') { closePreview(); document.querySelector('.popmenu')?.remove(); } });
  // drop anywhere on the page to upload into this folder
  let depth = 0;
  addEventListener('dragenter', (e) => {
    if (lib.reader || place === 'trash' || !e.dataTransfer?.types.includes('Files')) return;
    depth++;
    $('drop-to').textContent = `Drop to upload into ${here ? here.split('/').pop() : 'My files'}`;
    $('drop').hidden = false;
  });
  addEventListener('dragleave', () => { if (--depth <= 0) { depth = 0; $('drop').hidden = true; } });
  addEventListener('dragover', (e) => { if (!$('drop').hidden) e.preventDefault(); });
  addEventListener('drop', (e) => {
    if ($('drop').hidden) return;
    e.preventDefault();
    depth = 0;
    $('drop').hidden = true;
    upload([...e.dataTransfer.files]);
  });
  await show('');
}

start().catch((e) => { $('msg').hidden = false; $('msg').textContent = String(e.message || e); });
