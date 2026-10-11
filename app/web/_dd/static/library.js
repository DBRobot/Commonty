// A member's library in a page: the member's app hands over the key, the
// gate hands over ciphertext on WebDAV, and every name and every byte is
// turned over here. Files and Movies & TV are two corners of the same
// library, so the opening, listing and carrying live here once. The key is
// only ever this device's: the app opens it with its own keys, for its
// window and for a browser it let in (app/src/local.rs). No box serves
// this page to a member, and no passkey makes a key.

import init, { library_blocks_for_box, path_encrypt, path_decrypt, file_open, file_seal, plain_size, Sealer, Opener } from '/_dd/web/dd_web.js';
import { inApp, servedByApp, appFetch, pageConfig } from './shell.js';

/// The library this viewer opens. Returns `{ ok: lib }`, `{ none: true }`
/// when the account has no library, or `{ app: true }` when this page is
/// not the app's, which alone holds the key. A library comes with the role
/// this viewer has in it: a reader looks and does not touch.
export async function unlock() {
  await init();
  // the app's window, or a browser the app let in: the app opens the
  // library with this device's own keys and hands the page its key
  if (inApp || servedByApp) {
    const r = inApp ? await appFetch('/_dd/app/library') : await fetch('/_dd/app/library');
    if (r.status === 404) return { none: true };
    if (!r.ok) throw new Error(`the app said ${r.status}: ${await r.text()}`);
    return { ok: await r.json() };
  }
  const cfg = await pageConfig();
  // the demo holds no key of its own; the box hands it one (verify.nix)
  if (cfg.demoLibrary) {
    return { ok: { id: cfg.demoLibrary.id, key: cfg.demoLibrary.key, reader: true, demo: true } };
  }
  return { app: true };
}

/// the gate's url for a path in the library, under its encrypted name
export function dav(lib, path) {
  const enc = path ? path_encrypt(lib.key, lib.id, path).split('/').map(encodeURIComponent).join('/') : '';
  return `/_dd/dav/${lib.id}/${enc}`;
}

/// what a folder holds, names and sizes as they really are
export async function list(lib, dir) {
  const r = await fetch(dav(lib, dir) + '/', { method: 'PROPFIND', headers: { depth: '1' } });
  if (r.status === 404) return [];
  if (r.status !== 207) throw new Error(`the gate said ${r.status}`);
  const doc = new DOMParser().parseFromString(await r.text(), 'application/xml');
  const prefix = `/_dd/dav/${lib.id}/`;
  const out = [];
  for (const el of doc.getElementsByTagNameNS('DAV:', 'response')) {
    const href = el.getElementsByTagNameNS('DAV:', 'href')[0]?.textContent || '';
    const rel = decodeURI(href).slice(prefix.length).replace(/\/$/, '');
    if (!rel) continue;
    let path;
    try { path = path_decrypt(lib.key, lib.id, rel.split('/').map(decodeURIComponent).join('/')); }
    catch { continue; }                       // not ours to read: the trash, a stray
    if (path === dir) continue;               // the folder lists itself first
    const isDir = !!el.getElementsByTagNameNS('DAV:', 'collection').length;
    const sealed = Number(el.getElementsByTagNameNS('DAV:', 'getcontentlength')[0]?.textContent || 0);
    out.push({
      path,
      name: path.split('/').pop(),
      dir: isDir,
      size: isDir ? 0 : plain_size(sealed),
      sealed,
      modified: el.getElementsByTagNameNS('DAV:', 'getlastmodified')[0]?.textContent || '',
    });
  }
  out.sort((a, b) => (a.dir === b.dir ? a.name.localeCompare(b.name) : a.dir ? -1 : 1));
  return out;
}

/// a file out of the library and into the machine's downloads
export async function fetchPlain(lib, path) {
  const r = await fetch(dav(lib, path));
  if (!r.ok) throw new Error(`the gate said ${r.status}`);
  return file_open(lib.key, lib.id, new Uint8Array(await r.arrayBuffer()));
}

/// A note about the library kept beside it at the gate, not in it
/// ('progress', 'shelf'): sealed here as a file would be, and replaced
/// whole, so nothing of it goes to the trash. null when there is none.
export async function note(lib, kind) {
  const r = await fetch(`/_dd/library/${lib.id}/notes/${kind}`);
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(`the gate said ${r.status}`);
  return JSON.parse(new TextDecoder().decode(file_open(lib.key, lib.id, new Uint8Array(await r.arrayBuffer()))));
}

export async function putNote(lib, kind, value) {
  const body = file_seal(lib.key, lib.id, new TextEncoder().encode(JSON.stringify(value)));
  const r = await fetch(`/_dd/library/${lib.id}/notes/${kind}`, { method: 'PUT', body });
  if (!r.ok) throw new Error(`the gate said ${r.status}`);
}

export function save(bytes, name) {
  const url = URL.createObjectURL(bytes instanceof Blob ? bytes : new Blob([bytes]));
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

// A large file is never in the tab's memory whole: it is read, sealed and
// sent a piece at a time (an S3 multipart upload through the gate), and
// read back by ranges. A piece is 96 blocks: over S3's 5 MiB least part,
// under nginx's 10 MB body.
const BLOCK = 64 * 1024;
const SEALED_BLOCK = BLOCK + 16;
const HEADER = 32;
const PIECE_BLOCKS = 96;

/// a file into the library, sealed here; progress(done, total) if given
export async function put(lib, path, file, progress) {
  const url = dav(lib, path);
  const piece = PIECE_BLOCKS * BLOCK;
  if (file.size <= piece) {
    const sealed = file_seal(lib.key, lib.id, new Uint8Array(await file.arrayBuffer()));
    const r = await fetch(url, { method: 'PUT', body: sealed });
    if (!r.ok) throw new Error(`the gate said ${r.status}`);
    progress?.(file.size, file.size);
    return;
  }
  const sealer = new Sealer(lib.key, lib.id);
  const start = await fetch(url, { method: 'POST', headers: { 'x-dd-upload': 'start' } });
  if (!start.ok) throw new Error(`the gate said ${start.status}`);
  const { upload } = await start.json();
  try {
    const parts = [];
    for (let off = 0, n = 1; off < file.size; off += piece, n++) {
      const plain = new Uint8Array(await file.slice(off, off + piece).arrayBuffer());
      let body = sealer.seal(off / BLOCK, plain);
      if (n === 1) {
        const h = sealer.header();
        const first = new Uint8Array(h.length + body.length);
        first.set(h);
        first.set(body, h.length);
        body = first;
      }
      const r = await fetch(url, { method: 'PUT', headers: { 'x-dd-upload': upload, 'x-dd-part': String(n) }, body });
      if (!r.ok) throw new Error(`the gate said ${r.status}`);
      parts.push((await r.json()).etag);
      progress?.(off + plain.length, file.size);
    }
    const done = await fetch(url, {
      method: 'POST',
      headers: { 'x-dd-upload': upload, 'content-type': 'application/json' },
      body: JSON.stringify({ parts }),
    });
    if (!done.ok) throw new Error(`the gate said ${done.status}`);
  } catch (e) {
    fetch(url, { method: 'DELETE', headers: { 'x-dd-upload': upload } }).catch(() => {});
    throw e;
  } finally {
    sealer.free();
  }
}

async function range(url, from, to) {
  const r = await fetch(url, { headers: { range: `bytes=${from}-${to}` } });
  if (r.status !== 206 && r.status !== 200) throw new Error(`the gate said ${r.status}`);
  return new Uint8Array(await r.arrayBuffer());
}

/// A file out of the library by ranges, opened a piece at a time: straight
/// to a file the person picks where the browser allows it, otherwise into
/// a Blob of pieces, which a browser keeps on disk when it is large.
export async function download(lib, item, progress) {
  const url = dav(lib, item.path);
  let sink = null;
  if (window.showSaveFilePicker) {
    try {
      sink = await (await window.showSaveFilePicker({ suggestedName: item.name })).createWritable();
    } catch (e) {
      if (e.name === 'AbortError') return;
    }
  }
  const opener = new Opener(lib.key, lib.id, await range(url, 0, HEADER - 1));
  try {
    const step = PIECE_BLOCKS * SEALED_BLOCK;
    const pieces = [];
    for (let off = HEADER, block = 0; off < item.sealed; off += step, block += PIECE_BLOCKS) {
      const plain = opener.open(block, await range(url, off, Math.min(off + step, item.sealed) - 1));
      if (sink) await sink.write(plain);
      else pieces.push(new Blob([plain]));
      progress?.(Math.min((block + PIECE_BLOCKS) * BLOCK, item.size), item.size);
    }
    if (sink) await sink.close();
    else save(new Blob(pieces), item.name);
  } catch (e) {
    if (sink) await sink.abort().catch(() => {});
    throw e;
  } finally {
    opener.free();
  }
}

export async function trash(lib, path) {
  await fetch(dav(lib, path), { method: 'DELETE' });
}

/// A film this tab cannot open: the box does the work instead. It is
/// given a url it can fetch ranges from while the film plays and this
/// file's block keys (one per 64 KiB, made here from its header) sealed to
/// a key it made when it started: they open this file and nothing else,
/// and the library's key never leaves the tab.
/// What comes back is a playlist on the same host as this page, where in
/// the film it begins (`from`, seconds: resuming), and the film's length
/// when its container says.
export async function transcode(lib, path, sealedSize, from = 0) {
  const at = dav(lib, path);
  const pre = await fetch(at, { headers: { 'x-dd-presign': '1' } });
  if (!pre.ok) throw new Error(`the gate said ${pre.status}`);
  const { url } = await pre.json();
  const header = await range(at, 0, HEADER - 1);
  const box = await fetch('/_dd/transcode/key');
  if (!box.ok) throw new Error('this box does not transcode');
  const { key } = await box.json();
  const r = await fetch('/_dd/transcode/session', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ url, keys: library_blocks_for_box(lib.key, lib.id, header, sealedSize, key), size: sealedSize, from }),
  });
  if (!r.ok) throw new Error(`the box said ${r.status}: ${await r.text()}`);
  const s = await r.json();
  return { url: `/_dd/transcode${s.playlist}`, from: s.from || 0, duration: s.duration || null };
}

/// Tell the box it can stop. Without this the ffmpeg behind a film the
/// viewer closed after a minute keeps going until the session times out
/// ten minutes later, which on a box with one job at a time is the
/// difference between the next film starting now and starting then.
export async function stopTranscode(url, leaving) {
  const at = url.replace(/\/index\.m3u8$/, '');
  try {
    // keepalive so it still goes when the tab is on its way out; a beacon
    // would not, since that is a POST and this route takes DELETE
    await fetch(at, { method: 'DELETE', keepalive: !!leaving });
  } catch { /* it times out by itself; this is only sooner */ }
}

/// whether this browser plays a playlist by itself (Safari, every iPhone;
/// Chrome too, now): the page prefers its own player where it can run
export function playsPlaylists() {
  const v = document.createElement('video');
  return !!(v.canPlayType('application/vnd.apple.mpegurl') || v.canPlayType('application/x-mpegURL'));
}

/// Where it does not, a player from the box (flake.nix pins it beside our
/// own wasm). Loaded the first time a film is opened and not before: it is
/// half a megabyte and most visits never play anything.
let player = null;
export async function playlistPlayer() {
  if (!player) {
    // a classic script, not an import: the file is a UMD bundle and its
    // wrapper wants `this` to be the window, which a module denies it
    await new Promise((ok, no) => {
      const s = document.createElement('script');
      s.src = '/_dd/web/hls.js';
      s.onload = ok;
      s.onerror = () => no(new Error('the player did not load'));
      document.head.append(s);
    });
    player = window.Hls;
  }
  if (!player || !player.isSupported()) throw new Error('this browser cannot play a film');
  return player;
}

export function human(n) {
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n < 10 && i ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
}

/// a file or folder to a new name or place: the gate renames what the
/// bucket holds; nothing is read, and nothing is sealed again
export async function move(lib, from, to) {
  const r = await fetch(dav(lib, from), {
    method: 'MOVE',
    headers: { destination: location.origin + dav(lib, to), overwrite: 'F' },
  });
  if (!r.ok) throw new Error(`the gate said ${r.status}`);
}

// the trash keeps its own names in the clear (trash/<when>/...) and the
// library's below them, as they were
function trashUrl(lib, stamp, enc) {
  return `/_dd/dav/${lib.id}/trash/${stamp}${enc ? '/' + enc.split('/').map(encodeURIComponent).join('/') : ''}`;
}

async function propfind(url) {
  const r = await fetch(url + '/', { method: 'PROPFIND', headers: { depth: '1' } });
  if (r.status === 404) return [];
  if (r.status !== 207) throw new Error(`the gate said ${r.status}`);
  const doc = new DOMParser().parseFromString(await r.text(), 'application/xml');
  return [...doc.getElementsByTagNameNS('DAV:', 'response')].map((el) => ({
    href: decodeURI(el.getElementsByTagNameNS('DAV:', 'href')[0]?.textContent || ''),
    dir: !!el.getElementsByTagNameNS('DAV:', 'collection').length,
    sealed: Number(el.getElementsByTagNameNS('DAV:', 'getcontentlength')[0]?.textContent || 0),
  }));
}

/// What is in the trash: every file with where it was, when it went there,
/// and its size. The box lets it go for good 30 days on.
export async function trashed(lib) {
  const base = `/_dd/dav/${lib.id}/trash/`;
  const out = [];
  const walk = async (stamp, enc) => {
    for (const e of await propfind(trashUrl(lib, stamp, enc))) {
      const rel = e.href.replace(/\/$/, '').slice((base + stamp + '/').length);
      if (!rel || rel === enc) continue;
      const encRel = rel.split('/').map(decodeURIComponent).join('/');
      if (e.dir) { await walk(stamp, encRel); continue; }
      let path;
      try { path = path_decrypt(lib.key, lib.id, encRel); } catch { continue; }
      out.push({ stamp, enc: encRel, path, name: path.split('/').pop(), size: plain_size(e.sealed), when: new Date(Number(stamp) * 1000) });
    }
  };
  for (const e of await propfind(base.slice(0, -1))) {
    const stamp = e.href.replace(/\/$/, '').split('/').pop();
    if (e.dir && /^\d+$/.test(stamp)) await walk(stamp, '');
  }
  return out.sort((a, b) => b.when - a.when);
}

/// back where it was; beside it, under a new name, if that place is taken
export async function restore(lib, item, to = item.path) {
  const r = await fetch(trashUrl(lib, item.stamp, item.enc), {
    method: 'MOVE',
    headers: { destination: location.origin + dav(lib, to), overwrite: 'F' },
  });
  if (!r.ok) throw new Error(`the gate said ${r.status}`);
}

export async function mkdir(lib, path) {
  const r = await fetch(dav(lib, path), { method: 'MKCOL' });
  if (!r.ok && r.status !== 405) throw new Error(`the gate said ${r.status}`);
}
