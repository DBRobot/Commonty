// Files: one corner of the member's library, in the browser. Everything
// that opens the library and speaks to the gate is in library.js; this
// page is the folders, the upload and the trash.

import { unlock, list, download as fetchTo, put, trash, human } from './library.js';

const $ = (id) => document.getElementById(id);
import { me } from './shell.js';
const { user } = await me();
const ROOT = 'Files';
let lib = null;
let here = '';                   // the folder under ROOT, plain

function under(dir) {
  return `${ROOT}${dir ? '/' + dir : ''}`;
}

function crumbs() {
  const el = $('crumbs');
  const parts = here ? here.split('/') : [];
  el.replaceChildren();
  const add = (label, to) => {
    const a = document.createElement('button');
    a.className = 'crumb';
    a.textContent = label;
    a.onclick = () => show(to);
    el.append(a);
  };
  add('Files', '');
  parts.forEach((p, i) => {
    el.append(document.createTextNode('/'));
    add(p, parts.slice(0, i + 1).join('/'));
  });
  el.hidden = false;
}

async function show(dir) {
  here = dir;
  crumbs();
  const items = await list(lib, under(dir));
  const ul = $('list');
  ul.replaceChildren(...items.map((it) => {
    const li = document.createElement('li');
    const name = document.createElement('button');
    name.className = 'rowname';
    name.textContent = (it.dir ? '📁 ' : '') + it.name;
    name.onclick = () => (it.dir ? show(here ? `${here}/${it.name}` : it.name) : download(it));
    const meta = document.createElement('span');
    meta.className = 'tag';
    meta.textContent = it.dir ? '' : human(it.size);
    li.append(name, meta);
    if (!it.dir && !lib.reader) {
      const rm = document.createElement('button');
      rm.className = 'quiet inline';
      rm.textContent = 'Trash';
      rm.onclick = async () => {
        rm.disabled = true;
        await trash(lib, it.path);
        show(here);
      };
      li.append(rm);
    }
    return li;
  }));
  ul.hidden = false;
  $('empty').hidden = items.length > 0;
  $('up').hidden = lib.reader;
}

function addJob(text) {
  const li = document.createElement('li');
  li.textContent = text;
  $('jobs').append(li);
  $('jobs').hidden = false;
  return li;
}

const pct = (done, total) => (total ? Math.floor((100 * done) / total) : 100);

async function download(it) {
  const job = addJob(`${it.name} — fetching`);
  try {
    await fetchTo(lib, it, (d, t) => { job.textContent = `${it.name} — fetching ${pct(d, t)}%`; });
    job.textContent = `${it.name} — saved`;
  } catch (e) {
    job.textContent = `${it.name} — ${e.message}`;
  }
}

async function upload(files) {
  for (const f of files) {
    const job = addJob(`${f.name} — encrypting`);
    try {
      await put(lib, `${under(here)}/${f.name}`, f, (d, t) => {
        job.textContent = `${f.name} — encrypting and sending ${pct(d, t)}%`;
      });
      job.textContent = `${f.name} — ${human(f.size)}`;
    } catch (e) {
      job.textContent = `${f.name} — ${e.message}`;
    }
  }
  show(here);
}

async function start() {
  const r = await unlock(user);
  if (r.none) {
    $('msg').textContent = 'No library yet. `dd library new` makes one on the machine that holds your key.';
    return;
  }
  if (r.link) {
    $('msg').textContent = 'This browser is not linked to your files yet.';
    $('linkcmd').textContent = r.link;
    $('link').hidden = false;
    return;
  }
  lib = r.ok;
  $('msg').hidden = lib.reader === undefined ? true : !lib.reader;
  if (lib.reader) {
    $('msg').textContent = 'You are looking at the demo\u2019s files. A member sees their own here, and only they can read them.';
  }
  $('up').onclick = () => $('picker').click();
  $('picker').onchange = () => upload($('picker').files);
  await show('');
}

start().catch((e) => { $('msg').textContent = String(e.message || e); });
