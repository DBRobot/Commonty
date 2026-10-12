// Ask Commonty: the assistant in the lower right corner of every signed-in
// page. It knows the site map (/_dd/sitemap.json) and the docs, and nothing
// on the page: it never reads the page or anything of the member's. What
// it can do is say something, as plain words, and take someone to a place
// on the map - by its id, which our code looks up, so nothing the model
// writes is ever an address, a link or markup. Questions go to the model
// Chat uses, through the same door and limits as Chat. It can also point
// at one of our own controls (guide-tour.js), show someone around a page,
// say how the services are and what is new, and help write up a bug for
// the member to post themselves; its face is guide-face.js.

import { el } from './kit.js';
import { dress, undress, faceFor, moments } from './guide-face.js';
import { find, point, tour, stepsHere, inTour } from './guide-tour.js';

const KEYS = {
  off: 'dd-guide-off',
  auto: 'dd-guide-autogo',
  tucked: 'dd-guide-tucked',
  peeked: 'dd-guide-peeked',
  convo: 'dd-guide-convo',
  open: 'dd-guide-open',
  retry: 'dd-guide-retry',
  model: 'dd-guide-model',
  key: 'dd-guide-key',
  toured: 'dd-guide-toured',
  release: 'dd-guide-release',
  news: 'dd-guide-news',
  point: 'dd-guide-point',
  bug: 'dd-guide-bug',
};
// a peek an hour after it is tucked away, then two hours apart at least,
// twice a day at most
const HOUR = 3600e3;
const DAY = 24 * HOUR;
const APART = 2 * HOUR;
const A_DAY = 2;
// what one answer may say, and point at
const SAY = 600;
const GO = 3;
// what a drafted issue may be
const TITLE = 120;
const BODY = 2000;

const local = {
  get: (k) => { try { return localStorage.getItem(k); } catch { return null; } },
  set: (k, v) => { try { localStorage.setItem(k, v); } catch { /* refused: this page only */ } },
  del: (k) => { try { localStorage.removeItem(k); } catch { /* nothing kept */ } },
};
const tab = {
  get: (k) => { try { return sessionStorage.getItem(k); } catch { return null; } },
  set: (k, v) => { try { sessionStorage.setItem(k, v); } catch { /* refused: this page only */ } },
  del: (k) => { try { sessionStorage.removeItem(k); } catch { /* nothing kept */ } },
};

/// whether this device shows the assistant (Settings, This device)
export const shown = () => local.get(KEYS.off) !== '1';

let started = null;
let ui = null;

// The shortcut that opens it from anywhere: Ctrl+K, or ⌘K on a Mac, unless
// this device set its own (Settings, This device)
const mac = /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
export const DEFAULT_KEY = mac ? 'Meta+K' : 'Ctrl+K';
export const shortcut = () => local.get(KEYS.key) || DEFAULT_KEY;
export const ownKey = () => !!local.get(KEYS.key);
export function setShortcut(combo) {
  if (combo && combo !== DEFAULT_KEY) local.set(KEYS.key, combo);
  else local.del(KEYS.key);
}
/// a key press as a combination, like "Ctrl+Shift+G"; null for a modifier alone
export function comboOf(e) {
  if (['Control', 'Shift', 'Alt', 'Meta', 'AltGraph', 'CapsLock', 'Dead', 'Unidentified'].includes(e.key)) return null;
  let k = e.key.length === 1 ? e.key.toUpperCase() : e.key;
  // the key, not what Option or Shift made of it
  if (/^Key[A-Z]$/.test(e.code)) k = e.code.slice(3);
  else if (/^Digit\d$/.test(e.code)) k = e.code.slice(5);
  if (k === ' ') k = 'Space';
  return [e.ctrlKey && 'Ctrl', e.altKey && 'Alt', e.shiftKey && 'Shift', e.metaKey && 'Meta', k].filter(Boolean).join('+');
}
/// a combination as a person reads it
export const keyName = (c) => c.split('+').map((p) => ({ Meta: mac ? 'Cmd' : 'Meta', Alt: mac ? 'Option' : 'Alt' })[p] || p).join('+');
const ariaKey = (c) => c.replace('Ctrl', 'Control');

/// turned on or off in Settings: gone from every page on this device
export function setShown(on) {
  local.set(KEYS.off, on ? '0' : '1');
  if (!on) {
    ui?.remove();
    ui = null;
  } else if (started && !ui) {
    ui = build(started);
  }
}

/// shell.js starts it once it knows who is looking: a member or the demo.
/// `here` turns a map address into where this page may go (shell.js).
export function start(opts) {
  if (started) return;
  started = opts;
  if (shown()) ui = build(opts);
  addEventListener('storage', (e) => {
    if (e.key === KEYS.off) setShown(e.newValue !== '1');
  });
}

// ---- the map

let map = null;
let byId = {};
// the places and, under each, the controls it lists: "settings-devices.add-device"
let all = [];
let mapAsked = null;
const mapKey = 'kit:/_dd/sitemap.json';

function useMap(m) {
  if (!m || !Array.isArray(m.places)) return;
  map = m;
  const places = m.places.filter((p) => typeof p.id === 'string' && typeof p.label === 'string');
  const targets = places.flatMap((p) => (Array.isArray(p.targets) ? p.targets : [])
    .filter((t) => typeof t?.name === 'string' && /^[a-z0-9-]{1,40}$/.test(t.name) && typeof t.label === 'string')
    .map((t) => ({ id: `${p.id}.${t.name}`, label: t.label, parent: p.id, kind: p.kind, url: p.url, words: t.words || '', target: t.name, where: typeof t.where === 'string' ? t.where : null })));
  all = [...places, ...targets];
  byId = Object.fromEntries(all.map((p) => [p.id, p]));
}

// what this browser kept of the map draws at once; the box's is fetched
function loadMap() {
  if (!map) {
    try { useMap(JSON.parse(localStorage.getItem(mapKey) || 'null')); } catch { /* none kept */ }
  }
  mapAsked ??= fetch('/_dd/sitemap.json')
    .then((r) => (r.ok ? r.json() : null))
    .then((m) => {
      if (!m) return;
      useMap(m);
      local.set(mapKey, JSON.stringify(m));
    })
    .catch(() => {});
  return mapAsked;
}

const pathOf = (p) => {
  const seen = new Set();
  const parts = [];
  for (let n = p; n && !seen.has(n.id); n = byId[n.parent]) {
    seen.add(n.id);
    parts.unshift(n.label);
  }
  return parts.join(' › ');
};

/// A heading's anchor, the same here and on the docs page (shell.js slug)
const slug = (text) => text.toLowerCase().replace(/[^\p{L}\p{N}\s-]/gu, '').trim().replace(/\s+/g, '-');

// ---- matching in the page: the words against each place's name and words

const STOP = new Set(['the', 'to', 'me', 'take', 'go', 'how', 'do', 'where', 'is', 'my', 'page', 'and', 'of', 'in', 'on', 'for', 'can', 'a', 'an', 'i', 'it', 'what', 'show', 'open', 'find', 'get', 'with', 'there', 'want', 'button']);
const words = (s) => s.toLowerCase().replace(/[^\p{L}\p{N} ]/gu, ' ').split(/\s+/).filter((w) => w.length > 1 && !STOP.has(w));

function match(q, kinds) {
  const ws = words(q);
  if (!ws.length || !map) return [];
  return all
    .filter((n) => !kinds || kinds.includes(n.kind))
    .map((n) => {
      const label = n.label.toLowerCase();
      const hay = `${label} ${(n.group || '').toLowerCase()} ${n.words || ''} ${(n.subs || []).join(' ').toLowerCase()}`;
      let s = 0;
      for (const w of ws) {
        if (label === w || label.startsWith(w)) s += 3;
        else if (hay.includes(w)) s += 1.4;
        else if (w.length > 3 && hay.includes(w.slice(0, -1))) s += 0.8;
      }
      // a place before a control of it, when the words fit both alike
      return { n, s: (s / ws.length) * (n.target ? 0.95 : 1) };
    })
    .filter((x) => x.s >= 0.9)
    .sort((a, b) => b.s - a.s)
    .slice(0, GO)
    .map((x) => x.n);
}

// what is asked for that is not a place: a tour, a bug, the services, the news
const TOUR = /\b(show me around|give me a tour|a tour|tour of|walk me through|show me how this page works)\b/i;
const BUG = /\b(report (a |an )?(bug|problem|issue)|found (a |an )?(bug|problem)|bug report|(something|it|this|that)(['’]?s| is) (broken|wrong|not working))\b/i;
const NEWS = /\b(what['’]?s new|what is new|what changed|what has changed|release notes?|changelog|new features?|latest release)\b/i;
const STATUS = (q) => !/\b(back|set|sign|pick|look|make|show|turn|speed|start|log)\s+up\b/i.test(q)
  && (/\b(is|are)\b.{0,40}\b(down|up|working|online|offline)\s*\??\s*$/i.test(q) || /\b(outage|status|what['’]?s down|anything down|something down)\b/i.test(q));
const STOP_BUG = /^(cancel|stop|never ?mind|forget it|no)\b/i;

const isQuestion = (q) => /\?\s*$|^(how|what|why|who|when|which|can|could|does|do|is|are|will|should)\b/i.test(q.trim());

// ---- the docs the model is given: the few sections whose words match best

function sections(q) {
  const ws = words(q);
  if (!ws.length || !map) return [];
  const out = [];
  for (const n of map.places) {
    if (n.kind !== 'docs' || !n.doc) continue;
    const title = n.label.toLowerCase();
    for (const h of [null, ...(n.subs || [])]) {
      const hay = `${title} ${(h || '').toLowerCase()} ${h ? '' : n.words || ''}`;
      let s = 0;
      for (const w of ws) if (hay.includes(w) || (w.length > 3 && hay.includes(w.slice(0, -1)))) s += h?.toLowerCase().includes(w) ? 2 : 1;
      if (s) out.push({ n, h, s });
    }
  }
  return out.sort((a, b) => b.s - a.s).slice(0, 3);
}

// a section of a docs page's markdown, from its heading to the next of its rank
function cut(md, heading) {
  const lines = md.split('\n');
  let from = 0;
  if (heading) {
    const at = lines.findIndex((l) => /^##\s/.test(l) && l.replace(/^##\s+/, '').replace(/[`*_]/g, '').trim() === heading);
    if (at >= 0) from = at;
  }
  let to = lines.length;
  for (let i = from + 1; i < lines.length; i++) if (/^##?\s/.test(lines[i])) { to = i; break; }
  return lines.slice(from, to).join('\n').slice(0, 2500);
}

async function docsText(found) {
  return Promise.all(found.map(async ({ n, h }) => {
    const r = await fetch(`/_dd/docs/raw/${n.doc.split('/').map(encodeURIComponent).join('/')}`).catch(() => null);
    const md = r?.ok ? await r.text() : '';
    return `[${n.id}] ${pathOf(n)}${h ? ` › ${h}` : ''}\n${cut(md, h)}`;
  }));
}

// ---- the model: Chat's, through Chat's own door

class Unreachable extends Error {}
class SignIn extends Error {}

function endpoint(opts) {
  // a browser the app let in: the app sends it on with this device's token
  if (opts.servedByApp) return '/v1/chat/completions';
  // the app's own window has no way to it
  if (opts.inApp || typeof map?.model !== 'string') return null;
  try {
    const u = new URL(map.model);
    if (u.protocol !== 'https:') return null;
    return u.origin === location.origin ? u.pathname : u.href;
  } catch {
    return null;
  }
}

async function modelId(url, init) {
  const kept = tab.get(KEYS.model);
  if (kept) return kept;
  const r = await fetch(url.replace(/\/chat\/completions$/, '/models'), init);
  if (r.type === 'opaqueredirect' || r.status === 401) throw new SignIn();
  if (!r.ok) throw new Unreachable(String(r.status));
  const id = (await r.json()).data?.[0]?.id;
  if (typeof id !== 'string') throw new Unreachable('no model');
  tab.set(KEYS.model, id);
  return id;
}

const SYSTEM = `You are "Ask Commonty", the guide to the Commonty website. You can do two things only: answer questions about Commonty from the docs sections given to you, and point the member to places on the site from the list of places given to you.
Reply with one JSON object and nothing else: {"say": "...", "go": ["place id", ...], "from": "docs place id"}
- "say": plain words, at most three short sentences, in the language the question is written in (the docs are in English). No markdown, no links, no addresses, no code.
- "go": up to three ids, copied exactly from the list of places, of the places that answer the question. Empty if none fits.
- "from": the id in square brackets of the docs section you answered from, or "" if you did not use one.
If the docs given do not answer the question, say you could not find it in the docs. Never make up an id, a place or a fact.`;

const ISSUE = `You help a member of the Commonty website write up a bug for its public issue tracker. Reply with one JSON object and nothing else: {"title": "...", "body": "..."}
- "title": one plain line of at most 80 characters saying what goes wrong.
- "body": plain text with two short parts, "What happened:" and "What I expected:", from the member's own words. No markdown, no links, no code.
Write in the language the member wrote in. Leave out anything private: names, email addresses, passwords, file names, addresses. Never add facts the member did not give.`;

async function askModel(opts, q, found) {
  const places = all.map((p) => `${p.id} = ${pathOf(p)}`).join('\n');
  const docs = (await docsText(found)).join('\n\n');
  return complete(opts, SYSTEM, `Places:\n${places}\n\nDocs sections:\n${docs || '(none matched)'}\n\nQuestion: ${q}`, 400);
}

async function complete(opts, system, user, max) {
  const url = endpoint(opts);
  if (!url) throw new Unreachable('none');
  const init = { credentials: 'include', redirect: 'manual' };
  const model = await modelId(url, init);
  const r = await fetch(url, {
    ...init,
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      model,
      stream: false,
      temperature: 0.2,
      max_tokens: max,
      response_format: { type: 'json_object' },
      chat_template_kwargs: { enable_thinking: false },
      messages: [
        { role: 'system', content: system },
        { role: 'user', content: user },
      ],
    }),
  });
  if (r.type === 'opaqueredirect' || r.status === 401) throw new SignIn();
  if (r.status === 403 && opts.demo) throw new Unreachable('The demo gets ten questions an hour, and this hour’s are used. Try again later.');
  if (r.status === 429) throw new Unreachable('Too many questions at once. Try again in a moment.');
  if (!r.ok) throw new Unreachable(String(r.status));
  const j = await r.json();
  return j.choices?.[0]?.message?.content;
}

/// What the model said, held to what it may say: words as words, places
/// only from the map, the docs it cites only a docs page on the map.
/// Anything else is dropped, and an answer with nothing left is null.
export function checked(content, places) {
  if (typeof content !== 'string') return null;
  let v;
  try {
    const at = content.indexOf('{');
    v = JSON.parse(content.slice(at, content.lastIndexOf('}') + 1));
  } catch {
    return null;
  }
  if (!v || typeof v !== 'object') return null;
  const say = typeof v.say === 'string' ? v.say.trim().slice(0, SAY) : '';
  const go = [...new Set(Array.isArray(v.go) ? v.go.filter((id) => typeof id === 'string' && Object.hasOwn(places, id)) : [])].slice(0, GO);
  const from = typeof v.from === 'string' && Object.hasOwn(places, v.from) && places[v.from].kind === 'docs' ? v.from : null;
  if (!say && !go.length) return null;
  return { say, go, from };
}

// words only: no control characters but the line break, at most n of them
function plain(s, n) {
  if (typeof s !== 'string') return '';
  return [...s].filter((c) => c === '\n' || (c >= ' ' && c !== '\x7f')).join('').trim().slice(0, n);
}

/// A drafted issue, held to what it may be: a title and a body, both plain
/// text and capped; null for anything else
export function checkedIssue(content) {
  if (typeof content !== 'string') return null;
  let v;
  try {
    v = JSON.parse(content.slice(content.indexOf('{'), content.lastIndexOf('}') + 1));
  } catch {
    return null;
  }
  if (!v || typeof v !== 'object') return null;
  const title = plain(v.title, TITLE).replace(/\s+/g, ' ');
  const body = plain(v.body, BODY);
  return title && body ? { title, body } : null;
}

/// Where the issue opens, written up: the repository the map names, on
/// https, with the words as its query, never part of the address
export function issueUrl(report, issue) {
  try {
    const u = new URL(report);
    if (u.protocol !== 'https:' || !/\/issues\/new$/.test(u.pathname)) return null;
    u.search = '';
    u.hash = '';
    u.searchParams.set('title', issue.title);
    u.searchParams.set('body', issue.body);
    return u.href;
  } catch {
    return null;
  }
}

// ---- how the services are, and what is new: the box's own lists, as words

const when = (secs) => (typeof secs === 'number' && secs > 0
  ? new Date(secs * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' })
  : null);

async function fetchJson(path) {
  const r = await fetch(path, { credentials: 'same-origin' }).catch(() => null);
  if (!r) return { error: 'down' };
  if (r.status === 404) return { error: 'missing' };
  if (!r.ok) return { error: 'down' };
  try { return { json: await r.json() }; } catch { return { error: 'down' }; }
}

/// What /_dd/status.json says of the services asked about, or of all of them
export async function statusSay(q) {
  const { json, error } = await fetchJson('/_dd/status.json');
  if (error === 'missing') return 'I can’t tell yet: this Commonty doesn’t report how its services are doing.';
  if (error) return 'I couldn’t check just now. Try again in a moment.';
  const list = (Array.isArray(json?.services) ? json.services : [])
    .filter((s) => typeof s?.label === 'string' && typeof s.up === 'boolean')
    .map((s) => ({ id: typeof s.id === 'string' ? s.id.toLowerCase() : '', label: plain(s.label, 60), up: s.up, since: when(s.since) }));
  if (!list.length) return 'I can’t tell yet: no services are reported.';
  const ws = words(q);
  const named = list.filter((s) => ws.some((w) => s.id === w || s.label.toLowerCase().split(/\s+/).some((l) => l.startsWith(w))));
  const line = (s) => `${s.label} is ${s.up ? 'up' : 'down'}${s.since ? ` since ${s.since}` : ''}.`;
  if (named.length) return named.map(line).join('\n');
  const down = list.filter((s) => !s.up);
  return down.length ? down.map(line).join('\n') : `Everything is up: ${list.map((s) => s.label).join(', ')}.`;
}

/// The newest releases from /_dd/releases.json, as plain lines
export function releasesSay(json, few = 3) {
  const rel = (Array.isArray(json?.releases) ? json.releases : []).filter((r) => Number.isInteger(r?.n)).slice(0, few);
  if (!rel.length) return null;
  return rel.map((r) => {
    const head = `Release ${r.n}${when(r.at) ? `, ${when(r.at)}` : ''}${typeof r.title === 'string' && r.title.trim() ? `: ${plain(r.title, 120)}` : ''}`;
    const notes = (Array.isArray(r.notes) ? r.notes : []).map((n) => plain(n, 200)).filter(Boolean).slice(0, 6);
    return [head, ...notes.map((n) => `• ${n}`)].join('\n');
  }).join('\n\n');
}

// ---- the corner

// drawn from parts, never from markup
const NS = 'http://www.w3.org/2000/svg';
function svg(box, cls, ...paths) {
  const s = document.createElementNS(NS, 'svg');
  s.setAttribute('viewBox', box);
  s.setAttribute('aria-hidden', 'true');
  if (cls) s.setAttribute('class', cls);
  for (const attrs of paths) {
    const p = document.createElementNS(NS, 'path');
    for (const [k, v] of Object.entries(attrs)) p.setAttribute(k, v);
    s.append(p);
  }
  return s;
}
const line = { fill: 'none', stroke: 'currentColor', 'stroke-width': '2', 'stroke-linecap': 'round', 'stroke-linejoin': 'round' };
const chatMark = () => svg('0 0 24 24', 'icon',
  { d: 'M4 5h16v11H9l-5 4z', fill: 'none', stroke: 'currentColor', 'stroke-width': '1.8', 'stroke-linejoin': 'round' },
  { d: 'M8.5 10.5h7M12 8v5', class: 'plus', 'stroke-width': '1.8', 'stroke-linecap': 'round' });
const arrow = (d, box) => svg(box, null, { d, ...line });
const dot = (kind) => el('i', { class: `dot ${kind === 'docs' ? 'dot-docs' : kind === 'app' ? 'dot-app' : ''}` });

function build(opts) {
  const body = document.body;
  const eyes = el('span', { class: 'eyes', 'aria-hidden': 'true' }, el('span', { class: 'eye' }, el('i'), el('b')), el('span', { class: 'eye' }, el('i'), el('b')));
  const btn = el('button', { type: 'button', class: 'dd-guide-btn', 'aria-label': 'Open the assistant', 'aria-expanded': 'false', 'aria-controls': 'dd-guide', 'data-guide': 'assistant', 'aria-keyshortcuts': ariaKey(shortcut()) },
    chatMark(), eyes, el('i', { class: 'o', 'aria-hidden': 'true' }), el('i', { class: 'new', 'aria-hidden': 'true' }));
  const tuck = el('button', { type: 'button', class: 'dd-tuck', 'aria-label': 'Tuck the assistant away' }, arrow('m3 5 5 5-5 5', '0 0 12 20'));
  const log = el('div', { class: 'log', role: 'log', 'aria-live': 'polite', tabindex: '0', 'aria-label': 'Conversation' });
  const input = el('input', { type: 'text', placeholder: 'Ask, or say where to go', autocomplete: 'off', 'aria-label': 'Ask Commonty', maxlength: '300' });
  const auto = el('input', { type: 'checkbox', id: 'dd-guide-auto' });
  auto.checked = local.get(KEYS.auto) === '1';
  auto.addEventListener('change', () => local.set(KEYS.auto, auto.checked ? '1' : '0'));
  const hide = el('button', { type: 'button', 'aria-label': 'Hide the assistant' }, arrow('m5 8 5 5 5-5', '0 0 20 20'));
  const form = el('form', {}, input, el('button', { type: 'submit', text: 'Send' }));
  const panel = el('section', { class: 'dd-guide', id: 'dd-guide', 'aria-labelledby': 'dd-guide-h' },
    el('header', {}, el('h2', { id: 'dd-guide-h', text: 'Ask Commonty' }), hide),
    log, form,
    el('label', { class: 'fine', for: 'dd-guide-auto' }, el('span', { class: 'sw' }, auto, el('i')), 'Go there straight away'));
  panel.hidden = true;
  body.append(btn, tuck, panel);

  // Now and then, in its corner, it looks up and follows the pointer: only
  // while the member is there, and never over anything open or moving
  const face = moments(btn, () => !reduced() && seen() && !body.classList.contains('dd-tucked')
    && !body.classList.contains('dd-peek') && panel.hidden && !inTour() && !document.querySelector('.dd-ring'));
  btn.addEventListener('click', () => face.end(), { capture: true });

  // the conversation: this tab's, never a box's
  let convo = [];
  try { convo = JSON.parse(tab.get(KEYS.convo) || '[]'); } catch { convo = []; }
  if (!Array.isArray(convo)) convo = [];
  const keep = () => tab.set(KEYS.convo, JSON.stringify(convo.slice(-40)));

  function go(id, section) {
    const n = byId[id];
    if (!n) return;
    let url = n.url;
    if (n.target) {
      // here already, and showing: point at it where it is
      if (find(n.target)) {
        point(n.target, { clear });
        return;
      }
      tab.set(KEYS.point, JSON.stringify({ name: n.target, url: n.url, at: Date.now() }));
      try {
        // to another of the site's names the note goes along in the address,
        // which the page there takes out again
        const u = new URL(n.url);
        if (u.origin !== location.origin && n.kind === 'page' && !opts.inApp && !opts.servedByApp) {
          u.searchParams.set('dd-point', n.target);
          url = u.href;
        }
      } catch { /* not an address: here() says no */ }
    }
    const at = opts.here(url + (section && (n.subs || []).includes(section) ? `#${slug(section)}` : ''));
    if (!at || at === '#') return;
    // a page the app opens in the device's browser: asked for, not followed
    if (at.startsWith('/_dd/app/open')) {
      fetch(at).catch(() => {});
      return;
    }
    location.assign(at);
    // the same page at another tab (Settings): nothing loads, so point here
    if (n.target) pointSoon();
  }

  // a control asked for on the way here: pointed at once it shows
  function pointSoon() {
    let p = null;
    try { p = JSON.parse(tab.get(KEYS.point) || 'null'); } catch { /* none */ }
    if (!p || typeof p.name !== 'string' || !(Date.now() - p.at < 30e3)) return;
    try {
      const u = new URL(p.url);
      if (u.origin !== location.origin || u.pathname !== location.pathname) return;
    } catch {
      return;
    }
    point(p.name, { clear }).then((yes) => { if (yes) tab.del(KEYS.point); });
  }

  // the conversation steps aside from a control it is pointing at
  function clear(t) {
    if (panel.hidden) return;
    const a = t.getBoundingClientRect();
    const b = panel.getBoundingClientRect();
    if (a.right < b.left || a.left > b.right || a.bottom < b.top || a.top > b.bottom) return;
    open(false);
    btn.focus();
  }

  function card(id, section) {
    const n = byId[id];
    if (!n) return null;
    const b = el('button', { type: 'button' }, dot(n.kind),
      el('span', { class: 'path' }, el('b', { text: pathOf(n) + (section ? ` › ${section}` : '') }),
        el('span', { text: n.kind === 'app' ? 'opens through the Commonty app' : n.url.replace(/^https:\/\//, '') })));
    b.addEventListener('click', () => go(id, section));
    return b;
  }

  function draw(m) {
    if (m.who === 'me') {
      log.append(el('div', { class: 'msg me', text: m.text }));
      return;
    }
    const box = el('div', { class: 'msg it', text: m.text });
    const cards = (m.ids || []).map((id) => card(id, id === m.src ? m.section : null)).filter(Boolean);
    if (cards.length) box.append(el('div', { class: 'dest' }, cards));
    if (m.signIn) {
      const b = el('button', { type: 'button' }, dot('page'), el('span', { class: 'path' }, el('b', { text: 'Sign in to Chat' }), el('span', { text: 'and come back here' })));
      b.addEventListener('click', signIn);
      box.append(el('div', { class: 'dest' }, b));
    }
    if (m.issue) {
      box.append(el('div', { class: 'issue' },
        el('b', { text: m.issue.title }),
        el('span', { text: m.issue.body })));
      const b = el('button', { type: 'button', class: 'out', text: 'Open the issue in Git' });
      b.addEventListener('click', () => openIssue(m.issue));
      box.append(b);
    }
    if (m.src && byId[m.src]) box.append(el('span', { class: 'how', text: `From the docs: ${pathOf(byId[m.src])}${m.section ? ` › ${m.section}` : ''}` }));
    else if (m.how) box.append(el('span', { class: 'how', text: m.how }));
    log.append(box);
  }

  function chips() {
    if (convo.some((m) => m.who === 'me')) return;
    const asks = opts.demo
      ? ['Movies', 'who can see my files?', 'how do I join?', 'show me around']
      : ['wifi', 'who can see my files?', 'how do I get my stuff out', 'show me around'];
    if (btn.classList.contains('has-new')) asks.unshift('what’s new?');
    log.append(el('div', { class: 'chips' }, asks.map((t) => {
      const b = el('button', { type: 'button', text: t });
      b.addEventListener('click', () => ask(t));
      return b;
    })));
  }

  function redraw() {
    log.replaceChildren();
    if (!convo.length) convo = [{ who: 'it', text: 'Ask about Commonty, or tell me where you want to go.' }];
    convo.forEach(draw);
    chips();
    log.scrollTop = log.scrollHeight;
  }

  function say(m) {
    convo.push(m);
    keep();
    log.querySelector('.chips')?.remove();
    draw(m);
    log.scrollTop = log.scrollHeight;
  }

  // Chat's name signs this browser in on its own, once, and comes back
  // here; the question is asked again then
  function signIn() {
    const q = [...convo].reverse().find((m) => m.who === 'me')?.text;
    if (q) tab.set(KEYS.retry, q);
    try {
      const o = new URL(map.model).origin;
      const back = `/_dd/back?to=${encodeURIComponent(location.href)}`;
      location.assign(`${o}/_dd/login?rd=${encodeURIComponent(back)}`);
    } catch { /* no model on the map */ }
  }

  const places = (hits, how) => {
    const t = hits[0];
    // where a control is: there, and pointed at
    if (t?.target && (hits.length === 1 || !hits[1].target || find(t.target))) {
      say({ who: 'it', text: `${pathOf(t)}${t.where ? `, ${t.where}` : ''}.`, ids: [t.id], how });
      if (!t.where || find(t.target)) go(t.id);
      return;
    }
    if (hits.length === 1 && auto.checked) {
      say({ who: 'it', text: `Took you to ${pathOf(hits[0])}.`, how });
      go(hits[0].id);
      return;
    }
    say({ who: 'it', text: hits.length > 1 ? 'A few places:' : 'Here:', ids: hits.map((n) => n.id), how });
  };

  // A bug, written up with the member: what happened, what they expected,
  // then a draft they post themselves in Git. Nothing is posted from here.
  let bug = null;
  try { bug = JSON.parse(tab.get(KEYS.bug) || 'null'); } catch { bug = null; }
  const keepBug = (b) => { bug = b; if (b) tab.set(KEYS.bug, JSON.stringify(b)); else tab.del(KEYS.bug); };
  const PUBLIC = 'Issues are public - don’t include anything private.';

  async function bugStep(q) {
    if (STOP_BUG.test(q.trim())) {
      keepBug(null);
      say({ who: 'it', text: 'All right, nothing written up.' });
      return;
    }
    if (bug.step === 'what') {
      keepBug({ step: 'expected', what: q });
      say({ who: 'it', text: 'And what did you expect to happen?' });
      return;
    }
    const what = bug.what;
    keepBug(null);
    let issue = null;
    try {
      issue = checkedIssue(await complete(opts, ISSUE, `What happened: ${what}\nWhat they expected: ${q}`, 500));
    } catch { /* drafted here instead */ }
    issue ??= checkedIssue(JSON.stringify({ title: what.split('\n')[0].slice(0, 80), body: `What happened:\n${what}\n\nWhat I expected:\n${q}` }));
    if (!issue) {
      say({ who: 'it', text: 'I couldn’t write that up. Try again with a few more words.' });
      return;
    }
    say({ who: 'it', text: `Here is a draft. Check it, change what you like, and submit it in Git. ${PUBLIC}`, issue, how: 'I post nothing myself' });
  }

  function openIssue(issue) {
    const url = issueUrl(map?.report, issue);
    if (!url) return;
    const at = opts.here(url);
    if (!at || at === '#') return;
    if (at.startsWith('/_dd/app/open')) fetch(at).catch(() => {});
    else window.open(at, '_blank', 'noopener');
  }

  async function news() {
    const { json, error } = await fetchJson('/_dd/releases.json');
    if (error) {
      say({ who: 'it', text: error === 'missing' ? 'I can’t tell yet: this Commonty doesn’t list its releases.' : 'I couldn’t check just now. Try again in a moment.' });
      return;
    }
    const text = releasesSay(json);
    const n = json.releases?.[0]?.n;
    if (Number.isInteger(n)) {
      local.set(KEYS.release, String(n));
      newDot(false);
    }
    say({ who: 'it', text: text || 'Nothing listed yet.', how: 'from the release notes' });
  }

  function showAround() {
    const { steps } = stepsHere(keyName(shortcut()));
    if (!steps.length) {
      say({ who: 'it', text: 'There is nothing to show on this page yet.' });
      return;
    }
    say({ who: 'it', text: `${steps.length} things on this page. Next and Back step through, Escape stops.` });
    open(false);
    local.set(KEYS.toured, '1');
    tour(keyName(shortcut()), () => btn.focus());
  }

  let busy = false;
  async function ask(q) {
    if (busy) return;
    say({ who: 'me', text: q });
    await loadMap();
    if (bug && !opts.demo) {
      await bugStep(q);
      return;
    }
    if (TOUR.test(q)) {
      showAround();
      return;
    }
    if (BUG.test(q) && !opts.demo && map?.report) {
      keepBug({ step: 'what' });
      say({ who: 'it', text: `Sorry about that. What happened? A sentence or two is enough. ${PUBLIC}` });
      return;
    }
    if (NEWS.test(q)) {
      await news();
      return;
    }
    if (STATUS(q)) {
      say({ who: 'it', text: await statusSay(q), how: 'from the services’ status' });
      return;
    }
    if (!map) {
      say({ who: 'it', text: 'The map of the site did not load. Try again in a moment.' });
      return;
    }
    const near = match(q);
    if (!isQuestion(q) && near.length) {
      places(near, 'matched in this page');
      return;
    }
    const found = sections(q);
    busy = true;
    form.setAttribute('aria-busy', 'true');
    const wait = el('div', { class: 'msg it', text: '…', role: 'status', 'aria-label': 'Looking it up' });
    log.append(wait);
    log.scrollTop = log.scrollHeight;
    try {
      const a = checked(await askModel(opts, q, found), byId);
      wait.remove();
      if (!a) throw new Unreachable('malformed');
      const ids = [...a.go];
      if (a.from && !ids.includes(a.from)) ids.push(a.from);
      // the section is ours, from what was looked up, never the model's word
      const section = a.from ? found.find((f) => f.n.id === a.from)?.h || null : null;
      if (!a.from && a.go.length === 1 && auto.checked) {
        say({ who: 'it', text: `${a.say ? `${a.say}\n` : ''}Took you to ${pathOf(byId[a.go[0]])}.`, how: 'asked the box’s model: it picked from the map' });
        go(a.go[0]);
        return;
      }
      say({ who: 'it', text: a.say || (ids.length > 1 ? 'A few places:' : 'Here:'), ids, src: a.from, section, how: a.from ? null : 'asked the box’s model: it picked from the map' });
      // one control of ours: pointed at, from the map's own name for it
      const t = byId[a.go[0]];
      if (a.go.length === 1 && t?.target && (!t.where || find(t.target))) go(t.id);
    } catch (e) {
      wait.remove();
      if (e instanceof SignIn && endpoint(opts) && !opts.demo) {
        say({ who: 'it', text: 'To answer questions I need to sign in to Chat on this device first. It takes a moment and brings you back here.', signIn: true });
        return;
      }
      // what can still be said without the model: the places and docs whose words match
      const docs = found.map((f) => f.n.id).filter((id, i, all) => all.indexOf(id) === i);
      const ids = [...near.map((n) => n.id), ...docs].filter((id, i, all) => all.indexOf(id) === i).slice(0, GO);
      const why = e instanceof Unreachable && e.message.length > 20 ? `${e.message} ` : '';
      if (ids.length) say({ who: 'it', text: `${why}These may help:`, ids, how: 'matched in this page' });
      else say({ who: 'it', text: `${why}I couldn't find that. Ask it another way, or name the page, like "devices" or "backups".` });
    } finally {
      busy = false;
      form.removeAttribute('aria-busy');
    }
  }

  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const q = input.value.trim();
    input.value = '';
    if (q) ask(q.slice(0, 300));
  });

  // open, hide, tuck away
  const open = (yes, focus = true) => {
    if (yes) face.end();
    panel.hidden = !yes;
    btn.setAttribute('aria-expanded', String(yes));
    btn.setAttribute('aria-label', yes ? 'Hide the assistant' : btn.classList.contains('has-new') ? 'Open the assistant - there is something new' : 'Open the assistant');
    tab.set(KEYS.open, yes ? '1' : '0');
    if (yes) {
      loadMap().then(() => { if (!panel.hidden) redraw(); });
      redraw();
      if (focus) input.focus();
    }
  };
  btn.addEventListener('click', () => {
    if (body.classList.contains('dd-tucked')) {
      tucked(false);
      open(true);
      return;
    }
    open(panel.hidden);
  });
  btn.addEventListener('pointerenter', () => loadMap(), { once: true });
  hide.addEventListener('click', () => { open(false); btn.focus(); });
  panel.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { open(false); btn.focus(); }
  });

  // A peek is never played unseen: once due, it waits for the tab to be
  // shown, the window to have focus and the member to have moved, scrolled
  // or typed in the last half minute, then plays two seconds later. Only a
  // peek that played counts toward the day's two; a due one waits across
  // pages.
  let peekTimer = null;
  let lastActive = 0;
  const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches;
  const today = () => new Date().toDateString();
  // the peeks played today, by this device's own clock, and the last one
  const peeked = () => {
    try {
      const p = JSON.parse(local.get(KEYS.peeked) || 'null');
      if (p && typeof p.last === 'number') return p.day === today() ? p : { day: today(), count: 0, last: p.last };
    } catch { /* none kept */ }
    return { day: today(), count: 0, last: 0 };
  };
  const dueAt = () => {
    const p = peeked();
    const midnight = new Date();
    midnight.setHours(24, 0, 0, 0);
    return Math.max((Number(local.get(KEYS.tucked)) || 0) + HOUR, p.last + APART, p.count >= A_DAY ? midnight.getTime() : 0);
  };
  const due = () => body.classList.contains('dd-tucked') && Number(local.get(KEYS.tucked)) > 1 && !reduced() && Date.now() >= dueAt();
  const seen = () => document.visibilityState === 'visible' && document.hasFocus() && Date.now() - lastActive < 30e3;
  function peek() {
    const p = peeked();
    local.set(KEYS.peeked, JSON.stringify({ day: p.day, count: p.count + 1, last: Date.now() }));
    body.classList.remove('dd-peek');
    dress(btn, faceFor(new Date()));
    void btn.offsetWidth;
    body.classList.add('dd-peek');
  }
  btn.addEventListener('animationend', (e) => {
    if (e.animationName !== 'dd-peek') return;
    body.classList.remove('dd-peek');
    undress(btn);
  });
  function schedule() {
    if (peekTimer || !body.classList.contains('dd-tucked') || !(Number(local.get(KEYS.tucked)) > 1) || reduced()) return;
    if (!due()) {
      // asleep until it is due, at most a day at a time
      peekTimer = setTimeout(() => { peekTimer = null; schedule(); }, Math.min(DAY, Math.max(1000, dueAt() - Date.now())));
      return;
    }
    if (!seen()) return;
    peekTimer = setTimeout(() => {
      peekTimer = null;
      if (due() && seen()) peek();
      else schedule();
    }, 2000);
  }
  const unschedule = () => { clearTimeout(peekTimer); peekTimer = null; };
  const active = () => { lastActive = Date.now(); schedule(); };
  const watch = [['pointermove', active], ['scroll', active], ['keydown', active], ['focus', schedule]];
  for (const [ev, f] of watch) addEventListener(ev, f, { passive: true });
  document.addEventListener('visibilitychange', schedule);
  function tucked(yes) {
    face.end();
    body.classList.toggle('dd-tucked', yes);
    body.classList.remove('dd-peek');
    undress(btn);
    tuck.setAttribute('aria-label', yes ? 'Bring the assistant back' : 'Tuck the assistant away');
    // tucked away, the button is out of the keyboard's way; the strip is the way back
    btn.tabIndex = yes ? -1 : 0;
    btn.toggleAttribute('aria-hidden', yes);
    if (yes && !panel.hidden) open(false);
    // when it was tucked, across pages: an hour from then it may peek
    if (yes && !(Number(local.get(KEYS.tucked)) > 1)) local.set(KEYS.tucked, String(Date.now()));
    if (!yes) local.set(KEYS.tucked, '0');
    unschedule();
    schedule();
  }
  tuck.addEventListener('click', () => {
    // mid-peek, the strip is under the button: a click there is on the button
    if (body.classList.contains('dd-peek')) {
      tucked(false);
      open(true);
      return;
    }
    tucked(!body.classList.contains('dd-tucked'));
  });

  if (Number(local.get(KEYS.tucked)) > 1) tucked(true);
  else if (tab.get(KEYS.open) === '1') open(true, false);
  // back from signing in to Chat: the question that needed it
  const again = tab.get(KEYS.retry);
  if (again) {
    tab.del(KEYS.retry);
    const last = convo.at(-1);
    if (last?.signIn) {
      convo.pop();
      const q = convo.at(-1)?.who === 'me' ? convo.pop().text : again;
      keep();
      if (body.classList.contains('dd-tucked')) tucked(false);
      open(true, false);
      ask(q);
    }
  }

  // The shortcut, from anywhere. The usual one leaves alone what uses it
  // itself - writing in a box, the Git pages' code - and one set on this
  // device is the member's choice.
  const onKey = (e) => {
    if (e.defaultPrevented || comboOf(e) !== shortcut()) return;
    if (!ownKey() && e.target.closest?.('textarea, [contenteditable=""], [contenteditable="true"], .code, [data-own-keys]')) return;
    e.preventDefault();
    if (!panel.hidden && panel.contains(document.activeElement)) {
      open(false);
      btn.focus();
      return;
    }
    if (body.classList.contains('dd-tucked')) tucked(false);
    open(true);
  };
  document.addEventListener('keydown', onKey);
  const onStorage = (e) => { if (e.key === KEYS.key) btn.setAttribute('aria-keyshortcuts', ariaKey(shortcut())); };
  addEventListener('storage', onStorage);

  // a dot on the button while there are release notes this device has not seen
  function newDot(yes) {
    btn.classList.toggle('has-new', yes);
    if (panel.hidden) btn.setAttribute('aria-label', yes ? 'Open the assistant - there is something new' : 'Open the assistant');
  }
  (async () => {
    let newest = Number(tab.get(KEYS.news)?.split(' ')[0]);
    if (!(Date.now() - Number(tab.get(KEYS.news)?.split(' ')[1]) < 600e3)) {
      const { json } = await fetchJson('/_dd/releases.json');
      newest = json?.releases?.[0]?.n;
      if (!Number.isInteger(newest)) return;
      tab.set(KEYS.news, `${newest} ${Date.now()}`);
    }
    if (!Number.isInteger(newest)) return;
    const saw = Number(local.get(KEYS.release));
    // a device's first sight of them is not news
    if (!local.get(KEYS.release)) local.set(KEYS.release, String(newest));
    else newDot(newest > saw);
  })();

  // a control asked for from another page, in the address or this tab's note
  const q = new URLSearchParams(location.search);
  const asked = q.get('dd-point');
  if (asked !== null) {
    q.delete('dd-point');
    history.replaceState(history.state, '', location.pathname + (q.size ? `?${q}` : '') + location.hash);
    loadMap().then(() => { if (all.some((n) => n.target === asked)) point(asked, { clear }); });
  } else {
    pointSoon();
  }

  // the first time on this device, once, a page with a tour of its own shows
  // itself around; Skip ends it, and "show me around" brings it back
  if (!local.get(KEYS.toured)) {
    setTimeout(() => {
      if (local.get(KEYS.toured) || !panel.hidden || inTour() || document.querySelector('.dd-ring')) return;
      const { steps, own } = stepsHere(keyName(shortcut()));
      if (!own || steps.length < 2) return;
      local.set(KEYS.toured, '1');
      tour(keyName(shortcut()));
    }, 1500);
  }

  return {
    remove() {
      face.stop();
      undress(btn);
      document.removeEventListener('keydown', onKey);
      removeEventListener('storage', onStorage);
      unschedule();
      for (const [ev, f] of watch) removeEventListener(ev, f);
      document.removeEventListener('visibilitychange', schedule);
      body.classList.remove('dd-tucked', 'dd-peek', 'dd-eyes');
      btn.remove();
      tuck.remove();
      panel.remove();
    },
  };
}
