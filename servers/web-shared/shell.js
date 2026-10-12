// Every signed-in page: who is looking, the bar's menu, and on the home
// page the services. One answer from /_dd/me fills them all, so the page
// itself is a plain file: the box serves it to a browser and the app
// carries its own copy, and both look the same because they are the same.

import { webUrl } from './kit.js';
import { start as guide } from './guide.js';
// and the footer, after the page
import './footer.js';

let answer = null;

// A page anyone may read (the docs) marks its bar so: someone signed out
// is shown the way in there, not sent through it.
const open = !!document.querySelector('header.dd-bar[data-public]');

/// who this is and what they may open; asked once per page. On an open
/// page, null for someone who is not signed in.
export function me() {
  answer ??= fetch('/_dd/me').then(async (r) => {
    if (open && (r.status === 401 || r.status === 403)) return null;
    if (r.status === 401) {
      location.href = '/_dd/login?rd=' + encodeURIComponent(location.pathname);
      throw new Error('signed out');
    }
    if (!r.ok) throw new Error(`the box said ${r.status}`);
    return r.json();
  });
  return answer;
}

let config = null;
const entries = new Map();

/// which domain passkeys are for; asked once per page
export function pageConfig() {
  config ??= fetch('/_dd/config').then((r) => r.json());
  return config;
}

/// someone's signed entry, as the directory's text; asked once per page,
/// or again after `forgetEntry` when it has just been changed
export function entryText(user) {
  if (!entries.has(user)) {
    entries.set(user, fetch('/_dd/directory/' + encodeURIComponent(user)).then((r) => {
      if (!r.ok) { entries.delete(user); throw new Error(`the box said ${r.status}`); }
      return r.text();
    }));
  }
  return entries.get(user);
}
export const entryOf = async (user) => JSON.parse(await entryText(user));
export function forgetEntry(user) { entries.delete(user); }

/// Running inside the app rather than a browser tab: the app serves these
/// same files under its own address (app/src/site.rs), and answers the
/// library's key itself instead of a passkey.
// Photos in the app is Ente's page, on the photos host, in the app's own
// window. It knows it is there by what the app's page left in this tab
// (photos.js), and its bar goes back to the app's pages, not the site's.
const appOrigin = (() => {
  try { return sessionStorage.getItem('dd-app-origin'); } catch { return null; }
})();
export const inApp = location.protocol === 'commonty:' || location.hostname === 'commonty.localhost' || !!appOrigin;
/// Served by the app to a browser it let in (app/src/local.rs): the same
/// pages, on this computer, under the name the app gave this browser. The
/// app answers the keys here too, to this browser's own session.
export const servedByApp = /^s-[0-9a-f]{32}\.localhost$/.test(location.hostname);

/// a request to the app's own routes (/_dd/app/...), with the secret the
/// app put in this page when it served it: the app answers nothing else
export function appFetch(url, init = {}) {
  const secret = document.querySelector('meta[name="dd-app"]')?.content || '';
  return fetch(url, { ...init, headers: { ...(init.headers || {}), 'x-dd-app': secret } });
}
// where the app's pages are, from wherever this page is
const appPages = appOrigin || '';

// the app's own screen for this device, where it signs out; ?stay keeps
// it from coming straight back here
const deviceScreen = ((appOrigin || location.origin).startsWith('commonty:') ? 'tauri://localhost/' : 'http://tauri.localhost/') + '?stay';

// pages the app carries: a service that is one of them opens the app's
// copy, Photos opens in the app's window, and any other (games, code)
// opens in the device's browser
const carried = ['/_dd/home', '/_dd/files', '/_dd/media', '/_dd/settings'];
// In a browser the app let in: its pages here, Chat too, and Photos and
// Passwords through the app's own door for this browser (/_dd/app/go)
function served(url) {
  try {
    const u = new URL(url, location.href);
    if (carried.includes(u.pathname) || u.pathname === '/_dd/chat') return u.pathname + u.hash;
    if (u.hostname.startsWith('llm.')) return '/_dd/chat';
    if (u.pathname === '/_dd/photos') return '/_dd/app/go?service=photos';
    if (u.hostname.startsWith('vault.')) return '/_dd/app/go?service=vault';
    return url;
  } catch {
    return url;
  }
}
// The bar's addresses come from a box (/_dd/me): a path on this page's own
// origin, or a web address on the fleet's own domain, and nothing else
// (javascript:, data:, another site). The domain is what the app wrote into
// a page it serves (never what a box says), or this page's own name's on a box.
const fleet = document.querySelector('meta[name="dd-domain"]')?.content || location.hostname.split('.').slice(-2).join('.');
function linkable(url) {
  if (/^\/(?![/\\])/.test(String(url))) return url;
  const w = webUrl(url);
  if (!w) return '#';
  const h = new URL(w).hostname;
  return h === fleet || h.endsWith('.' + fleet) ? w : '#';
}
/// where this page may send someone for an address of the fleet's: the
/// address itself in a browser, the app's own copy or door in the app
export function here(raw) {
  const url = linkable(raw);
  if (servedByApp) return served(url);
  if (!inApp) return url;
  try {
    const u = new URL(url, location.href);
    if (carried.includes(u.pathname)) return appPages + u.pathname + u.hash;
    if (u.pathname === '/_dd/photos') return appPages + '/_dd/app/photos';
    // Passwords too: the app serves the vault's code itself (app/src/local.rs)
    if (u.hostname.startsWith('vault.')) return appPages + '/_dd/app/vault';
    // in the app's window already (Photos): the window may go there itself
    if (appOrigin) return url;
    if (u.origin !== location.origin) return '/_dd/app/open?url=' + encodeURIComponent(u.href);
    return url;
  } catch {
    return url;
  }
}

// A link out of the app (Photos, Games, Code) is the app's to open in the
// device's browser. Followed as a link, the window went to an answer with
// nothing in it and stayed blank; asked for instead, the page stays put.
if (inApp) {
  document.addEventListener('click', (e) => {
    const a = e.target.closest?.('a[href^="/_dd/app/open"]');
    if (!a) return;
    e.preventDefault();
    fetch(a.getAttribute('href')).catch(() => {});
  });
}

function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else e.setAttribute(k, v);
  }
  e.append(...kids);
  return e;
}

// Signing out and opening the demo change the session, so the gate takes
// them as a POST from our own pages only. The link still works on its own:
// it opens a page with the same button.
function posting(a) {
  a.addEventListener('click', (e) => {
    e.preventDefault();
    const f = el('form', { method: 'post', action: a.href, hidden: '' });
    document.body.append(f);
    f.submit();
  });
  return a;
}

// A setting that belongs at hand (Ad blocking at home): its name opens its
// page, the switch beside it flips it. Its state is asked for when the menu
// opens, not on every page.
function toggleRow(item) {
  const sw = el('button', { type: 'button', class: 'switch', role: 'switch', 'aria-checked': 'false', 'aria-label': item.label });
  sw.disabled = true;
  const show = (s) => {
    sw.disabled = false;
    sw.setAttribute('aria-checked', String(!!s.on));
    sw.title = s.on ? 'On' : s.resumesIn ? 'Paused' : 'Off';
  };
  const read = () => fetch(item.toggle).then((r) => (r.ok ? r.json() : Promise.reject(r))).then(show).catch(() => { sw.title = 'Not answering'; });
  sw.addEventListener('click', async (e) => {
    e.preventDefault();
    const on = sw.getAttribute('aria-checked') !== 'true';
    sw.disabled = true;
    try {
      const r = await fetch(item.switch, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ on }) });
      if (r.ok) show(await r.json()); else read();
    } catch { read(); }
  });
  const row = el('div', { class: 'toggle' }, el('a', { href: here(item.home || item.url), text: item.label }), sw);
  row.read = read;
  return row;
}

// The bar's own pages live on the home site. On another of the fleet's
// sites (Passwords, Git, Metrics) a bare /_dd/ link would open them there,
// and every page after it would stay on that site.
function homeLink(m, url) {
  return url.startsWith('/') && m.home ? m.home + url : url;
}

// Every page's tab shows the Commonty mark. Without one of its own, a
// browser keeps whatever icon it last stored for the address.
if (!document.querySelector('link[rel~="icon"]')) {
  document.head.append(Object.assign(document.createElement('link'), { rel: 'icon', type: 'image/svg+xml', href: '/_dd/static/favicon.svg' }));
}

// The first stop for a keyboard: past the bar, to the page. Focus is moved
// by hand, since the docs and Git pages take over clicks on their links.
function skipLink() {
  const main = document.querySelector('main');
  if (!main || document.querySelector('.skip-to')) return;
  main.id ||= 'main';
  if (!main.hasAttribute('tabindex')) main.tabIndex = -1;
  const a = el('a', { class: 'skip-to', href: '#' + main.id, text: 'Skip to content' });
  a.addEventListener('click', (e) => {
    e.preventDefault();
    main.focus();
    main.scrollIntoView();
  });
  document.body.prepend(a);
}
skipLink();

// the name is the control: it opens everything that is not a service
function bar(m, slot) {
  const nav = el('nav');
  m.menu.forEach((group, i) => {
    if (i) nav.append(el('hr'));
    for (const item of group) {
      // the app has no browser session to end; its own screen is where
      // this device signs out
      if (inApp && item.url === '/_dd/logout') {
        nav.append(el('a', { href: deviceScreen, text: 'This device' }));
        continue;
      }
      if (item.url === '/_dd/logout') {
        const out = posting(el('a', { href: homeLink(m, item.url), text: item.label }));
        // a browser the app let in leaves the app's session as it signs out
        if (servedByApp) {
          out.addEventListener('click', () => {
            fetch('/_dd/app/leave', { method: 'POST', keepalive: true }).catch(() => {});
          }, { capture: true });
        }
        nav.append(out);
        continue;
      }
      if (item.toggle) {
        nav.append(toggleRow({ ...item, home: homeLink(m, item.url) }));
        continue;
      }
      nav.append(el('a', { href: here(homeLink(m, item.url)), text: item.label }));
    }
  });
  const summary = el('summary', {},
    el('span', { class: 'avatar', 'aria-hidden': 'true', text: m.initial }),
    el('span', { text: m.user }),
    el('span', { class: 'chev', 'aria-hidden': 'true' }));
  // the logo goes home: the app's home, when this page is in the app
  const brand = document.querySelector('header.dd-bar .brand');
  if (brand && appOrigin) brand.setAttribute('href', appOrigin + '/_dd/home');
  else if (brand) brand.setAttribute('href', homeLink(m, '/_dd/home'));
  const menu = el('details', { class: 'menu', 'data-guide': 'menu' }, summary, nav);
  menu.addEventListener('toggle', () => {
    if (menu.open) for (const t of nav.querySelectorAll('.toggle')) t.read();
  });
  slot.replaceChildren(menu);
  // a menu closes when you are done with it: a click anywhere else, or Escape
  document.addEventListener('click', (e) => {
    if (menu.open && !menu.contains(e.target)) menu.open = false;
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && menu.open) {
      menu.open = false;
      summary.focus();
    }
  });
}

// A service's mark: the box's own svg, not anything a person wrote
function mark(s) {
  const m = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  m.setAttribute('viewBox', '0 0 20 20');
  m.setAttribute('fill', 'none');
  m.setAttribute('stroke-width', '1.5');
  m.setAttribute('aria-hidden', 'true');
  m.innerHTML = s.icon;
  return m;
}

// Left of the name: every service, to go from one to another without
// going home first, in the home page's order; and Friends.
function links(m) {
  const row = document.querySelector('header.dd-bar .bar');
  const brand = row?.querySelector('.brand');
  if (!row || !brand || row.querySelector('.links')) return;
  const nav = el('nav', { class: 'links', 'aria-label': 'Main' });
  const open = m.services.filter((s) => !s.shut);
  if (open.length) {
    const chev = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    chev.setAttribute('viewBox', '0 0 12 12');
    chev.setAttribute('aria-hidden', 'true');
    chev.innerHTML = '<path d="M2.5 4.5 6 8l3.5-3.5" fill="none" stroke="currentColor" stroke-width="1.6"/>';
    const btn = el('button', { type: 'button', class: 'svc-btn', 'aria-expanded': 'false', 'aria-controls': 'dd-services' }, el('span', { text: 'Services' }), chev);
    const list = el('div', { class: 'svc-menu', id: 'dd-services' });
    list.hidden = true;
    for (const s of open) {
      const a = el('a', { class: 's', href: here(s.url) },
        mark(s),
        el('span', {}, el('b', { text: s.name }), el('span', { text: s.blurb })));
      if (s.color) a.style.setProperty('--c', s.color);
      if (s.host === location.host) a.setAttribute('aria-current', 'page');
      list.append(a);
    }
    list.append(el('a', { class: 'all', href: here(m.home + '/_dd/home'), text: 'All services on the home page' }));
    const show = (on) => {
      list.hidden = !on;
      btn.setAttribute('aria-expanded', String(on));
    };
    btn.onclick = (e) => {
      e.stopPropagation();
      show(list.hidden);
    };
    document.addEventListener('click', (e) => {
      if (!list.hidden && !list.contains(e.target)) show(false);
    });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && !list.hidden) {
        show(false);
        btn.focus();
      }
    });
    nav.append(el('div', { class: 'svc' }, btn, list));
  }
  if (!m.demo) {
    const f = el('a', { class: 'link', href: here(m.home + '/_dd/friends'), text: 'Friends' });
    if (location.pathname === '/_dd/friends') f.setAttribute('aria-current', 'page');
    nav.append(f);
  }
  brand.after(nav);
}

// a guest's home: what they are, and a way to see the rest
function guestHome(m) {
  const intro = document.querySelector('.intro');
  if (intro) {
    intro.querySelector('h1').textContent = `Hello, ${m.user}`;
    intro.querySelector('p').textContent = m.guestOf
      ? `You're a guest of ${m.guestOf}'s. You can join the game servers your friends invite you to.`
      : "You're a guest here. You can join the game servers your friends invite you to.";
  }
  const main = document.querySelector('main.home');
  if (!main || main.querySelector('.guest-more')) return;
  main.append(el('section', { class: 'guest-more' },
    el('h2', { text: 'Want to see what else Commonty can do?' }),
    el('p', { text: 'Files, photos, films and more, kept on boxes the people in it own. The demo shows a member\'s view. Opening it signs you out of your account here; sign back in after.' }),
    posting(el('a', { class: 'button', href: m.home + '/_dd/demo', text: 'Try the demo' }))));
}

// a suspended member: why, and until when they can take their files
function suspendedHome(m) {
  const s = m.suspended || {};
  const until = s.until ? new Date(s.until * 1000).toLocaleDateString() : 'the end of the suspension';
  const intro = document.querySelector('.intro');
  if (!intro) return;
  intro.querySelector('h1').textContent = 'Your account is suspended';
  intro.querySelector('p').textContent = `The reason given: ${s.reason || 'none'}. Until ${until} you can open and take your own files; nothing of yours is read, changed or deleted before then. After that, everything of yours is deleted, unless the suspension is lifted.`;
}

// the home page: one row per service
function services(m, list) {
  if (!m.services.length) {
    list.replaceWith(el('p', { class: 'empty', text: 'Nothing runs here yet.' }));
    return;
  }
  list.replaceChildren(...m.services.map((s) => {
    const row = s.shut
      ? el('div', { class: 'service off', 'aria-disabled': 'true' })
      : el('a', { class: 'service', href: here(s.url) });
    if (s.color) row.style.setProperty('--c', s.color);
    row.append(
      mark(s),
      el('span', {},
        el('span', { class: 'name', text: s.name }),
        el('span', { class: 'blurb', text: s.shut ? 'Not in the demo.' : s.blurb })),
      el('span', { class: 'host', text: s.host }),
      el('span', { class: 'open', text: s.shut ? 'Shut' : 'Open' }));
    return el('li', { 'data-guide': `tile-${slug(s.name)}` }, row);
  }));
}

// ---- the search in the bar, and the docs beside it

/// A heading's anchor, the same here and on the docs page
export function slug(text) {
  return text.toLowerCase().replace(/[^\p{L}\p{N}\s-]/gu, '').trim().replace(/\s+/g, '-');
}

// The fleet's bare name, where the docs are, from wherever this page is:
// home.<domain> once someone signed in says it, else this host less its
// first label, and the host itself when it is the bare name already.
let site = null;
export function docsUrl(path = '') {
  if (site) return `https://${site}/docs${path}`;
  const parts = location.hostname.split('.');
  if (parts.length <= 2 || inApp) return `/docs${path}`;
  return `https://${parts.slice(1).join('.')}/docs${path}`;
}

/// A page's own results in the bar's search, above the services and the
/// docs: `find(text)` answers [{ title, sub, href, icon? }], and `more(text)`,
/// if given, is where all of them are.
const sources = [];
export function searchFrom(source) {
  sources.push(source);
}

const glass = '<circle cx="11" cy="11" r="7"/><path d="m20 20-3.5-3.5"/>';
const docIcon = '<path d="M6 3h8l4 4v14H6z"/><path d="M14 3v4h4M9 12h6M9 16h6"/>';
function svg(viewBox, inner, cls) {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('viewBox', viewBox);
  s.setAttribute('aria-hidden', 'true');
  if (cls) s.setAttribute('class', cls);
  s.innerHTML = inner;
  return s;
}

let docsIndex = null;
function docs() {
  docsIndex ??= fetch('/_dd/docs/index.json').then((r) => (r.ok ? r.json() : null)).catch(() => null);
  return docsIndex;
}

/// a page path under docs/ as its address: README.md is the docs' front
export function docsPath(file) {
  return file === 'README.md' ? '' : '/' + file.replace(/\.md$/, '').replace(/\/README$/, '');
}

function findDocs(index, text) {
  const w = text.toLowerCase();
  const hits = [];
  for (const [file, p] of Object.entries(index.pages)) {
    const at = docsPath(file);
    if (p.title.toLowerCase().includes(w)) hits.push({ rank: 0, title: p.title, href: docsUrl(at) });
    for (const [, h] of p.headings) {
      if (h.toLowerCase().includes(w)) hits.push({ rank: 1, title: h, sub: p.title, href: docsUrl(at) + '#' + slug(h) });
    }
    if (!p.title.toLowerCase().includes(w)) {
      const i = p.text.toLowerCase().indexOf(w);
      if (i >= 0) {
        const from = Math.max(0, p.text.lastIndexOf(' ', Math.max(0, i - 40)));
        hits.push({ rank: 2, title: p.title, sub: '…' + p.text.slice(from, i + w.length + 60).trim() + '…', href: docsUrl(at) });
      }
    }
  }
  return hits.sort((a, b) => a.rank - b.rank).slice(0, 6);
}

// the words searched for, marked where they show
function marked(text, w) {
  const out = document.createDocumentFragment();
  const low = text.toLowerCase();
  let at = 0;
  for (let i = low.indexOf(w); w && i >= 0; i = low.indexOf(w, i + w.length)) {
    out.append(text.slice(at, i), el('mark', { text: text.slice(i, i + w.length) }));
    at = i + w.length;
  }
  out.append(text.slice(at));
  return out;
}

function tools() {
  const row = document.querySelector('header.dd-bar .bar');
  if (!row || row.querySelector('.dd-search')) return;
  // before the name, or whatever a page keeps at the right end of its bar
  const after = row.querySelector('#me') || row.querySelector('.brand ~ :not(.links)');

  const docsLink = el('a', { class: 'docs-link', href: here(docsUrl()), text: 'Docs' });
  if (location.pathname === '/docs' || location.pathname.startsWith('/docs/')) docsLink.setAttribute('aria-current', 'page');

  const input = el('input', { id: 'dd-q', type: 'search', placeholder: 'Search Commonty', 'aria-label': 'Search Commonty', autocomplete: 'off', spellcheck: 'false' });
  const found = el('div', { class: 'found', role: 'listbox' });
  found.hidden = true;
  const opener = el('button', { type: 'button', class: 'open-search', 'aria-label': 'Search' }, svg('0 0 24 24', glass));
  const form = el('form', { class: 'dd-search', role: 'search', 'data-guide': 'search' }, opener, svg('0 0 24 24', glass, 'glass'), input, el('kbd', { text: '/' }), found);
  row.insertBefore(docsLink, after);
  row.insertBefore(form, after);

  const close = () => {
    found.hidden = true;
    form.classList.remove('open');
  };
  let asked = 0;
  async function show() {
    const text = input.value.trim();
    const w = text.toLowerCase();
    const n = ++asked;
    if (!text) {
      found.hidden = true;
      return;
    }
    const item = (r, cls, icon, color) => {
      const ic = el('span', { class: 'ic' }, icon);
      if (color) ic.style.setProperty('--c', color);
      return el('a', { class: cls, href: here(r.href), role: 'option' }, ic,
        el('span', {}, el('b', {}, marked(r.title, w)), ...(r.sub ? [el('small', {}, marked(r.sub, w))] : [])));
    };
    const ours = await Promise.all(sources.map((s) => Promise.resolve(s.find(text)).catch(() => [])));
    const m = await (answer || Promise.resolve(null)).catch(() => null);
    const index = await docs();
    if (n !== asked) return;
    const parts = [];
    sources.forEach((s, i) => {
      if (!ours[i].length && !s.more) return;
      parts.push(el('h4', { text: s.name }));
      for (const r of ours[i].slice(0, 6)) parts.push(item(r, 'plain', svg('0 0 24 24', r.icon || docIcon)));
      if (s.more) parts.push(el('a', { class: 'more', href: s.more(text), text: `All results for “${text}”` }));
    });
    const svcs = (m?.services || []).filter((s) => !s.shut && `${s.name} ${s.blurb}`.toLowerCase().includes(w));
    if (svcs.length) {
      parts.push(el('h4', { text: 'Services' }));
      for (const s of svcs) {
        const ic = svg('0 0 20 20', s.icon);
        parts.push(item({ title: s.name, sub: s.blurb, href: s.url }, '', ic, s.color));
      }
    }
    const hits = index ? findDocs(index, text) : [];
    if (hits.length) {
      parts.push(el('h4', { text: 'Docs' }));
      for (const h of hits) parts.push(item(h, 'plain', svg('0 0 24 24', docIcon)));
    }
    if (!parts.length) parts.push(el('p', { text: `Nothing matches “${text}”.` }));
    found.replaceChildren(...parts);
    found.hidden = false;
  }
  input.addEventListener('focus', () => docs());
  input.addEventListener('input', show);
  const step = (by) => {
    const all = [...found.querySelectorAll('a')];
    if (!all.length) return;
    const at = all.findIndex((a) => a.classList.contains('on'));
    all[at]?.classList.remove('on');
    const next = all[(at + by + all.length) % all.length];
    next.classList.add('on');
    next.scrollIntoView({ block: 'nearest' });
  };
  input.addEventListener('keydown', (e) => {
    if (e.key === 'ArrowDown') { e.preventDefault(); step(1); }
    if (e.key === 'ArrowUp') { e.preventDefault(); step(-1); }
    if (e.key === 'Escape') { close(); input.blur(); }
  });
  form.addEventListener('submit', (e) => {
    e.preventDefault();
    const pick = found.querySelector('a.on') || found.querySelector('a');
    if (pick) pick.click();
  });
  found.addEventListener('click', () => { close(); input.value = ''; });
  opener.addEventListener('click', () => {
    form.classList.add('open');
    input.focus();
  });
  document.addEventListener('click', (e) => {
    if (!form.contains(e.target)) close();
  });
  document.addEventListener('keydown', (e) => {
    if (e.key !== '/' || e.ctrlKey || e.metaKey || e.altKey) return;
    const t = e.target;
    if (t.closest?.('input, textarea, select, [contenteditable=""], [contenteditable="true"]')) return;
    e.preventDefault();
    form.classList.add('open');
    input.focus();
  });
  // the docs are on the bare name, which the signed-in answer says for sure
  me().then((m) => {
    if (!m?.home) return;
    site = new URL(m.home).hostname.replace(/^home\./, '');
    docsLink.href = here(docsUrl());
  }).catch(() => {});
}
tools();

// someone signed out, on a page anyone may read: the way in
function signedOut(slot) {
  const parts = location.hostname.split('.');
  const base = parts.length > 2 ? parts.slice(1).join('.') : location.hostname;
  slot.replaceChildren(el('a', { class: 'sign-in', href: `https://home.${base}/`, text: 'Sign in' }));
}

me().then((m) => {
  const slot = document.getElementById('me');
  if (!m) {
    if (slot) signedOut(slot);
    return;
  }
  if (slot) bar(m, slot);
  links(m);
  if (m.role === 'guest' && document.getElementById('services')) guestHome(m);
  if (m.role === 'suspended') suspendedHome(m);
  const list = document.getElementById('services');
  if (list) services(m, list);
  const banner = document.getElementById('demo');
  if (banner) banner.hidden = !m.demo;
  for (const e of document.querySelectorAll('[data-user]')) e.dataset.user = m.user;
  // the assistant in the corner, for members and the demo, on a page with
  // the look it needs; Chat is a conversation of its own already
  const chat = location.pathname === '/_dd/chat' || location.hostname.startsWith('llm.');
  if ((m.role === 'member' || m.role === 'demo') && !chat && document.querySelector('link[href$="/home.css"]')) {
    guide({ here, inApp, servedByApp, demo: !!m.demo });
  }
}).catch((e) => {
  const list = document.getElementById('services');
  if (list) list.replaceWith(el('p', { class: 'empty', text: String(e.message || e) }));
});
