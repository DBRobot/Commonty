// What Movies & TV knows about a film beyond its name: its art, a year, a
// line about it, and where each viewer stopped. It all lives in the
// library under .commonty/, sealed like everything else there, so the box
// holds none of it in the clear. A poster comes from TMDB, asked from this
// device with nothing but the title and year, once per title; a film TMDB
// does not know gets a still from its own frames instead.

import { fetchPlain, put, mkdir } from './library.js';

const DIR = '.commonty';
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
// its posters would otherwise send the whole file once per poster.
function writer(lib, path) {
  let timer = null;
  let chain = Promise.resolve();
  let made = false;
  return (value) => {
    clearTimeout(timer);
    timer = setTimeout(() => {
      chain = chain.then(async () => {
        if (!made) { await mkdir(lib, DIR); made = true; }
        await put(lib, path, new Blob([JSON.stringify(value)]));
      }).catch(() => {});
    }, 1500);
  };
}

/// The shelf's store: what is known about each title, and where each one
/// was left. A reader of someone else's library keeps their place on this
/// device only; the library is not theirs to write to.
export async function open(lib) {
  const [meta, progress] = await Promise.all([
    readJSON(lib, SHELF, { items: {} }),
    lib.reader ? JSON.parse(local(lib) || '{}') : readJSON(lib, PROGRESS, {}),
  ]);
  meta.items ||= {};
  const saveMeta = lib.reader ? () => {} : writer(lib, SHELF);
  const saveProgress = lib.reader
    ? (p) => { try { localStorage.setItem(`dd-progress-${lib.id}`, JSON.stringify(p)); } catch { /* private window */ } }
    : writer(lib, PROGRESS);
  const art = new Map();
  let dirs = null;
  return {
    item: (path) => (meta.items[path] ||= {}),
    known: (path) => meta.items[path],
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
      if (lib.reader || !blob) return null;
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

function local(lib) {
  try { return localStorage.getItem(`dd-progress-${lib.id}`); } catch { return null; }
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
  if (it.looked) return false;
  const found = await tmdb(key, kind === 'show' ? '/search/tv' : '/search/movie', {
    query: title,
    [kind === 'show' ? 'first_air_date_year' : 'year']: year,
    include_adult: 'false',
  });
  const hit = found.results?.[0];
  it.looked = new Date().toISOString();
  if (hit) {
    it.tmdb = hit.id;
    it.title = hit.title || hit.name || title;
    it.year = Number((hit.release_date || hit.first_air_date || '').slice(0, 4)) || year || null;
    it.overview = hit.overview || '';
    it.poster = await store.keep(path, 'poster', await image(hit.poster_path, 'w342'));
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
