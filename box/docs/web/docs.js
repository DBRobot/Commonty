// The docs: docs/ in the repository, drawn as pages. The gate hands over
// the index (the nav, and every page's title and headings) and each page's
// markdown as it is; this draws them, cleaned of anything that could run.
// Anyone may read them; a member also gets the way to the source.

import { me, slug, docsPath } from './shell.js';
import { $, el, load, prefetch } from './kit.js';

// where the pages are: /docs on the bare name, or the gate's own address
// for them anywhere else (a box's own name, a test)
const base = location.pathname.startsWith('/_dd/docs/page') ? '/_dd/docs/page' : '/docs';
const fileAt = (pathname) => {
  const rest = decodeURIComponent(pathname.slice(base.length)).replace(/^\/+|\/+$/g, '');
  return rest;
};
const addressOf = (file) => base + docsPath(file);

// ---- what the gate holds: drawn from what this browser kept, then fresh
// (kit.js load), so a page opened again is there at once

let index = null;
const rawUrl = (file) => '/_dd/docs/raw/' + file.split('/').map(encodeURIComponent).join('/');

let marked = null;
const markedReady = import('/_dd/web/marked.js').then((m) => { marked = m.marked; });

// a member sees the source on the forge; a stranger cannot reach it
let member = null;
me().then((m) => { member = m && m.role === 'member' ? m : null; draw(); }).catch(() => {});
const forge = () => (member && index?.repo ? `${new URL(member.home).origin.replace('://home.', '://git.')}/${index.repo}` : null);

// ---- the page

let file = null;
let view = 'page';

/// a page's file from the address: a folder's README, or the page itself
function resolve(pathname) {
  const at = fileAt(pathname);
  if (!index) return null;
  if (!at) return index.home || '';
  for (const f of [`${at}.md`, `${at}/README.md`, at]) if (index.pages[f]) return f;
  return null;
}

function order() {
  const out = index.home ? [index.home] : [''];
  for (const g of index.groups) out.push(...g.pages);
  return out;
}
const titleOf = (f) => (f === '' ? 'Docs' : index.pages[f]?.title || f);

function drawNav() {
  const nav = $('docs-nav');
  const link = (f, text, cls) => {
    const a = el('a', { href: addressOf(f), class: cls, text });
    if (f === file) a.setAttribute('aria-current', 'page');
    return a;
  };
  const parts = [link(index.home || '', index.home ? 'Docs home' : 'All pages', 'front')];
  for (const g of index.groups) {
    if (g.name) parts.push(el('h3', { text: g.name }));
    for (const f of g.pages) parts.push(link(f, titleOf(f)));
  }
  nav.replaceChildren(...parts);
}

function drawCrumb() {
  const crumb = $('docs-crumb');
  const path = file === '' ? 'docs/' : `docs/${file}`;
  const repo = forge();
  if (!repo) {
    crumb.replaceChildren(path);
    return;
  }
  const segs = path.split('/').filter(Boolean);
  const parts = [];
  segs.forEach((s, i) => {
    if (i) parts.push('/');
    const kind = i === segs.length - 1 && file !== '' ? 'blob' : 'tree';
    parts.push(el('a', { href: `${repo}/${kind}/main/${segs.slice(0, i + 1).join('/')}`, text: s }));
  });
  crumb.replaceChildren(...parts);
}

// the links inside a page: another page goes to its address here, anything
// else in the repository to the forge for a member, and stays words for a
// stranger, who cannot open it
function relink(md) {
  const dir = file.split('/').slice(0, -1);
  const repo = forge();
  for (const a of md.querySelectorAll('a[href]')) {
    const href = a.getAttribute('href');
    if (/^[a-z][a-z0-9+.-]*:/i.test(href) || href.startsWith('//')) {
      a.target = '_blank';
      a.rel = 'noopener';
      continue;
    }
    if (href.startsWith('#')) continue;
    const [p, hash] = href.split('#');
    const out = [...dir];
    let above = false;
    for (const s of p.split('/')) {
      if (s === '..') { if (!out.length) above = true; out.pop(); } else if (s && s !== '.') out.push(s);
    }
    const target = out.join('/');
    if (!above && /\.md$/.test(target)) {
      a.href = addressOf(target) + (hash ? `#${hash}` : '');
      if (!index.pages[target]) a.classList.add('missing');
      continue;
    }
    // past docs/: the repository itself, from its root
    const inRepo = (above ? out : ['docs', ...out]).join('/');
    a.classList.add('code');
    if (repo) {
      a.href = `${repo}/${p.endsWith('/') ? 'tree' : 'blob'}/main/${inRepo}${hash ? `#${hash}` : ''}`;
    } else {
      a.replaceWith(el('code', { text: a.textContent }));
    }
  }
}

function headings(md) {
  for (const h of md.querySelectorAll('h2, h3, h4')) {
    h.id = slug(h.textContent);
    h.prepend(el('a', { class: 'anchor', href: `#${h.id}`, 'aria-hidden': 'true', text: '#' }));
  }
}

function frontList() {
  const md = el('div', { class: 'docs-md' }, el('h1', { text: 'Docs' }));
  for (const g of index.groups) {
    if (g.name) md.append(el('h2', { text: g.name }));
    md.append(el('ul', { class: 'index' }, ...g.pages.map((f) => el('li', {}, el('a', { href: addressOf(f), text: titleOf(f) })))));
  }
  return md;
}

async function drawBody() {
  const body = $('docs-body');
  const want = file;
  if (want === null) {
    body.replaceChildren(el('div', { class: 'docs-md' },
      el('h1', { text: 'No page here' }),
      el('p', {}, 'Nothing in the docs is at this address. ', el('a', { href: base, text: 'Go to the docs home.' }))));
    return;
  }
  if (want === '') {
    body.replaceChildren(frontList());
    return;
  }
  // the page's markdown: kept, at once; the box's, when it differs
  let shown = false;
  load(rawUrl(want), (text) => {
    if (file !== want) return;
    shown = true;
    drawText(want, text);
  }, { as: 'text' }).catch(() => {
    if (file === want && !shown) {
      body.replaceChildren(el('p', { class: 'docs-wait', text: 'This page did not load. Try again in a moment.' }));
    }
  });
}

async function drawText(want, text) {
  const body = $('docs-body');
  if (view === 'md') {
    body.replaceChildren(el('pre', { class: 'docs-raw', text }));
    drawToc(null);
    return;
  }
  await markedReady;
  if (file !== want) return;
  const md = el('div', { class: 'docs-md' });
  const html = marked.parse(text, { gfm: true, breaks: false });
  md.innerHTML = window.DOMPurify ? window.DOMPurify.sanitize(html) : '';
  for (const t of md.querySelectorAll('table')) {
    const wrap = el('div', { class: 'table' });
    t.replaceWith(wrap);
    wrap.append(t);
  }
  relink(md);
  headings(md);
  if (want === index.home) md.querySelector('h1 + p')?.classList.add('lead');
  body.replaceChildren(md);
  drawToc(md);
}

function drawToc(md) {
  const toc = $('docs-toc');
  const parts = [];
  const hs = md ? [...md.querySelectorAll('h2, h3')] : [];
  if (hs.length) {
    parts.push(el('h3', { text: 'On this page' }));
    for (const h of hs) parts.push(el('a', { href: `#${h.id}`, class: h.tagName.toLowerCase(), 'data-id': h.id, text: h.textContent.replace(/^#/, '') }));
  }
  const meta = el('div', { class: 'docs-meta' });
  const repo = forge();
  if (repo && file) {
    meta.append(
      el('a', { href: `${repo}/commits/main/docs/${file}`, text: 'History' }));
  }
  parts.push(meta);
  toc.replaceChildren(...parts);
  spy();
}

function drawPager() {
  const pager = $('docs-pager');
  if (file === null || view === 'md') {
    pager.replaceChildren();
    return;
  }
  const all = order();
  const i = all.indexOf(file);
  const card = (f, cls, label) => el('a', { href: addressOf(f), class: cls }, el('small', { text: label }), el('b', { text: titleOf(f) }));
  pager.replaceChildren(
    ...(i > 0 ? [card(all[i - 1], 'prev', 'Previous')] : []),
    ...(i >= 0 && i < all.length - 1 ? [card(all[i + 1], 'next', 'Next')] : []));
}

function drawHead() {
  drawCrumb();
  const edit = $('docs-edit');
  const repo = forge();
  edit.hidden = !(repo && file);
  if (repo && file) edit.href = `${repo}/blob/main/docs/${file}`;
  for (const b of document.querySelectorAll('.docs-seg button')) b.hidden = !file;
  $('view-page').setAttribute('aria-pressed', String(view === 'page'));
  $('view-md').setAttribute('aria-pressed', String(view === 'md'));
}

async function draw(scroll) {
  if (!index) return;
  file = resolve(location.pathname);
  document.title = file ? `${titleOf(file)} · Docs · Commonty` : 'Docs · Commonty';
  drawNav();
  drawHead();
  drawPager();
  await drawBody();
  if (scroll === 'top') window.scrollTo(0, 0);
  if (location.hash) document.getElementById(decodeURIComponent(location.hash.slice(1)))?.scrollIntoView();
}

// ---- moving between pages without leaving this one

document.addEventListener('click', (e) => {
  const a = e.target.closest('a[href]');
  if (!a || e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || a.target) return;
  const u = new URL(a.href, location.href);
  if (u.origin !== location.origin || !(u.pathname === base || u.pathname.startsWith(base + '/'))) return;
  e.preventDefault();
  $('docs-nav').classList.remove('open');
  $('contents-btn').setAttribute('aria-expanded', 'false');
  if (u.pathname === location.pathname && u.hash) {
    history.pushState(null, '', u.href);
    document.getElementById(decodeURIComponent(u.hash.slice(1)))?.scrollIntoView();
    return;
  }
  history.pushState(null, '', u.href);
  draw(u.hash ? null : 'top');
});
addEventListener('popstate', () => draw());

// a page is asked for as the pointer reaches its link, so the click finds it
document.addEventListener('pointerover', (e) => {
  const a = e.target.closest?.('a[href]');
  if (!a || !index) return;
  const u = new URL(a.href, location.href);
  if (u.origin !== location.origin || !u.pathname.startsWith(base)) return;
  const f = resolve(u.pathname);
  if (f) prefetch(rawUrl(f), { as: 'text' });
});

$('view-page').onclick = () => { view = 'page'; drawHead(); drawPager(); drawBody(); };
$('view-md').onclick = () => { view = 'md'; drawHead(); drawPager(); drawBody(); };
$('contents-btn').onclick = (e) => {
  const open = $('docs-nav').classList.toggle('open');
  e.currentTarget.setAttribute('aria-expanded', String(open));
};

// the heading being read, lit in the contents
function spy() {
  const hs = [...document.querySelectorAll('.docs-md h2, .docs-md h3')];
  let on = hs[0];
  for (const h of hs) if (h.getBoundingClientRect().top < 120) on = h;
  for (const a of document.querySelectorAll('.docs-toc a[data-id]')) a.classList.toggle('on', !!on && a.dataset.id === on.id);
}
addEventListener('scroll', spy, { passive: true });

// ---- start: what was kept draws now, the box's answer when it comes

load('/_dd/docs/index.json', (i) => {
  index = i;
  draw();
}).catch((e) => {
  if (!index) $('docs-body').replaceChildren(el('p', { class: 'docs-wait', text: `The docs did not load (${e.message}). Try again in a moment.` }));
});
