// What Movies & TV knows about a film beyond its name: its art, a year, a
// line about it, and where each viewer stopped. The pictures live in the
// library under .commonty/art/; the rest is a note the gate keeps beside
// the library (the shelf, and each viewer's place), so a place saved every
// half minute is not a file left in the trash each time. All of it is
// sealed with the library's key, so the box holds none of it in the clear.
// A poster comes from TMDB, asked from this device with nothing but the
// title and year, once per title; a film TMDB does not know gets a still
// from its own frames instead.

import { fetchPlain, put, mkdir, trash, note, putNote } from './library.js';

const DIR = '.commonty';
// TMDB's terms: nothing of theirs kept longer than six months. It is asked
// again after five, and past six a title shows only what is ours (a still
// from the film) until it has been.
const DAY = 24 * 3600 * 1000;
const REFRESH = 150 * DAY;
const EXPIRE = 180 * DAY;
const age = (it) => (it?.looked ? Date.now() - Date.parse(it.looked) : Infinity);
// where an older page kept them, as files in the library
const SHELF = `${DIR}/shelf.json`;
const PROGRESS = `${DIR}/progress.json`;

/// "Charade (1963).mkv", "Bonanza.S01E04.The.Paiute.War.1080p.mkv"
export function parse(name) {
  const base = name.replace(/\.[a-z0-9]{2,4}$/i, '');
  const ep = base.match(/[Ss](\d{1,2})[ ._-]*[Ee](\d{1,3})|\b(\d{1,2})x(\d{2,3})\b/);
  const year = base.match(/[(\[ ._-]((?:19|20)\d{2})(?=[)\] ._-]|$)/);
  const junk = base.match(/[ ._-](2160p|1080p|720p|480p|blu-?ray|web-?dl|webrip|hdtv|dvdrip|x26[45]|hevc|h\.?26[45])\b/i);
  const cuts = [year?.index, ep?.index, junk?.index].filter((i) => i != null && i > 0);
  const clean = (s) => s.replace(/[._]+/g, ' ').replace(/[\s([-]+$/, '').replace(/^[\s)\]-]+/, '').trim();
  const title = clean(cuts.length ? base.slice(0, Math.min(...cuts)) : base) || base;
  let episodeTitle = '';
  if (ep) {
    const rest = base.slice(ep.index + ep[0].length);
    const stop = rest.search(/[ ._-](2160p|1080p|720p|480p|blu-?ray|web-?dl|webrip|hdtv|x26[45]|hevc)\b/i);
    episodeTitle = clean(stop >= 0 ? rest.slice(0, stop) : rest);
  }
  return {
    title,
    year: year ? Number(year[1]) : null,
    season: ep ? Number(ep[1] || ep[3]) : null,
    episode: ep ? Number(ep[2] || ep[4]) : null,
    episodeTitle,
  };
}

async function readJSON(lib, path, empty) {
  try {
    return JSON.parse(new TextDecoder().decode(await fetchPlain(lib, path)));
  } catch {
    return empty;
  }
}

// One write at a time, and a burst of changes as one: a shelf filling in
// its posters would otherwise send the whole note once per poster.
function writer(save) {
  let timer = null;
  let chain = Promise.resolve();
  return (value) => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      chain = chain.then(() => save(value)).catch(() => {});
    }, 1500);
  };
}

// A note from the gate; the first time, what an older page kept instead
// (a file in the library, a reader's place in this browser) is moved
// there and the old copy let go. The gate out of reach: { offline }.
async function load(lib, kind, old) {
  let value;
  try {
    value = await note(lib, kind);
  } catch {
    return { offline: true };
  }
  if (value || !old) return { value };
  const was = await old.read();
  if (was) {
    try {
      await putNote(lib, kind, was);
      await old.drop();
    } catch { /* moved next visit */ }
  }
  return { value: was };
}

/// The shelf's store: what is known about each title, and where each one
/// was left. The shelf is the owner's, and a reader reads it; each viewer
/// keeps their own place. With the gate out of reach the page still
/// works: the shelf as an older page left it, and the place in this
/// browser until the gate answers again.
export async function open(lib) {
  const owner = !lib.reader;
  const inLibrary = (path) => ({ read: () => readJSON(lib, path, null), drop: () => trash(lib, path) });
  const inBrowser = { read: async () => local(lib), drop: async () => forgetLocal(lib) };
  // the demo keeps nothing at the gate
  const [shelf, place] = lib.demo
    ? [{ offline: true }, { offline: true }]
    : await Promise.all([
      load(lib, 'shelf', owner ? inLibrary(SHELF) : null),
      load(lib, 'progress', owner ? inLibrary(PROGRESS) : inBrowser),
    ]);
  // a reader whose owner has not opened Movies & TV since, or no gate
  const meta = shelf.value || (owner && !shelf.offline ? null : await readJSON(lib, SHELF, null)) || { items: {} };
  meta.items ||= {};
  const saveMeta = owner && !shelf.offline ? writer((m) => putNote(lib, 'shelf', m)) : () => {};
  const progress = (place.offline ? local(lib) : place.value) || {};
  const saveProgress = place.offline ? (p) => keepLocal(lib, p) : writer((p) => putNote(lib, 'progress', p));
  const art = new Map();
  let dirs = null;
  return {
    item: (path) => (meta.items[path] ||= {}),
    /// what may be shown: TMDB's part only while it is under six months old
    known: (path) => {
      const it = meta.items[path];
      if (!it || !it.tmdb || age(it) < EXPIRE) return it;
      return { still: it.still };
    },
    /// whether anything on the page came from TMDB (for its credit)
    fromTmdb: () => Object.values(meta.items).some((it) => it.tmdb && age(it) < EXPIRE),
    changed: () => saveMeta(meta),
    progress,
    moved: () => saveProgress(progress),

    /// a picture kept in the library: an object url, fetched once a visit
    async picture(path) {
      if (!path) return null;
      if (!art.has(path)) {
        art.set(path, fetchPlain(lib, path).then((b) => URL.createObjectURL(new Blob([b], { type: 'image/jpeg' }))).catch(() => null));
      }
      return art.get(path);
    },

    /// a picture into the library, named for what it shows
    async keep(key, kind, blob) {
      if (!blob) return null;
      // a library this viewer only reads: the picture stays in this tab
      if (lib.reader) {
        const path = `tab:${kind}:${key}`;
        art.set(path, Promise.resolve(URL.createObjectURL(blob)));
        return path;
      }
      const h = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(`${kind}:${key}`)));
      const name = [...h.slice(0, 8)].map((b) => b.toString(16).padStart(2, '0')).join('');
      const path = `${DIR}/art/${name}.jpg`;
      dirs ||= mkdir(lib, DIR).then(() => mkdir(lib, `${DIR}/art`));
      await dirs;
      await put(lib, path, blob);
      art.set(path, Promise.resolve(URL.createObjectURL(blob)));
      return path;
    },
  };
}

// a place kept in this browser: the demo's, or anyone's while the gate is away
function local(lib) {
  try { return JSON.parse(localStorage.getItem(`dd-progress-${lib.id}`) || 'null'); } catch { return null; }
}

function keepLocal(lib, p) {
  try { localStorage.setItem(`dd-progress-${lib.id}`, JSON.stringify(p)); } catch { /* private window */ }
}

function forgetLocal(lib) {
  try { localStorage.removeItem(`dd-progress-${lib.id}`); } catch { /* private window */ }
}

// TMDB: a read token (long) goes in a header, an api key (short) in the url
async function tmdb(key, path, params = {}) {
  const u = new URL(`https://api.themoviedb.org/3${path}`);
  for (const [k, v] of Object.entries(params)) if (v != null && v !== '') u.searchParams.set(k, v);
  const headers = {};
  if (key.length > 60) headers.authorization = `Bearer ${key}`;
  else u.searchParams.set('api_key', key);
  const r = await fetch(u, { headers, referrerPolicy: 'no-referrer', credentials: 'omit' });
  if (!r.ok) throw new Error(`TMDB said ${r.status}`);
  return r.json();
}

async function image(path, size) {
  if (!path) return null;
  const r = await fetch(`https://image.tmdb.org/t/p/${size}${path}`, { referrerPolicy: 'no-referrer', credentials: 'omit' });
  return r.ok ? r.blob() : null;
}

/// A film or a show looked up once: its poster, a wide picture, the year
/// and a line about it, kept in the library. Not found is kept too, so
/// the next visit does not ask again.
export async function lookUp(store, key, path, { title, year }, kind) {
  const it = store.item(path);
  if (age(it) < REFRESH) return false;
  const found = await tmdb(key, kind === 'show' ? '/search/tv' : '/search/movie', {
    query: title,
    [kind === 'show' ? 'first_air_date_year' : 'year']: year,
    include_adult: 'false',
  });
  const hit = found.results?.[0];
  it.looked = new Date().toISOString();
  // asked again: whatever it said before goes, the seasons with it
  for (const k of ['tmdb', 'title', 'year', 'overview', 'poster', 'backdrop', 'seasons']) delete it[k];
  if (hit) {
    it.tmdb = hit.id;
    it.title = hit.title || hit.name || title;
    it.year = Number((hit.release_date || hit.first_air_date || '').slice(0, 4)) || year || null;
    it.overview = hit.overview || '';
    it.poster = await store.keep(path, 'poster', await image(hit.poster_path, 'w500'));
    it.backdrop = await store.keep(path, 'backdrop', await image(hit.backdrop_path, 'w780'));
  }
  store.changed();
  return !!hit;
}

/// a season's episodes by number: their names, a line, and a still each
export async function lookUpSeason(store, key, showPath, season) {
  const show = store.item(showPath);
  if (!show.tmdb) return null;
  show.seasons ||= {};
  if (show.seasons[season]) return show.seasons[season];
  const s = await tmdb(key, `/tv/${show.tmdb}/season/${season}`);
  const eps = {};
  for (const e of s.episodes || []) {
    eps[e.episode_number] = {
      name: e.name || '',
      overview: e.overview || '',
      runtime: e.runtime || null,
      still: await store.keep(`${showPath}#${season}x${e.episode_number}`, 'still', await image(e.still_path, 'w300')),
    };
  }
  show.seasons[season] = eps;
  store.changed();
  return eps;
}

/// a frame from a video element that is playing or paused on one
export function frame(video) {
  if (!video.videoWidth) return Promise.resolve(null);
  const w = Math.min(780, video.videoWidth);
  const c = document.createElement('canvas');
  c.width = w;
  c.height = Math.round(w * video.videoHeight / video.videoWidth);
  c.getContext('2d').drawImage(video, 0, 0, c.width, c.height);
  return new Promise((ok) => c.toBlob(ok, 'image/jpeg', 0.82));
}

/// A still from a file about to go into the library, taken here while it
/// is still in the clear: a tenth of the way in, and not past five minutes.
/// A file this browser cannot decode gives none, and the page draws a card.
export async function stillFromFile(file) {
  const v = document.createElement('video');
  v.muted = true;
  v.preload = 'metadata';
  const url = URL.createObjectURL(file);
  const wait = (ev) => new Promise((ok, no) => {
    const t = setTimeout(() => no(new Error('timeout')), 15000);
    v.addEventListener(ev, () => { clearTimeout(t); ok(); }, { once: true });
    v.addEventListener('error', () => { clearTimeout(t); no(new Error('decode')); }, { once: true });
  });
  try {
    v.src = url;
    await wait('loadedmetadata');
    v.currentTime = Math.min((v.duration || 60) * 0.1, 300);
    await wait('seeked');
    return await frame(v);
  } catch {
    return null;
  } finally {
    v.removeAttribute('src');
    URL.revokeObjectURL(url);
  }
}

/// the device, for "paused on your laptop": what this is, roughly
export function thisDevice(inApp) {
  const ua = navigator.userAgent;
  const os = /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iPhone' : /Windows/.test(ua) ? 'Windows' : /Mac OS/.test(ua) ? 'a Mac' : /Linux/.test(ua) ? 'Linux' : 'a device';
  if (inApp) return `the app on ${os}`;
  const b = /Firefox\//.test(ua) ? 'Firefox' : /Edg\//.test(ua) ? 'Edge' : /Chrome\//.test(ua) ? 'Chrome' : /Safari\//.test(ua) ? 'Safari' : 'a browser';
  return `${b} on ${os}`;
}
