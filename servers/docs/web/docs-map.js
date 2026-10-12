// The site, drawn: the site map (/_dd/sitemap.json) as the Git pages draw
// a run - a card for each place, a line to each from where it hangs. The
// same map the assistant in the corner goes by. A docs page asks for it
// with a fenced block of the language `sitemap`; a card goes where it says.

import { el } from './kit.js';
import { here } from './shell.js';

// a column for each level, and the room between cards
const COL = 236;
const GAP = 14;
// what a card lists of a page's headings before "and N more"
const SUBS = 6;

const dot = (kind) => el('i', { class: `dot ${kind === 'docs' ? 'dot-docs' : kind === 'app' ? 'dot-app' : ''}` });

function card(n) {
  const subs = n.subs || [];
  return el('a', { class: 'job', href: here(n.url), 'data-id': n.id },
    el('div', {}, dot(n.kind),
      el('span', { class: 't' }, n.group ? el('span', { class: 'grp', text: n.group }) : null, n.label),
      n.kind === 'app' ? el('small', { text: 'app' }) : null),
    subs.slice(0, SUBS).map((s) => el('div', { class: 'sub', text: s })),
    subs.length > SUBS ? el('div', { class: 'sub more', text: `and ${subs.length - SUBS} more` }) : null);
}

function draw(slot, places) {
  const byId = Object.fromEntries(places.map((p) => [p.id, p]));
  const kids = (id) => places.filter((p) => p.parent === id);
  const depth = (p) => {
    let d = 0;
    for (let n = p; n.parent && byId[n.parent] && d < 8; n = byId[n.parent]) d++;
    return d;
  };
  const cards = el('div', { class: 'cards' });
  const edges = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  edges.setAttribute('class', 'edges');
  edges.setAttribute('aria-hidden', 'true');
  cards.append(edges);
  const at = {};
  for (const p of places) {
    at[p.id] = card(p);
    cards.append(at[p.id]);
  }
  const dag = el('div', { class: 'dag', tabindex: '0', role: 'region', 'aria-label': 'The site map' }, cards);
  slot.replaceChildren(el('div', { class: 'box' },
    el('header', {}, el('b', { text: 'Every place on the site' }), el('span', { class: 'spacer' }),
      el('div', { class: 'legend' },
        el('span', {}, dot('page'), 'A page'),
        el('span', {}, dot('app'), 'Opens through the app'),
        el('span', {}, dot('docs'), 'Docs'))),
    dag));

  // layered: the level is the column, leaves stack, a parent sits level
  // with its first child
  let y = 0;
  let deepest = 0;
  const place = (p, seen) => {
    if (seen.has(p.id)) return;
    seen.add(p.id);
    const c = at[p.id];
    const d = depth(p);
    deepest = Math.max(deepest, d);
    c.style.left = `${d * COL}px`;
    const ch = kids(p.id);
    if (!ch.length) {
      c.style.top = `${y}px`;
      y += c.offsetHeight + GAP;
      return;
    }
    const top = y;
    for (const k of ch) place(k, seen);
    c.style.top = `${top}px`;
  };
  const seen = new Set();
  for (const root of places.filter((p) => !p.parent || !byId[p.parent])) {
    place(root, seen);
    y += 18;
  }
  const w = deepest * COL + 188;
  cards.style.width = `${w}px`;
  cards.style.height = `${y}px`;
  for (const p of places) {
    const a = at[p.parent];
    const b = at[p.id];
    if (!a) continue;
    const x1 = a.offsetLeft + a.offsetWidth;
    const y1 = a.offsetTop + 17;
    const x2 = b.offsetLeft;
    const y2 = b.offsetTop + 17;
    const mx = x1 + (x2 - x1) / 2;
    const r = Math.min(6, Math.abs(y2 - y1) / 2);
    const s = Math.sign(y2 - y1);
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    path.setAttribute('d', y1 === y2
      ? `M${x1},${y1} L${x2},${y2}`
      : `M${x1},${y1} L${mx - r},${y1} Q${mx},${y1} ${mx},${y1 + s * r} L${mx},${y2 - s * r} Q${mx},${y2} ${mx + r},${y2} L${x2},${y2}`);
    path.dataset.to = p.id;
    edges.append(path);
  }
  // the way to a card, lit while it is pointed at or focused
  const mark = (id) => {
    for (const c of cards.querySelectorAll('.job.here')) c.classList.remove('here');
    for (const e of edges.querySelectorAll('path.on')) e.classList.remove('on');
    for (let n = byId[id], i = 0; n && i < 8; n = byId[n.parent], i++) {
      at[n.id].classList.add('here');
      edges.querySelector(`path[data-to="${CSS.escape(n.id)}"]`)?.classList.add('on');
    }
  };
  cards.addEventListener('pointerover', (e) => { const c = e.target.closest('.job'); if (c) mark(c.dataset.id); });
  cards.addEventListener('focusin', (e) => { const c = e.target.closest('.job'); if (c) mark(c.dataset.id); });
}

/// Draw the map in place of `slot`: the places this person may open, or
/// what to do to see them
export async function drawMap(slot) {
  slot.replaceChildren(el('div', { class: 'box map-wait', 'aria-busy': 'true', 'aria-label': 'Drawing the map' }));
  let r = null;
  try { r = await fetch('/_dd/sitemap.json'); } catch { /* said below */ }
  if (r?.status === 401 || r?.status === 403) {
    slot.replaceChildren(el('p', { class: 'map-note', text: 'Sign in as a member, or open the demo, to see the map drawn here: it shows the places you can open.' }));
    return;
  }
  const m = r?.ok ? await r.json().catch(() => null) : null;
  if (!Array.isArray(m?.places)) {
    slot.replaceChildren(el('p', { class: 'map-note', text: 'The map did not load. Try again in a moment.' }));
    return;
  }
  draw(slot, m.places.filter((p) => typeof p.id === 'string' && typeof p.label === 'string' && typeof p.url === 'string'));
}
