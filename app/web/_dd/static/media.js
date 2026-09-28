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
  const b = el('button', { type: 'button', class: 'card', 'data-path': it.path });
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
  b.onclick = () => openTitle(it);
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

// Posters, once per title, a few at a time after the shelves are up. The
// owner's are kept in the library; a viewer who only reads it (the demo,
// a shared library) looks up what the owner has not, and keeps that in
// this tab (shelf.js keep).
async function fillIn() {
  if (!tmdb) return;
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

// ---- a title, opened
//
// A card opens its title over the shelves, as Games opens a game: a band
// with the poster, what it is, and what to do. A show's seasons and
// episodes are in the same band, scrolling inside it, so there is no
// page of its own to go to and come back from.

let opened = null;               // the title in the band
let openedCard = null;           // the card it was opened from, for the way back

// the page behind the band, and the band behind the player, are out of
// reach of Tab and of a screen reader
function syncInert() {
  const player = !$('player').hidden;
  for (const e of document.querySelectorAll('main, header.dd-bar')) e.inert = player || !$('ribbon').hidden;
  $('ribbon').inert = player;
}

// the episode a show goes on with: the one being watched, or the one
// after the last finished, or the first
function nextEpisode(it) {
  const eps = it.episodes;
  const watching = eps
    .filter((e) => { const p = store.progress[e.path]; return p && !p.done && p.at > 30; })
    .sort((a, b) => (store.progress[b.path].when || '').localeCompare(store.progress[a.path].when || ''))[0];
  if (watching) return watching;
  let last = -1;
  eps.forEach((e, i) => { if (store.progress[e.path]?.done) last = i; });
  return eps[last + 1] || eps[0];
}

async function openTitle(it, { at } = {}) {
  // an episode on a shelf opens its show, at that episode
  if (it.kind === 'episode') {
    const s = shows.find((x) => x.name === it.show);
    return s ? openTitle(s, { at: it }) : play(it, { resume: true });
  }
  if ($('ribbon').hidden) openedCard = document.activeElement;
  opened = it;
  const isShow = it.kind === 'show';
  show = isShow ? it : null;
  const k = store.known(it.path) || {};
  const p = store.progress[it.path];
  $('ribbon').classList.toggle('show', isShow);
  paint($('r-poster'), [k.poster, k.still], titleOf(it));
  $('r-eyebrow').textContent = isShow ? 'Show' : 'Film';
  $('r-title').textContent = titleOf(it);
  const meta = isShow
    ? [yearOf(it), `${it.count} episode${it.count === 1 ? '' : 's'} here`]
    : [yearOf(it), p?.of ? minutes(p.of) : '', human(it.size)];
  $('r-meta').replaceChildren(...meta.filter(Boolean).map((m) => el('span', { text: String(m) })));
  $('r-text').textContent = k.overview || '';
  $('r-text').hidden = !k.overview;
  $('r-remove').hidden = !!lib.reader;
  $('addep').hidden = !isShow || !!lib.reader;
  $('seasons').hidden = !isShow;
  $('eps').hidden = !isShow;
  if (isShow) {
    const next = at || nextEpisode(it);
    const q = next && store.progress[next.path];
    const going = !!(q && !q.done && q.at > 30);
    const name = (e) => store.known(it.path)?.seasons?.[e.parsed.season]?.[e.parsed.episode]?.name || e.parsed.episodeTitle || '';
    $('r-where').hidden = true;
    $('r-play').textContent = next ? `${going ? 'Resume' : 'Play'} S${next.parsed.season} E${next.parsed.episode ?? '?'}` : 'Play';
    $('r-play').disabled = !next;
    $('r-play').onclick = () => next && play(next, { resume: true });
    $('r-spec').textContent = next ? [name(next), going && q.of ? `${minutes(q.of - q.at)} left` : ''].filter(Boolean).join(' · ') : '';
    $('r-over').hidden = true;
    $('r-remove').textContent = 'Remove show';
    $('r-remove').onclick = () => remove(it, `${titleOf(it)} and its ${it.count} episode${it.count === 1 ? '' : 's'}`);
    const seasons = [...new Set(it.episodes.map((e) => e.parsed.season))];
    const s = next?.parsed.season ?? seasons[0] ?? 1;
    $('seasons').replaceChildren(...seasons.map((n) => {
      const b = el('button', { type: 'button', 'aria-pressed': String(n === s), text: `Season ${n}` });
      b.onclick = () => season(n);
      return b;
    }));
    $('seasons').hidden = seasons.length < 2;
    show.next = next;
    await season(s);
  } else {
    const going = !!(p && !p.done && p.at > 30);
    $('r-where').hidden = !(going && p.of);
    if (going && p.of) {
      $('r-where').querySelector('i').style.width = `${Math.round((p.at / p.of) * 100)}%`;
      $('r-left').textContent = `${minutes(p.of - p.at)} left${p.where ? ` · stopped on ${p.where}` : ''}`;
    }
    $('r-play').disabled = false;
    $('r-play').textContent = going ? `Resume at ${clock(p.at)}` : 'Play';
    $('r-play').onclick = () => play(it, { resume: true });
    $('r-spec').textContent = '';
    $('r-over').hidden = !going;
    $('r-over').onclick = () => play(it, { resume: false });
    $('r-remove').textContent = 'Remove from library';
    $('r-remove').onclick = () => remove(it, titleOf(it));
  }
  if ($('ribbon').hidden) {
    $('veil').hidden = false;
    $('ribbon').hidden = false;
    $('ribbon').scrollTop = 0;
    syncInert();
    $('r-play').focus({ preventScroll: true });
  }
}

function closeTitle() {
  if ($('ribbon').hidden) return;
  $('ribbon').hidden = true;
  $('veil').hidden = true;
  opened = null;
  show = null;
  syncInert();
  render();
  const back = openedCard?.dataset?.path && [...document.querySelectorAll('main [data-path]')].find((e) => e.dataset.path === openedCard.dataset.path);
  (back || openedCard)?.focus?.();
}

async function remove(it, what) {
  if (!confirm(`Remove ${what} from the library? It goes to the trash in Files, where it can be put back.`)) return;
  await trash(lib, it.path);
  closeTitle();
  await reload();
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
    if (p && !p.done && p.of && p.at > 30) still.append(progressBar(p.at / p.of));
    const state = p?.done ? 'Watched' : '';
    const left = p && !p.done && p.of && p.at > 30 ? `${minutes(p.of - p.at)} left` : info.runtime ? `${info.runtime} m` : human(e.size);
    const b = el('button', { type: 'button', class: 'ep', 'data-path': e.path },
      el('span', { class: 'n', text: e.parsed.episode ?? '·' }),
      still,
      el('span', {},
        el('span', { class: 't', text: info.name || e.parsed.episodeTitle || e.name }),
        el('span', { class: 'd', text: [state, info.overview].filter(Boolean).join(' · ') })),
      el('span', { class: 'len', text: left }));
    if (show.next?.path === e.path) b.setAttribute('aria-current', 'true');
    b.onclick = () => play(e, { resume: true });
    return el('li', {}, b);
  };
  const draw = () => {
    $('eps').replaceChildren(...eps.map(row));
    // the list opens on the episode it goes on with
    const cur = $('eps').querySelector('[aria-current="true"]');
    $('eps').scrollTop = cur ? cur.parentElement.offsetTop - $('eps').offsetTop - 8 : 0;
  };
  draw();
  // the season's names and stills, looked up once
  if (!names && tmdb && store.known(show.path)?.tmdb) {
    try {
      names = await lookUpSeason(store, tmdb, show.path, n);
      if (names && show && eps[0]?.show === show.name) draw();
    } catch { /* the filenames will do */ }
  }
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

// The player covers the page: while it is open the page behind is out of
// reach of Tab and of a screen reader, and closing it puts you back on the
// film you opened.
let openedFrom = null;
function showPlayer(path) {
  openedFrom = path;
  $('player').hidden = false;
  syncInert();
}
function hidePlayer() {
  $('player').hidden = true;
  syncInert();
}
function backToFilm() {
  const where = $('ribbon').hidden ? 'main' : '#ribbon';
  const at = openedFrom && [...document.querySelectorAll(`${where} [data-path]`)].find((e) => e.dataset.path === openedFrom);
  (at || ($('ribbon').hidden ? null : $('r-play')))?.focus();
}

async function play(it, { resume }) {
  const v = $('video');
  await stop();
  const p = store.progress[it.path];
  const from = resume && p && !p.done ? p.at : 0;
  showPlayer(it.path);
  $('p-title').textContent = it.label ? `${it.label}${it.parsed.episodeTitle ? ` · ${it.parsed.episodeTitle}` : ''}` : titleOf(it);
  $('p-sub').textContent = from ? `Picking up at ${minutes(from)}` : 'Unlocked on this device';
  $('p-note').textContent = 'Asking the box to play it…';
  $('p-save').hidden = true;
  // on the picture, so Space plays and pauses from the start
  v.tabIndex = 0;
  v.focus();
  playing = { it, offset: from, duration: null, lastSave: Date.now(), stilled: false };
  paintBar();
  // A film is bigger than a tab: the box decrypts it in its own memory for
  // this one viewing and sends a playlist. Safari plays a playlist itself;
  // everywhere else the page loads a player from the box.
  try {
    await attach(it, from);
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
    // the whole file in the tab: its own time is the film's
    playing.offset = 0;
    v.src = URL.createObjectURL(new Blob([plain]));
    if (from) v.addEventListener('loadedmetadata', () => { v.currentTime = from; }, { once: true });
    $('p-note').textContent = '';
    v.play().catch(() => {});
  } catch (e) {
    $('p-note').textContent = e.message;
  }
}

// A session at the box from `from` on, and the player on its playlist
async function attach(it, from) {
  const v = $('video');
  const s = await transcode(lib, it.path, it.sealed, from);
  session = s.url;
  playing.offset = s.from;
  playing.duration = s.duration ?? playing.duration;
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
}

async function closePlayer() {
  await stop();
  hidePlayer();
  // what was watched moves the band on too: the next episode, the time left
  if (opened) await openTitle(opened);
  render();
  backToFilm();
}

// The player's keys, as players have them: Space or K plays and pauses,
// the arrows go back and on ten seconds, F is the whole screen. A button
// with the focus keeps Space and Enter for itself.
function playerKeys(e) {
  if ($('player').hidden || e.ctrlKey || e.metaKey || e.altKey) return;
  const v = $('video');
  const onButton = e.target.closest?.('button, input, select, textarea');
  if ((e.key === ' ' || e.key === 'k') && !onButton) {
    e.preventDefault();
    v.paused ? v.play().catch(() => {}) : v.pause();
  } else if ((e.key === 'ArrowLeft' || e.key === 'ArrowRight') && e.target.id !== 'c-vol') {
    e.preventDefault();
    seekTo(filmAt() + (e.key === 'ArrowLeft' ? -10 : 10));
  } else if (e.key === 'f' && !onButton) {
    e.preventDefault();
    wholeScreen();
  }
}

// ---- the bar
//
// The box packs a film as it plays, so the video element only ever knows
// the part packed so far: its own controls showed a film half a minute
// long, then fourteen. The bar counts in the film's time instead - the
// length the box read from the file, and where this session began - and a
// jump past what is packed asks the box to start again from there.

const clock = (t) => {
  const s = Math.max(0, Math.floor(t || 0));
  const h = Math.floor(s / 3600), m = Math.floor(s / 60) % 60, x = String(s % 60).padStart(2, '0');
  return h ? `${h}:${String(m).padStart(2, '0')}:${x}` : `${m}:${x}`;
};
const filmAt = () => (playing ? playing.offset + ($('video').currentTime || 0) : 0);
function filmLength() {
  const v = $('video');
  return playing?.duration || (Number.isFinite(v.duration) ? (playing?.offset || 0) + v.duration : 0);
}
// How far the box has packed, in the film's time: the end of the playlist
// as it stands, which the video element does not say reliably of one
// still growing. Safari's own player has only the element to ask.
function readyTo() {
  const d = hls?.latestLevelDetails;
  const r = $('video').seekable;
  const end = d ? d.edge : r.length ? r.end(r.length - 1) : 0;
  return (playing?.offset || 0) + (Number.isFinite(end) ? end : 0);
}

let dragging = false;
function paintBar() {
  const v = $('video'), seek = $('c-seek');
  const len = filmLength();
  const at = dragging ? Number(seek.value) : filmAt();
  const pct = (t) => `${len ? Math.min(100, Math.max(0, (t / len) * 100)) : 0}%`;
  seek.max = String(Math.max(1, Math.round(len)));
  if (!dragging) seek.value = String(Math.round(at));
  seek.style.setProperty('--played', pct(at));
  seek.style.setProperty('--from', pct(playing?.offset || 0));
  seek.style.setProperty('--ready', pct(readyTo()));
  seek.setAttribute('aria-valuetext', len ? `${clock(at)} of ${clock(len)}` : clock(at));
  $('c-now').textContent = clock(at);
  $('c-total').textContent = len ? clock(len) : '–:––';
  const b = $('c-play');
  b.dataset.on = v.paused ? 'play' : 'pause';
  b.setAttribute('aria-label', v.paused ? 'Play' : 'Pause');
  const muted = v.muted || v.volume === 0;
  $('c-mute').dataset.on = muted ? 'muted' : 'sound';
  $('c-mute').setAttribute('aria-label', muted ? 'Sound on' : 'Mute');
  $('c-vol').value = String(muted ? 0 : v.volume);
  $('c-vol').style.setProperty('--played', `${muted ? 0 : v.volume * 100}%`);
}

// Where in the film, not in the video: inside what is packed the video
// just goes there; before where this session began, or past what the box
// has packed, the box starts again at that point.
let jumps = 0;
async function seekTo(t) {
  if (!playing) return;
  const v = $('video');
  const len = filmLength();
  t = Math.max(0, len ? Math.min(t, len - 2) : t);
  if (!session || (t >= playing.offset && t <= readyTo())) {
    v.currentTime = Math.max(0, t - playing.offset);
    return;
  }
  const mine = ++jumps;
  const it = playing.it;
  const going = !v.paused;
  $('p-note').textContent = `Asking the box to start at ${clock(t)}…`;
  hls?.destroy();
  hls = null;
  const old = session;
  session = null;
  Promise.resolve(stopTranscode(old)).catch(() => {});
  v.removeAttribute('src');
  v.load();
  // the bar stands at the new place while the box gets there
  playing.offset = t;
  paintBar();
  try {
    await attach(it, t);
    if (mine !== jumps) return; // a later jump took over
    $('p-note').textContent = '';
    if (going) v.play().catch(() => {});
  } catch (e) {
    if (mine === jumps) $('p-note').textContent = `The box could not start there: ${e.message}`;
  }
}

function wholeScreen() {
  document.fullscreenElement ? document.exitFullscreen().catch(() => {}) : $('player').requestFullscreen?.().catch(() => {});
}

// on the whole screen the bars step aside after a moment of playing
let idle = null;
function wake() {
  $('player').classList.remove('idle');
  clearTimeout(idle);
  idle = setTimeout(() => { if (!$('video').paused) $('player').classList.add('idle'); }, 2500);
}

function initBar() {
  const v = $('video'), seek = $('c-seek');
  const toggle = () => (v.paused ? v.play().catch(() => {}) : v.pause());
  $('c-play').onclick = toggle;
  v.addEventListener('click', toggle);
  v.addEventListener('dblclick', wholeScreen);
  $('c-back').onclick = () => seekTo(filmAt() - 10);
  $('c-on').onclick = () => seekTo(filmAt() + 10);
  $('c-full').onclick = wholeScreen;
  // dragging shows the time under the thumb; letting go goes there
  seek.addEventListener('input', () => { dragging = true; paintBar(); });
  seek.addEventListener('change', () => { dragging = false; seekTo(Number(seek.value)); });
  $('c-mute').onclick = () => {
    if (v.muted || v.volume === 0) { v.muted = false; if (v.volume === 0) v.volume = 0.5; } else v.muted = true;
  };
  $('c-vol').addEventListener('input', () => {
    v.volume = Number($('c-vol').value);
    v.muted = v.volume === 0;
  });
  try {
    const kept = Number(localStorage.getItem('dd-volume'));
    if (kept > 0 && kept <= 1) v.volume = kept;
  } catch {}
  v.addEventListener('volumechange', () => {
    try { localStorage.setItem('dd-volume', String(v.volume)); } catch {}
  });
  for (const ev of ['timeupdate', 'progress', 'play', 'pause', 'durationchange', 'volumechange', 'loadedmetadata']) {
    v.addEventListener(ev, paintBar);
  }
  v.addEventListener('pause', wake);
  for (const ev of ['mousemove', 'keydown', 'focusin', 'touchstart']) $('player').addEventListener(ev, wake);
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
  if (opened) {
    const again = [...films, ...shows].find((x) => x.path === opened.path);
    if (again) await openTitle(again);
    else closeTitle();
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
    if (s) openTitle(s);
  };
  $('r-close').onclick = closeTitle;
  $('veil').onclick = closeTitle;
  $('p-close').onclick = closePlayer;
  const v = $('video');
  initBar();
  v.addEventListener('timeupdate', () => { remember(false); stillFrom(v); });
  v.addEventListener('pause', () => remember(true));
  v.addEventListener('ended', () => remember(true));
  addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    if (!$('player').hidden) closePlayer();
    else closeTitle();
  });
  addEventListener('keydown', playerKeys);
  // a tab closed mid-film keeps its place, and the box stops working on it
  addEventListener('pagehide', () => {
    remember(true);
    if (session) stopTranscode(session, true);
  });
  render();
  fillIn();
}

start().catch((e) => { $('msg').hidden = false; $('msg').textContent = String(e.message || e); });
