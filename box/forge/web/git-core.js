// Git's shared pieces: building the page, the forge's API, words and
// numbers, the router. The views are git-*.js; git.js starts them.
//
// Git: our own pages over the forge. Forgejo keeps the repositories, pull
// requests, issues and the CI; these pages read and write them through its
// API, as whoever the gate says is looking (nginx sets the forge's
// reverse-proxy header from the same sign-in as every other service).
// The page is one file; the address says which view to draw.

import { me } from './shell.js';

export const $ = (id) => document.getElementById(id);
// replaceChildren that takes lists and skips what is not there (a bare
// null would be drawn as the word)
export const put = (parent, ...kids) => parent.replaceChildren(...kids.flat(Infinity).filter((k) => k !== null && k !== undefined && k !== false));
export const app = () => $('app');

// ---- building the page

export function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else if (k === 'html') e.innerHTML = v;
    else if (k.startsWith('on')) e[k] = v;
    else if (k === 'style' && typeof v === 'object') Object.assign(e.style, v);
    else e.setAttribute(k, v === true ? '' : v);
  }
  for (const c of kids.flat(Infinity)) if (c !== null && c !== undefined && c !== false) e.append(c);
  return e;
}

const ICONS = {
  repo: '<path d="M4 19V5a2 2 0 0 1 2-2h13v14H6a2 2 0 0 0-2 2Zm0 0a2 2 0 0 0 2 2h13v-4"/>',
  code: '<path d="m8 7-5 5 5 5M16 7l5 5-5 5"/>',
  pr: '<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="19" r="2"/><path d="M6 7v10M18 17V9a3 3 0 0 0-3-3h-4m0 0 2-2m-2 2 2 2"/>',
  merged: '<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="12" r="2"/><path d="M6 7v10M6 7c0 4 5 5 10 5"/>',
  issue: '<circle cx="12" cy="12" r="9"/><circle cx="12" cy="12" r="1.5"/>',
  play: '<circle cx="12" cy="12" r="9"/><path d="m10 8 6 4-6 4Z"/>',
  gear: '<circle cx="12" cy="12" r="3"/><path d="M12 2v3M12 19v3M4.2 4.2l2.1 2.1M17.7 17.7l2.1 2.1M2 12h3M19 12h3M4.2 19.8l2.1-2.1M17.7 6.3l2.1-2.1"/>',
  dir: '<path d="M3 6a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v9a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2Z"/>',
  file: '<path d="M14 3H6a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8Z"/><path d="M14 3v5h5"/>',
  branch: '<circle cx="6" cy="5" r="2"/><circle cx="6" cy="19" r="2"/><circle cx="18" cy="7" r="2"/><path d="M6 7v10M18 9c0 5-12 3-12 8"/>',
  tag: '<path d="M3 12V4h8l10 10-8 8Z"/><circle cx="7.5" cy="8.5" r="1.3"/>',
  commit: '<circle cx="12" cy="12" r="3.5"/><path d="M2 12h6.5M15.5 12H22"/>',
  check: '<path d="m5 12 5 5 9-10"/>',
  x: '<path d="M6 6l12 12M18 6 6 18"/>',
  lock: '<rect x="4" y="11" width="16" height="10" rx="2"/><path d="M8 11V7a4 4 0 0 1 8 0v4"/>',
  copy: '<rect x="9" y="9" width="11" height="11" rx="2"/><path d="M5 15V5a2 2 0 0 1 2-2h8"/>',
  down: '<path d="m6 9 6 6 6-6"/>',
  plus: '<path d="M12 5v14M5 12h14"/>',
  clock: '<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>',
  pulse: '<path d="M3 12h4l3-8 4 16 3-8h4"/>',
  key: '<circle cx="8" cy="15" r="4"/><path d="m11 12 9-9M17 6l3 3"/>',
  bell: '<path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9"/><path d="M10 21a2 2 0 0 0 4 0"/>',
  search: '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>',
  archive: '<rect x="3" y="4" width="18" height="4" rx="1"/><path d="M5 8v12h14V8M10 12h4"/>',
};
export function ic(name, cls = 'i') {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('viewBox', '0 0 24 24');
  s.setAttribute('class', cls);
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = ICONS[name] || '';
  return s;
}

export function avatar(u, cls = 'avatar') {
  return el('img', { class: cls, src: u?.avatar_url || '', alt: '', loading: 'lazy' });
}

// ---- the forge's API

export class ApiError extends Error {
  constructor(status, body) {
    let msg = body;
    try { msg = JSON.parse(body).message || body; } catch { /* not json */ }
    super(msg || `the forge said ${status}`);
    this.status = status;
  }
}

// One call, as the person looking. The answer and, for lists, how many
// there are in all (the forge says so in a header).
export async function api(path, opts = {}) {
  const init = { method: opts.method || 'GET', headers: { accept: 'application/json' }, credentials: 'same-origin' };
  if (opts.body !== undefined) {
    init.headers['content-type'] = 'application/json';
    init.body = JSON.stringify(opts.body);
  }
  const r = await fetch(`/api/v1${path}`, init);
  if (r.status === 401) {
    location.href = '/_dd/login?rd=' + encodeURIComponent(location.pathname + location.search);
    throw new ApiError(401, 'signed out');
  }
  if (!r.ok) throw new ApiError(r.status, await r.text());
  if (r.status === 204 || r.headers.get('content-length') === '0') return null;
  const type = r.headers.get('content-type') || '';
  const data = type.includes('json') ? await r.json() : await r.text();
  if (opts.withTotal) return { data, total: Number(r.headers.get('x-total-count') || 0) };
  return data;
}

// the text of a file, a diff, a log: not json
export async function text(url) {
  const r = await fetch(url, { credentials: 'same-origin' });
  if (!r.ok) throw new ApiError(r.status, await r.text());
  return r.text();
}

export const q = (o) => {
  const p = new URLSearchParams();
  for (const [k, v] of Object.entries(o)) if (v !== undefined && v !== null && v !== '') p.set(k, v);
  const s = p.toString();
  return s ? `?${s}` : '';
};
export const enc = (path) => path.split('/').map(encodeURIComponent).join('/');

// ---- words and numbers

export function ago(when) {
  const t = new Date(when).getTime();
  if (!t || t < 0) return '';
  const s = Math.round((Date.now() - t) / 1000);
  if (s < 45) return 'just now';
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  const d = Math.round(h / 24);
  if (d === 1) return 'yesterday';
  if (d < 30) return `${d} days ago`;
  return new Date(t).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: d > 300 ? 'numeric' : undefined });
}
export const when = (w) => el('time', { datetime: w, title: new Date(w).toLocaleString(), text: ago(w) });
export const plural = (n, one, many = `${one}s`) => `${n.toLocaleString()} ${n === 1 ? one : many}`;
export function bytes(n) {
  if (n < 1024) return `${n} B`;
  const u = ['KB', 'MB', 'GB'];
  let i = -1;
  do { n /= 1024; i++; } while (n >= 1024 && i < u.length - 1);
  return `${n.toFixed(n < 10 ? 1 : 0)} ${u[i]}`;
}
export function duration(ms) {
  const s = Math.round(ms / 1000);
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m} m ${s % 60} s`;
  return `${Math.floor(m / 60)} h ${m % 60} m`;
}
export const short = (sha) => (sha || '').slice(0, 7);
export const firstLine = (msg) => (msg || '').split('\n')[0];

export function toast(msg) {
  const t = el('div', { class: 'toast', role: 'status', text: msg });
  document.body.append(t);
  setTimeout(() => t.remove(), 2600);
}
export async function copy(textToCopy, btn) {
  try {
    await navigator.clipboard.writeText(textToCopy);
    toast('Copied');
  } catch {
    btn?.closest('.clone')?.querySelector('input')?.select();
  }
}

// ---- markdown, cleaned of anything that could run

let marked = null;
export async function markdown(src, base) {
  marked ??= (await import('/_dd/web/marked.js')).marked;
  const html = marked.parse(src || '', { gfm: true, breaks: false });
  const div = el('div', { class: 'md' });
  div.innerHTML = window.DOMPurify ? window.DOMPurify.sanitize(html) : '';
  // relative links and pictures inside a readme point into the repository
  if (base) {
    for (const a of div.querySelectorAll('a[href]')) {
      const h = a.getAttribute('href');
      if (!/^([a-z]+:|#|\/)/i.test(h)) a.setAttribute('href', `${base.tree}/${h}`);
    }
    for (const img of div.querySelectorAll('img[src]')) {
      const s = img.getAttribute('src');
      if (!/^([a-z]+:|\/)/i.test(s)) img.setAttribute('src', `${base.raw}/${s}`);
    }
  }
  return div;
}

// ---- the person looking

export let who = null;
export async function whoami() {
  if (who) return who;
  const [m, u] = await Promise.all([me(), api('/user').catch(() => null)]);
  who = { name: m.user, forge: u, member: m.role === 'member' };
  return who;
}

// ---- small shared pieces

export function statusDot(state) {
  // cancelled is red: a run here is cancelled when one of its jobs failed
  const s = { success: 'ok', failure: 'bad', error: 'bad', cancelled: 'bad', pending: 'busy', running: 'busy', waiting: 'busy', warning: 'busy', blocked: 'busy', skipped: '' }[state] ?? '';
  if (s === 'busy' && (state === 'running' || state === 'pending')) return el('span', { class: 'spin', title: state });
  return el('span', { class: `dot ${s}`, title: state || 'no checks' });
}

export function pop(button, build) {
  const wrap = el('span', { class: 'pop' });
  const menu = el('div', { class: 'menu', role: 'menu' });
  menu.hidden = true;
  button.setAttribute('aria-haspopup', 'true');
  button.onclick = async (e) => {
    e.stopPropagation();
    if (!menu.hidden) { menu.hidden = true; return; }
    menu.replaceChildren(...[await build(() => { menu.hidden = true; })].flat());
    menu.hidden = false;
    menu.querySelector('input')?.focus();
  };
  document.addEventListener('click', (e) => { if (!wrap.contains(e.target)) menu.hidden = true; });
  wrap.append(button, menu);
  return wrap;
}

export function pager(page, total, per, go) {
  const last = Math.max(1, Math.ceil(total / per));
  if (last <= 1) return null;
  return el('div', { class: 'pager' },
    el('button', { class: 'btn plain', disabled: page <= 1, onclick: () => go(page - 1) }, 'Newer'),
    el('span', { class: 'small muted', style: 'align-self:center', text: `${page} of ${last}` }),
    el('button', { class: 'btn plain', disabled: page >= last, onclick: () => go(page + 1) }, 'Older'));
}

export function fail(e) {
  const status = e?.status;
  const msg = status === 404 ? 'Nothing here, or nothing you may see.' : status === 403 ? 'You may not see this.' : e?.message || String(e);
  return el('div', { class: 'box empty' }, el('p', { class: 'err', text: msg }), el('p', {}, el('a', { href: '/', 'data-nav': '', text: 'Your repositories' })));
}

export function setTitle(...parts) {
  document.title = [...parts.filter(Boolean), 'Git'].join(' · ');
}

// ---- where we are

const routes = [];
export function route(pattern, view) { routes.push([pattern, view]); }

// Addresses the forge itself answers: clones, raw files, archives, logs.
// A link to one of these is a real page load, not a view.
const FORGE = /^\/(api|assets|avatars|repo-avatars|attachments|user|login|-)\/|\/(raw|archive|media|releases\/download)\/|\/logs$|\.git(\/|$)/;

export function go(href, replace = false) {
  const u = new URL(href, location.href);
  if (u.origin !== location.origin || u.pathname.startsWith('/_dd/') || FORGE.test(u.pathname)) {
    location.href = u.href;
    return;
  }
  history[replace ? 'replaceState' : 'pushState']({}, '', u.pathname + u.search + u.hash);
  render();
}

let drawing = 0;
export async function render() {
  const mine = ++drawing;
  const path = decodeURIComponent(location.pathname).replace(/\/+$/, '') || '/';
  const params = new URLSearchParams(location.search);
  for (const [pattern, view] of routes) {
    const m = path.match(pattern);
    if (!m) continue;
    const main = app();
    try {
      await view({ m, params, path, current: () => mine === drawing });
    } catch (e) {
      if (mine !== drawing) return;
      console.error(e);
      main.replaceChildren(fail(e));
    }
    if (location.hash) document.getElementById(location.hash.slice(1))?.scrollIntoView();
    return;
  }
  $('rhead').replaceChildren();
  app().replaceChildren(fail({ status: 404 }));
}

// links inside the page move between views without a reload
document.addEventListener('click', (e) => {
  if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
  const a = e.target.closest('a[href]');
  if (!a || a.target === '_blank' || a.hasAttribute('download')) return;
  const u = new URL(a.href, location.href);
  if (u.origin !== location.origin || u.pathname.startsWith('/_dd/') || FORGE.test(u.pathname)) return;
  if (u.pathname === location.pathname && u.search === location.search && u.hash) return;
  e.preventDefault();
  go(u.href);
  window.scrollTo(0, 0);
});
addEventListener('popstate', render);

$('git-search')?.addEventListener('submit', (e) => {
  e.preventDefault();
  const v = new FormData(e.target).get('q');
  if (v) go(`/search${q({ q: v })}`);
});

// the bell: unread notifications, looked at now and every two minutes
async function bell() {
  try {
    const { new: n } = await api('/notifications/new');
    $('git-bell').hidden = false;
    $('git-bell-n').hidden = !n;
    $('git-bell-n').textContent = n > 99 ? '99+' : String(n);
  } catch { /* the bell waits */ }
}

export { bell };
