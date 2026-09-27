// Movies & TV: the two corners of the library a player cares about.
// Films are files under Movies/, a programme is a folder under Shows/
// with its episodes inside. The same passkey, the same gate, the same
// ciphertext: only the shape of the listing is different (library.js).

import { unlock, list, fetchPlain, save, put, trash, mkdir, transcode, stopTranscode, playsPlaylists, playlistPlayer, human } from './library.js';

const $ = (id) => document.getElementById(id);
import { me } from './shell.js';
const { user } = await me();
// a whole film cannot be opened in a tab: the bytes are turned over here,
// so the tab would hold the film twice. Small things play, the rest is
// what the desktop mount and the app are for.
const INLINE = 256 * 1024 * 1024;
let lib = null;
let show = null;                 // the programme being looked at, or null
let hls = null;                  // the player, where the browser needs one
let session = null;              // the box's playlist, while a film is open

function tile(label, sub, onclick) {
  const li = document.createElement('li');
  const b = document.createElement('button');
  b.className = 'rowname';
  b.textContent = label;
  b.onclick = onclick;
  const tag = document.createElement('span');
  tag.className = 'tag';
  tag.textContent = sub;
  li.append(b, tag);
  return li;
}

function rm(path, after) {
  const b = document.createElement('button');
  b.className = 'quiet inline';
  b.textContent = 'Trash';
  b.onclick = async () => {
    b.disabled = true;
    await trash(lib, path);
    after();
  };
  return b;
}

async function films() {
  const items = (await list(lib, 'Movies')).filter((i) => !i.dir);
  const ul = $('films');
  ul.replaceChildren(...items.map((it) => {
    const li = tile(it.name, human(it.size), () => play(it));
    if (!lib.reader) li.append(rm(it.path, films));
    return li;
  }));
  $('nofilms').hidden = items.length > 0;
}

async function programmes() {
  const items = (await list(lib, 'Shows')).filter((i) => i.dir);
  const ul = $('shows');
  ul.replaceChildren(...items.map((it) => tile(`📺 ${it.name}`, '', () => episodes(it.name))));
  $('noshows').hidden = items.length > 0;
  $('back').hidden = true;
  $('showname').hidden = true;
  $('addep').hidden = true;
  $('newshow').hidden = !!lib.reader;
  show = null;
}

async function episodes(name) {
  show = name;
  const items = (await list(lib, `Shows/${name}`)).filter((i) => !i.dir);
  const ul = $('shows');
  ul.replaceChildren(...items.map((it) => {
    const li = tile(it.name, human(it.size), () => play(it));
    if (!lib.reader) li.append(rm(it.path, () => episodes(name)));
    return li;
  }));
  $('noshows').hidden = items.length > 0;
  $('showname').textContent = name;
  $('showname').hidden = false;
  $('back').hidden = false;
  $('addep').hidden = !!lib.reader;
  $('newshow').hidden = true;
}

async function play(it) {
  const p = $('playing');
  const v = $('video');
  p.hidden = false;
  $('savefile').hidden = true;
  $('title').textContent = it.name;
  // A film is bigger than a tab: the box decrypts it in its own memory
  // for this one viewing and sends a playlist. Safari plays a playlist
  // itself; everywhere else the page loads a player from the box.
  $('note').textContent = 'Asking the box to play it…';
  try {
    await close();
    const url = await transcode(lib, it.path, it.sealed);
    session = url;
    if (playsPlaylists()) {
      v.src = url;
    } else {
      const Hls = await playlistPlayer();
      hls?.destroy();
      hls = new Hls();
      hls.loadSource(url);
      hls.attachMedia(v);
    }
    v.hidden = false;
    $('note').textContent = '';
    v.play().catch(() => {});
    return;
  } catch (e) {
    $('note').textContent = e.message;
    // and fall through: a small file still opens in the tab
  }
  if (it.size > INLINE) {
    v.hidden = true;
    $('note').textContent = `${human(it.size)}: too big to open in this browser, which plays no playlist of its own. \`dd media\` puts this library on your desktop as folders.`;
    $('savefile').hidden = false;
    $('savefile').onclick = () => download(it);
    return;
  }
  $('note').textContent = 'Opening…';
  try {
    const plain = await fetchPlain(lib, it.path);
    v.src = URL.createObjectURL(new Blob([plain]));
    v.hidden = false;
    $('note').textContent = '';
    v.play().catch(() => {});
  } catch (e) {
    $('note').textContent = e.message;
  }
}

/// stop the film and tell the box to stop making it
async function close() {
  $('video').pause();
  $('video').removeAttribute('src');
  hls?.destroy();
  hls = null;
  if (session) {
    const gone = session;
    session = null;
    await stopTranscode(gone);
  }
}

async function download(it) {
  $('note').textContent = 'Fetching…';
  try {
    save(await fetchPlain(lib, it.path), it.name);
    $('note').textContent = 'Saved.';
  } catch (e) {
    $('note').textContent = e.message;
  }
}

async function upload(files, into) {
  for (const f of files) {
    $('msg').hidden = false;
    $('msg').textContent = `${f.name} — encrypting`;
    try {
      await put(lib, `${into}/${f.name}`, f);
      $('msg').textContent = `${f.name} — ${human(f.size)}`;
    } catch (e) {
      $('msg').textContent = `${f.name} — ${e.message}`;
    }
  }
  await (into === 'Movies' ? films() : episodes(show));
}

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
  $('shelves').hidden = false;
  for (const id of ['addfilm', 'newshow']) $(id).hidden = !!lib.reader;
  $('addfilm').onclick = () => $('filmpicker').click();
  $('filmpicker').onchange = () => upload($('filmpicker').files, 'Movies');
  $('addep').onclick = () => $('eppicker').click();
  $('eppicker').onchange = () => upload($('eppicker').files, `Shows/${show}`);
  $('back').onclick = () => programmes();
  $('newshow').onclick = async () => {
    const name = prompt('What is the programme called?');
    if (!name) return;
    await mkdir(lib, `Shows/${name}`);
    await episodes(name);
  };
  $('close').onclick = () => {
    close();
    $('playing').hidden = true;
  };
  // a tab closed mid-film should not leave the box transcoding either
  addEventListener('pagehide', () => { if (session) stopTranscode(session, true); });
  await Promise.all([films(), programmes()]);
}

start().catch((e) => { $('msg').textContent = String(e.message || e); });
