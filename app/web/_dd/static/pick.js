// Choosing a place in a member's Files from any page that holds the library
// (Files itself, the PDF editor, Mail): a file to open, or a folder to save
// into, and saving there without ever writing over something. Everything
// is read and sealed in the tab through library.js; nothing new at the gate.

import { el } from './kit.js';
import { list, put, mkdir, human } from './library.js';

const ROOT = 'Files';
let cssLoaded = false;
function css() {
  if (cssLoaded) return;
  cssLoaded = true;
  document.head.append(el('link', { rel: 'stylesheet', href: '/_dd/static/pick.css' }));
}

const label = (dir) => {
  const parts = dir.split('/').slice(1);
  return parts.length ? `My files / ${parts.join(' / ')}` : 'My files';
};

// the folder `dir` if it is inside Files, else Files itself
const inFiles = (dir) => (dir === ROOT || dir?.startsWith(`${ROOT}/`) ? dir : ROOT);

function dialog(title, { folders, want, start, ok, lib, host }) {
  css();
  return new Promise((resolve) => {
    const d = el('dialog', { class: 'dd-pick', 'aria-labelledby': 'dd-pick-t' });
    const where = el('div', { class: 'where', role: 'status' });
    const box = el('div', { class: 'rows' });
    let at = inFiles(start);
    let answer = null;
    const draw = async () => {
      where.textContent = label(at);
      box.replaceChildren(el('div', { class: 'row note', text: 'Opening…' }));
      let items = [];
      try { items = await list(lib, at); } catch (e) { box.replaceChildren(el('div', { class: 'row note', role: 'alert', text: e.message })); return; }
      const rows = [];
      if (at !== ROOT) rows.push(el('button', { type: 'button', class: 'row', text: '↑ Up a folder', onclick: () => { at = at.split('/').slice(0, -1).join('/'); draw(); } }));
      for (const it of items) {
        if (it.name.startsWith('.')) continue;
        if (it.dir) rows.push(el('button', { type: 'button', class: 'row dir', onclick: () => { at = it.path; draw(); } }, el('span', { text: it.name })));
        else if (!folders && want.test(it.name)) {
          rows.push(el('button', { type: 'button', class: 'row', onclick: () => { answer = it; d.close(); } },
            el('span', { text: it.name }), el('span', { class: 'size', text: human(it.size) })));
        }
      }
      if (!rows.length || (rows.length === 1 && at !== ROOT)) rows.push(el('div', { class: 'row note', text: folders ? 'No folders in here.' : 'Nothing here that fits.' }));
      box.replaceChildren(...rows);
      box.querySelector('button')?.focus();
    };
    const foot = [el('button', { type: 'button', class: 'quiet', text: 'Cancel', onclick: () => d.close() })];
    if (folders) {
      foot.unshift(el('button', { type: 'button', class: 'quiet', text: 'New folder', onclick: async () => {
        const name = await ask(d, 'New folder');
        if (!name) return;
        await mkdir(lib, ROOT);
        await mkdir(lib, `${at}/${name}`);
        at = `${at}/${name}`;
        draw();
      } }));
      foot.push(el('button', { type: 'button', text: ok, onclick: () => { answer = at; d.close(); } }));
    }
    d.append(
      el('h2', { id: 'dd-pick-t', text: title }),
      where, box,
      el('div', { class: 'foot' }, ...foot));
    d.onclose = () => { d.remove(); resolve(answer); };
    (host || document.body).append(d);
    d.showModal();
    draw();
  });
}

// a name typed in a small form inside the picker
function ask(parent, title) {
  return new Promise((resolve) => {
    const input = el('input', { type: 'text', 'aria-label': title, autocomplete: 'off' });
    const err = el('p', { class: 'note', role: 'alert' });
    const form = el('form', { class: 'ask' }, input, err,
      el('div', { class: 'foot' },
        el('button', { type: 'button', class: 'quiet', text: 'Cancel', onclick: () => { form.remove(); resolve(null); } }),
        el('button', { type: 'submit', text: 'Make it' })));
    form.onsubmit = (e) => {
      e.preventDefault();
      const v = input.value.trim();
      if (!v) return;
      if (v.includes('/') || v.includes('\\')) { err.textContent = 'A name cannot have a / or \\ in it.'; return; }
      if (v.startsWith('.')) { err.textContent = 'A name cannot start with a dot.'; return; }
      form.remove();
      resolve(v);
    };
    parent.querySelector('.foot').before(form);
    input.focus();
  });
}

/// A file in the member's Files that matches `want` (a RegExp on its
/// name), or null. Opens in `start` (a library path under Files).
export function pickFile(lib, { title = 'Choose a file', want = /./, start = ROOT, host } = {}) {
  return dialog(title, { folders: false, want, start, lib, host });
}

/// A folder in Files to put something in (its library path), or null.
/// Folders can be made on the way.
export function pickFolder(lib, { title = 'Save to Files', start = ROOT, ok = 'Save here', host } = {}) {
  return dialog(title, { folders: true, start, ok, lib, host });
}

/// `name`, or "name (2)", "name (3)"... whichever is free in `dir`
export async function freeName(lib, dir, name) {
  const taken = new Set((await list(lib, dir).catch(() => [])).map((x) => x.name));
  if (!taken.has(name)) return name;
  const dot = name.lastIndexOf('.');
  const stem = dot > 0 ? name.slice(0, dot) : name;
  const ext = dot > 0 ? name.slice(dot) : '';
  for (let n = 2; ; n++) {
    const t = `${stem} (${n})${ext}`;
    if (!taken.has(t)) return t;
  }
}

/// Seal `bytes` into `dir` as `name`, or the next free name; the folders on
/// the way are made. Never writes over a file. Returns the path it used.
export async function saveInto(lib, dir, name, bytes) {
  const clean = (name || 'file').replace(/[/\\]/g, '-').replace(/^\.+/, '') || 'file';
  const parts = inFiles(dir).split('/');
  for (let k = 1; k <= parts.length; k++) await mkdir(lib, parts.slice(0, k).join('/'));
  const free = await freeName(lib, inFiles(dir), clean);
  const path = `${inFiles(dir)}/${free}`;
  await put(lib, path, bytes instanceof Blob ? bytes : new Blob([bytes]));
  return path;
}
