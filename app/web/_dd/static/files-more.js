// What Files does with more than one file at a time: folders and selections
// out as one zip, whole folders in from a drop, every name in the library
// for search, and how much a folder holds. All of it in the tab: names are
// opened here, bytes are opened and sealed here, the gate moves ciphertext.

import { list, walk, pieces, put, mkdir, save } from './library.js';
import { ZipWriter } from './zip.js';

const ROOT = 'Files';
// the most a zip may be when the browser cannot write it straight to disk
const IN_TAB_MAX = 2 * 2 ** 30;

/// A zip of `items` (files and folders of the library), written as it goes
/// to a file the person picks where the browser allows it, else gathered in
/// pieces a browser keeps on disk when they are large. progress(done bytes,
/// total bytes). Names inside are as the folder they were picked in shows.
export async function zipOf(lib, items, name, progress, signal) {
  let sink = null;
  const picker = !!window.showSaveFilePicker;
  if (picker) {
    try {
      const w = await (await window.showSaveFilePicker({ suggestedName: name })).createWritable();
      sink = { write: (b) => w.write(b), close: () => w.close(), abort: () => w.abort().catch(() => {}) };
    } catch (e) {
      if (e.name === 'AbortError') return false;
    }
  }
  if (!sink) {
    const kept = [];
    sink = {
      write: async (b) => { kept.push(new Blob([b])); },
      close: async () => save(new Blob(kept, { type: 'application/zip' }), name),
      abort: async () => { kept.length = 0; },
    };
  }
  try {
    // each picked thing with everything under it, named from where it was picked
    const base = (p) => p.split('/').slice(0, -1).join('/');
    const all = [];
    for (const it of items) {
      const from = base(it.path);
      all.push({ ...it, rel: it.path.slice(from.length + 1) });
      if (it.dir) for (const e of await walk(lib, it.path)) all.push({ ...e, rel: e.path.slice(from.length + 1) });
    }
    const total = all.reduce((n, e) => n + (e.dir ? 0 : e.size), 0);
    // without a picker the zip is gathered in the tab before it is saved
    if (!picker && total > IN_TAB_MAX) throw new Error(`that is ${Math.round(total / 2 ** 30)} GB: too much to put in a zip in this browser. Download it in parts, or use Chrome or Edge, which write a zip straight to disk`);
    let done = 0;
    const zip = new ZipWriter((b) => sink.write(b));
    const when = (s) => (s ? new Date(s) : new Date());
    async function* counted(e) {
      for await (const p of pieces(lib, e, signal)) {
        done += p.length;
        progress?.(done, total);
        yield p;
      }
    }
    for (const e of all) {
      if (signal?.aborted) throw new DOMException('stopped', 'AbortError');
      if (e.dir) await zip.folder(e.rel, when(e.modified));
      else await zip.file(e.rel, when(e.modified), counted(e));
    }
    await zip.end();
    await sink.close();
    progress?.(total, total);
    return true;
  } catch (e) {
    await sink.abort();
    throw e;
  }
}

/// The files a drop carries, folders opened: [{ file, rel }], rel being the
/// path under the folder dropped into ("Trip/day 1/a.jpg"). A browser
/// without folder entries gives the files alone.
export async function dropped(dt) {
  const entries = [...(dt.items || [])].map((i) => i.webkitGetAsEntry?.()).filter(Boolean);
  if (!entries.length) return [...dt.files].map((file) => ({ file, rel: file.name }));
  const out = [];
  const readAll = (dir) => new Promise((ok, no) => {
    const r = dir.createReader();
    const got = [];
    const more = () => r.readEntries((batch) => { if (!batch.length) ok(got); else { got.push(...batch); more(); } }, no);
    more();
  });
  const walkEntry = async (e, at) => {
    if (e.isFile) out.push({ file: await new Promise((ok, no) => e.file(ok, no)), rel: at + e.name });
    else if (e.isDirectory) {
      out.push({ dir: true, rel: at + e.name });
      for (const k of await readAll(e)) await walkEntry(k, `${at}${e.name}/`);
    }
  };
  for (const e of entries) await walkEntry(e, '');
  return out;
}

/// The files of an <input webkitdirectory>, as dropped() gives them
export const picked = (files) => [...files].map((file) => ({ file, rel: file.webkitRelativePath || file.name }));

/// Put what dropped() or picked() gave under `into` (a library path),
/// folders made on the way. job(name) gives { step(done, total, what),
/// done(text), fail(text) } for each file, as the Files tray does.
export async function putAll(lib, into, things, job) {
  const made = new Set();
  const folder = async (path) => {
    const parts = path.split('/');
    for (let k = 1; k <= parts.length; k++) {
      const p = parts.slice(0, k).join('/');
      if (made.has(p)) continue;
      await mkdir(lib, p);
      made.add(p);
    }
  };
  await folder(into);
  for (const t of things) {
    const clean = t.rel.split('/').filter((s) => s && s !== '.' && s !== '..').join('/');
    if (!clean) continue;
    if (t.dir) { await folder(`${into}/${clean}`); continue; }
    const dir = clean.includes('/') ? `${into}/${clean.split('/').slice(0, -1).join('/')}` : into;
    await folder(dir);
    const j = job(clean);
    try {
      await put(lib, `${into}/${clean}`, t.file, (d, n) => j.step(d, n, 'Encrypting and sending'));
      j.done('Done');
    } catch (e) {
      j.fail(e.message);
    }
  }
}

/// Every name in the member's Files, for finding one: walked once and kept
/// in this tab's memory alone (names are private), walked again when asked.
export class Names {
  constructor(lib) { this.lib = lib; this.all = null; this.at = 0; this.busy = null; }

  async load(fresh = false) {
    if (this.all && !fresh && Date.now() - this.at < 5 * 60e3) return this.all;
    this.busy ??= walk(this.lib, ROOT).then((all) => {
      this.all = all;
      this.at = Date.now();
      return all;
    }).finally(() => { this.busy = null; });
    return this.busy;
  }

  forget() { this.at = 0; }

  /// names with every word of `q` in them, folders first, the closest first
  async find(q, limit = 50) {
    const words = q.toLowerCase().split(/\s+/).filter(Boolean);
    if (!words.length) return [];
    const all = await this.load();
    const hits = [];
    for (const e of all) {
      const n = e.name.toLowerCase();
      if (!words.every((w) => n.includes(w))) continue;
      const rank = (n.startsWith(words[0]) ? 0 : 1) + (e.dir ? 0 : 0.5);
      hits.push({ e, rank });
    }
    hits.sort((a, b) => a.rank - b.rank || a.e.name.localeCompare(b.e.name, undefined, { numeric: true }));
    return hits.slice(0, limit).map((h) => h.e);
  }
}

/// How much a folder holds, everything under it: asked a few at a time and
/// remembered for this visit
const sizes = new Map();
const waiting = [];
let running = 0;
export function folderSize(lib, path) {
  if (sizes.has(path)) return sizes.get(path);
  const p = new Promise((ok) => waiting.push({ path, ok }));
  sizes.set(path, p);
  pump(lib);
  return p;
}
export function sizesChanged(path) {
  // the folder and every one above it
  for (const k of [...sizes.keys()]) if (path === k || path.startsWith(`${k}/`) || k.startsWith(`${path}/`)) sizes.delete(k);
}
function pump(lib) {
  while (running < 3 && waiting.length) {
    const { path, ok } = waiting.shift();
    running++;
    (async () => {
      let n = 0;
      try {
        const stack = [path];
        while (stack.length) {
          for (const e of await list(lib, stack.pop())) {
            if (e.dir) stack.push(e.path);
            else n += e.size;
          }
        }
        ok(n);
      } catch {
        sizes.delete(path);
        ok(null);
      } finally {
        running--;
        pump(lib);
      }
    })();
  }
}
