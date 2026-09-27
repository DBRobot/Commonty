// Movies & TV: shelves of films and shows, with posters, and a player that
// picks up where you stopped. Films are files under Movies/; a show is a
// folder under Shows/ with its episodes inside, or inside season folders.
// Everything is opened in this tab (library.js); what the page knows
// about each title beyond its name is kept, sealed, in the library too
// (shelf.js).

import { unlock, list, fetchPlain, save, put, trash, mkdir, transcode, stopTranscode, playsPlaylists, playlistPlayer, human } from './library.js';
import { parse, open, lookUp, lookUpSeason, frame, stillFromFile, thisDevice } from './shelf.js';
import { me, inApp } from './shell.js';

const $ = (id) => document.getElementById(id);
const { user, tmdb } = await me();
// A film too big for the tab plays through the box; a small file that the
// box cannot take plays from the tab itself.
const INLINE = 256 * 1024 * 1024;
const WHERE = thisDevice(inApp);

let lib = null;
let store = null;
let films = [];
let shows = [];
let show = null;                 // the show open, and its episodes

const el = (tag, props = {}, ...kids) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else e.setAttribute(k, v);
  }
  e.append(...kids);
  return e;
};

const minutes = (s) => {
  const m = Math.round(s / 60);
  return m >= 60 ? `${Math.floor(m / 60)} h ${String(m % 60).padStart(2, '0')} m` : `${m} m`;
};

// ---- what a title is called and looks like

function titleOf(it) {
  const k = store.known(it.path);
  return k?.title || it.parsed.title;
}
function yearOf(it) {
  return store.known(it.path)?.year || it.parsed.year || '';
}

// A card's picture: the kept art when there is some, and until then (or
// for good) a card drawn from the title, the same colours every visit.
function drawn(title) {
  let h = 0;
  for (const c of title) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  const hues = [212, 350, 160, 32, 262, 196, 312, 96];
  const a = hues[h % hues.length];
  const d = el('div', { class: 'drawn' }, el('b', { text: title }));
  d.style.setProperty('--h', a);
  return d;
}

async function paint(pic, paths, title) {
  pic.replaceChildren(drawn(title));
  for (const p of paths) {
    const url = await store.picture(p);
    if (url) {
      pic.replaceChildren(el('img', { src: url, alt: '', loading: 'lazy' }));
      return;
    }
  }
}

function progressBar(fraction) {
  const bar = el('div', { class: 'progress' }, el('i'));
  bar.firstChild.style.width = `${Math.round(Math.min(1, fraction) * 100)}%`;
  return bar;
}

// ---- the shelves

function card(it, shape) {
  const k = store.known(it.path) || {};
  const b = el('button', { type: 'button', class: 'card' });
  const pic = el('div', { class: 'pic' });
  const art = shape === 'wide'
    ? [k.backdrop, k.still, k.poster]
    : [k.poster, k.still];
  paint(pic, art, titleOf(it));
  const p = store.progress[it.path];
  if (p && !p.done && p.of) pic.append(progressBar(p.at / p.of));
  if (it.fresh) pic.append(el('span', { class: 'badge', text: 'New' }));
  const sub = it.kind === 'show'
    ? [yearOf(it), it.count ? `${it.count} episode${it.count === 1 ? '' : 's'}` : ''].filter(Boolean).join(' · ')
    : shape === 'wide' && p?.of ? `${minutes(p.of - p.at)} left` : String(yearOf(it) || human(it.size));
  b.append(pic, el('span', { class: 't', text: it.label || titleOf(it) }), el('span', { class: 's', text: sub }));
  b.onclick = () => (it.kind === 'show' ? openShow(it) : play(it, { resume: true }));
  return b;
}

function shelf(id, items, shape) {
  $(id).replaceChildren(...items.map((it) => card(it, shape)));
  $(`${id}-count`).textContent = items.length;
  $(`${id}-shelf`).hidden = !items.length;
}

// what was being watched, newest first; an episode stands for its show
function continuing() {
  const byPath = new Map(films.map((f) => [f.path, f]));
  for (const s of shows) for (const e of s.episodes || []) byPath.set(e.path, e);
  return Object.entries(store.progress)
    .filter(([path, p]) => !p.done && byPath.has(path) && p.at > 30)
    .sort((a, b) => (b[1].when || '').localeCompare(a[1].when || ''))
    .map(([path]) => byPath.get(path));
}

function hero() {
  const next = continuing()[0];
  const newest = [...films].sort((a, b) => b.added - a.added)[0];
  const it = next || newest;
  $('hero').hidden = !it;
  if (!it) return;
  const k = store.known(it.path) || {};
  const p = store.progress[it.path];
  const resuming = !!(next && p);
  $('hero-eyebrow').textContent = resuming ? 'Continue watching' : 'Recently added';
  $('hero-title').textContent = it.label || titleOf(it);
  const meta = [yearOf(it), p?.of ? minutes(p.of) : '', resuming && p.of ? `${minutes(p.of - p.at)} left` : ''].filter(Boolean);
  $('hero-meta').replaceChildren(...meta.map((m) => el('span', { text: String(m) })));
  $('hero-progress').hidden = !(resuming && p.of);
  if (resuming && p.of) $('hero-progress').firstChild.style.width = `${Math.round((p.at / p.of) * 100)}%`;
  $('hero-text').textContent = resuming
    ? `Stopped ${minutes(p.at)} in${p.where ? `, on ${p.where}` : ''}.`
    : (k.overview || '');
  $('hero-play').textContent = resuming ? 'Resume' : 'Play';
  $('hero-play').onclick = () => play(it, { resume: true });
  $('hero-over').hidden = !resuming;
  $('hero-over').onclick = () => play(it, { resume: false });
  paint($('hero-art'), [k.backdrop, k.still, k.poster], titleOf(it));
}

function render() {
  hero();
  shelf('continue', continuing(), 'wide');
  shelf('recent', [...films].sort((a, b) => b.added - a.added).slice(0, 12), 'poster');
  shelf('shows', shows, 'poster');
  shelf('films', [...films].sort((a, b) => titleOf(a).localeCompare(titleOf(b))), 'poster');
  $('empty').hidden = !!(films.length || shows.length);
  $('tmdb').hidden = !store.fromTmdb();
  tab(current);
}

let current = 'all';
function tab(t) {
  current = t;
  for (const b of document.querySelectorAll('[data-tab]')) b.setAttribute('aria-pressed', String(b.dataset.tab === t));
  for (const s of document.querySelectorAll('#home .shelf')) {
    const items = s.querySelector('.row').children.length;
    s.hidden = !items || (t !== 'all' && s.dataset.kind !== t);
  }
  $('hero').hidden = $('hero').hidden || t === 'shows';
  if (t === 'all') hero();
}

// ---- reading the library

async function episodesOf(name) {
  const top = await list(lib, `Shows/${name}`);
  const files = top.filter((i) => !i.dir);
  // season folders, one level down
  for (const d of top.filter((i) => i.dir)) {
    const n = Number((d.name.match(/(\d+)/) || [])[1]) || null;
    for (const f of (await list(lib, d.path)).filter((i) => !i.dir)) {
      f.seasonHint = n;
      files.push(f);
    }
  }
  return files.map((f) => {
    const parsed = parse(f.name);
    parsed.season ??= f.seasonHint ?? 1;
    return { ...f, kind: 'episode', parsed, show: name };
  }).sort((a, b) => (a.parsed.season - b.parsed.season) || ((a.parsed.episode ?? 999) - (b.parsed.episode ?? 999)) || a.name.localeCompare(b.name));
}

async function load() {
  const week = Date.now() - 7 * 24 * 3600 * 1000;
  films = (await list(lib, 'Movies')).filter((i) => !i.dir).map((f) => ({
    ...f, kind: 'film', parsed: parse(f.name), added: Date.parse(f.modified) || 0,
  }));
  for (const f of films) f.fresh = f.added > week;
  shows = await Promise.all((await list(lib, 'Shows')).filter((i) => i.dir).map(async (d) => {
    const episodes = await episodesOf(d.name);
    const name = parse(d.name).title;
    for (const e of episodes) {
      e.label = `${name} · S${e.parsed.season} E${e.parsed.episode ?? '?'}`;
    }
    return { ...d, kind: 'show', parsed: parse(d.name), episodes, count: episodes.length };
  }));
}

// Posters, once per title, a few at a time after the shelves are up. Only
// the library's owner asks: a reader sees what the owner's devices kept.
async function fillIn() {
  if (!tmdb || lib.reader) return;
  // lookUp passes over what it asked recently, and asks again what is due
  for (const it of [...films, ...shows]) {
    try {
      if (await lookUp(store, tmdb, it.path, it.parsed, it.kind) && !show) render();
    } catch (e) {
      // TMDB unreachable or the key refused: the drawn cards stay, and the
      // next visit tries again
      if (/401|403/.test(e.message)) return;
    }
  }
}

// ---- a show

async function openShow(it) {
  show = it;
  $('home').hidden = true;
  $('show-page').hidden = false;
  $('actions').hidden = true;
  const k = store.known(it.path) || {};
  $('show-title').textContent = titleOf(it);
  $('show-meta').replaceChildren(...[yearOf(it), `${it.count} episode${it.count === 1 ? '' : 's'}`].filter(Boolean).map((m) => el('span', { text: String(m) })));
  $('show-text').textContent = k.overview || '';
  paint($('show-poster'), [k.poster, k.still], titleOf(it));
  $('addep').hidden = !!lib.reader;
  const seasons = [...new Set(it.episodes.map((e) => e.parsed.season))];
  const s = seasons.find((n) => it.episodes.some((e) => e.parsed.season === n && !store.progress[e.path]?.done)) ?? seasons[0];
  $('seasons').replaceChildren(...seasons.map((n) => {
    const b = el('button', { type: 'button', 'aria-pressed': String(n === s), text: `Season ${n}` });
    b.onclick = () => season(n);
    return b;
  }));
  $('seasons').hidden = seasons.length < 2;
  await season(s ?? 1);
  scrollTo(0, 0);
}

async function season(n) {
  for (const b of $('seasons').children) b.setAttribute('aria-pressed', String(b.textContent === `Season ${n}`));
  const eps = show.episodes.filter((e) => e.parsed.season === n);
  let names = store.known(show.path)?.seasons?.[n] || null;
  const row = (e) => {
    const info = names?.[e.parsed.episode] || {};
    const p = store.progress[e.path];
    const still = el('span', { class: 'still' });
    paint(still, [info.still, store.known(e.path)?.still], e.parsed.episodeTitle || show.name);
    const state = p?.done ? 'Watched' : p?.of && p.at > 30 ? `Watching · ${minutes(p.of - p.at)} left` : '';
    const b = el('button', { type: 'button', class: 'ep' },
      el('span', { class: 'n', text: e.parsed.episode ?? '·' }),
      still,
      el('span', {},
        el('span', { class: 't', text: info.name || e.parsed.episodeTitle || e.name }),
        el('span', { class: 'd', text: state || info.overview || '' })),
      el('span', { class: 'len', text: info.runtime ? `${info.runtime} m` : human(e.size) }));
    b.onclick = () => play(e, { resume: true });
    return el('li', {}, b);
  };
  $('eps').replaceChildren(...eps.map(row));
  // the season's names and stills, looked up once
  if (!names && tmdb && !lib.reader && store.known(show.path)?.tmdb) {
    try {
      names = await lookUpSeason(store, tmdb, show.path, n);
      if (names && show && eps[0]?.show === show.name) $('eps').replaceChildren(...eps.map(row));
    } catch { /* the filenames will do */ }
  }
}

function home() {
  show = null;
  $('show-page').hidden = true;
  $('home').hidden = false;
  $('actions').hidden = false;
  render();
}

// ---- playing

let hls = null;
let session = null;
let playing = null;              // { it, offset, duration, lastSave, stilled }

async function stop() {
  const v = $('video');
  if (playing) remember(true);
  v.pause();
  v.removeAttribute('src');
  v.load();
  hls?.destroy();
  hls = null;
  playing = null;
  if (session) {
    const gone = session;
    session = null;
    await stopTranscode(gone);
  }
}

// where it got to: every half minute while it plays, and when it stops
function remember(now) {
  if (!playing) return;
  const v = $('video');
  const at = playing.offset + (v.currentTime || 0);
  const of = playing.duration || (Number.isFinite(v.duration) ? playing.offset + v.duration : null);
  if (at < 5) return;
  if (!now && Date.now() - playing.lastSave < 30000) return;
  playing.lastSave = Date.now();
  store.progress[playing.it.path] = {
    at: Math.round(at),
    of: of ? Math.round(of) : null,
    when: new Date().toISOString(),
    where: WHERE,
    done: !!(of && (at > of * 0.95 || of - at < 120)),
  };
  store.moved();
}

// a still for a title with no picture: a frame a minute into watching
async function stillFrom(v) {
  const it = playing?.it;
  if (!it || playing.stilled || lib.reader) return;
  if (v.currentTime < 60) return;
  playing.stilled = true;
  const k = store.known(it.path) || {};
  if (k.poster || k.still) return;
  const path = await store.keep(it.path, 'still', await frame(v));
  if (path) {
    store.item(it.path).still = path;
    store.changed();
  }
}

async function play(it, { resume }) {
  const v = $('video');
  await stop();
  const p = store.progress[it.path];
  const from = resume && p && !p.done ? p.at : 0;
  $('player').hidden = false;
  $('p-title').textContent = it.label ? `${it.label}${it.parsed.episodeTitle ? ` · ${it.parsed.episodeTitle}` : ''}` : titleOf(it);
  $('p-sub').textContent = from ? `Picking up at ${minutes(from)}` : 'Unlocked on this device';
  $('p-note').textContent = 'Asking the box to play it…';
  $('p-save').hidden = true;
  $('p-remove').hidden = !!lib.reader;
  $('p-remove').onclick = async () => {
    if (!confirm(`Remove ${titleOf(it)} from the library?`)) return;
    await stop();
    await trash(lib, it.path);
    $('player').hidden = true;
    await reload();
  };
  $('p-close').focus();
  playing = { it, offset: 0, duration: null, lastSave: Date.now(), stilled: false };
  // A film is bigger than a tab: the box decrypts it in its own memory for
  // this one viewing and sends a playlist. Safari plays a playlist itself;
  // everywhere else the page loads a player from the box.
  try {
    const s = await transcode(lib, it.path, it.sealed, from);
    session = s.url;
    playing.offset = s.from;
    playing.duration = s.duration;
    // Our player wherever it runs (every desktop browser, Android), the
    // browser's own only where it cannot (an iPhone). Chrome plays a
    // playlist by itself now, and its own player took one still growing
    // for a live broadcast: it chased the newest piece, then went back to
    // the start once the box had packed the whole film.
    const Hls = await playlistPlayer().catch(() => null);
    if (!Hls && playsPlaylists()) {
      v.src = s.url;
    } else {
      if (!Hls) throw new Error('this browser cannot play a film');
      // the playlist grows as the box works: a moment's 404 or a slow
      // segment is waiting, not failing
      // and from its start: a playlist still growing looks like a live
      // broadcast, which a player joins near the newest piece instead
      hls = new Hls({ startPosition: 0, manifestLoadingMaxRetry: 6, levelLoadingMaxRetry: 6, fragLoadingMaxRetry: 6 });
      hls.on(Hls.Events.ERROR, (_, d) => {
        if (d.fatal) $('p-note').textContent = `The player stopped: ${d.details}. Close it and press play again.`;
      });
      hls.loadSource(s.url);
      hls.attachMedia(v);
    }
    $('p-note').textContent = '';
    v.play().catch(() => {});
    return;
  } catch (e) {
    playing.boxSaid = e.message;
    // and fall through: a small file still opens in the tab
  }
  if (it.size > INLINE) {
    // what the box said, not only that it could not
    $('p-note').textContent = `The box could not play this (${playing.boxSaid}), and at ${human(it.size)} it is too big to open in the tab. Save it, or open it from the desktop app.`;
    $('p-save').hidden = false;
    $('p-save').onclick = () => fetchPlain(lib, it.path).then((b) => save(b, it.name));
    return;
  }
  $('p-note').textContent = 'Opening…';
  try {
    const plain = await fetchPlain(lib, it.path);
    v.src = URL.createObjectURL(new Blob([plain]));
    if (from) v.addEventListener('loadedmetadata', () => { v.currentTime = from; }, { once: true });
    $('p-note').textContent = '';
    v.play().catch(() => {});
  } catch (e) {
    $('p-note').textContent = e.message;
  }
}

async function closePlayer() {
  await stop();
  $('player').hidden = true;
  if (show) await season(Number($('seasons').querySelector('[aria-pressed="true"]')?.textContent.replace('Season ', '')) || show.episodes[0]?.parsed.season || 1);
  else render();
}

// ---- adding

async function upload(files, into) {
  const msg = $('uploading');
  msg.hidden = false;
  for (const f of files) {
    const path = `${into}/${f.name}`;
    msg.textContent = `${f.name}: taking a still`;
    const still = await stillFromFile(f);
    try {
      await put(lib, path, f, (done, total) => {
        msg.textContent = `${f.name}: encrypting and sending, ${Math.round((done / total) * 100)}%`;
      });
      if (still) {
        store.item(path).still = await store.keep(path, 'still', still);
        store.changed();
      }
      msg.textContent = `${f.name}: added, ${human(f.size)}`;
    } catch (e) {
      msg.textContent = `${f.name}: ${e.message}`;
    }
  }
  await reload();
}

async function reload() {
  await load();
  if (show) {
    show = shows.find((s) => s.path === show.path) || null;
    if (show) return openShow(show);
    return home();
  }
  render();
  fillIn();
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
  $('msg').textContent = 'Opening the library…';
  store = await open(lib);
  await load();
  $('msg').hidden = true;
  $('home').hidden = false;
  $('actions').hidden = false;
  for (const id of ['addfilm', 'newshow']) $(id).hidden = !!lib.reader;
  for (const b of document.querySelectorAll('[data-tab]')) b.onclick = () => tab(b.dataset.tab);
  $('addfilm').onclick = () => $('filmpicker').click();
  $('filmpicker').onchange = () => upload([...$('filmpicker').files], 'Movies');
  $('addep').onclick = () => $('eppicker').click();
  $('eppicker').onchange = () => upload([...$('eppicker').files], `Shows/${show.name}`);
  $('newshow').onclick = async () => {
    const name = prompt('What is the show called?');
    if (!name) return;
    await mkdir(lib, 'Shows');
    await mkdir(lib, `Shows/${name}`);
    await load();
    const s = shows.find((x) => x.name === name);
    if (s) openShow(s);
  };
  $('back').onclick = home;
  $('p-close').onclick = closePlayer;
  const v = $('video');
  v.addEventListener('timeupdate', () => { remember(false); stillFrom(v); });
  v.addEventListener('pause', () => remember(true));
  v.addEventListener('ended', () => remember(true));
  addEventListener('keydown', (e) => { if (e.key === 'Escape' && !$('player').hidden) closePlayer(); });
  // a tab closed mid-film keeps its place, and the box stops working on it
  addEventListener('pagehide', () => {
    remember(true);
    if (session) stopTranscode(session, true);
  });
  render();
  fillIn();
}

start().catch((e) => { $('msg').hidden = false; $('msg').textContent = String(e.message || e); });
