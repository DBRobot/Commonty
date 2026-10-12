// Pointing at a control, and a short tour of a page (guide.js). Only what
// our own pages mark with data-guide is ever found, by its name; the words
// said at each step are ours, from the table below, never the model's.

import { el } from './kit.js';

const NAME = /^[a-z0-9-]{1,40}$/;
const reduced = () => matchMedia('(prefers-reduced-motion: reduce)').matches;

/// the control our page marks with this name, if it shows
export function find(name) {
  if (typeof name !== 'string' || !NAME.test(name)) return null;
  return [...document.querySelectorAll(`[data-guide="${name}"]`)].find((e) => e.getClientRects().length > 0 && !e.closest('[aria-hidden]:not([aria-hidden="false"])')) || null;
}

// a ring around a control, kept on it as the page scrolls, and an arrow
// over it that bounces
function ring(target, cls) {
  const r = el('div', { class: `dd-ring ${cls || ''}`, 'aria-hidden': 'true' });
  const tip = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  tip.setAttribute('viewBox', '0 0 20 24');
  tip.setAttribute('class', 'tip');
  const p = document.createElementNS('http://www.w3.org/2000/svg', 'path');
  p.setAttribute('d', 'M10 2v18m-7-7 7 7 7-7');
  tip.append(p);
  r.append(tip);
  document.body.append(r);
  let raf = 0;
  const place = () => {
    const b = target.getBoundingClientRect();
    r.style.left = `${b.left - 5}px`;
    r.style.top = `${b.top - 5}px`;
    r.style.width = `${b.width + 10}px`;
    r.style.height = `${b.height + 10}px`;
    r.classList.toggle('under', b.top < 48);
    raf = requestAnimationFrame(place);
  };
  place();
  return () => { cancelAnimationFrame(raf); r.remove(); };
}

let pointing = null;

/// Point at a marked control for a few seconds: waits a while for it to
/// show (a tab opening, a list loading), and says whether it did. `clear`
/// is told the control first, to get out of its way.
export function point(name, { wait = 8000, hold = 5000, clear } = {}) {
  pointing?.();
  return new Promise((done) => {
    const until = Date.now() + wait;
    const look = () => {
      const t = find(name);
      if (!t) {
        if (Date.now() < until) setTimeout(look, 150);
        else done(false);
        return;
      }
      t.scrollIntoView({ block: 'center', inline: 'nearest', behavior: reduced() ? 'auto' : 'smooth' });
      clear?.(t);
      const off = ring(t, 'point');
      const stop = () => {
        clearTimeout(timer);
        removeEventListener('keydown', esc);
        removeEventListener('pointerdown', stop);
        off();
        if (pointing === stop) pointing = null;
      };
      const esc = (e) => { if (e.key === 'Escape') stop(); };
      const timer = setTimeout(stop, hold);
      addEventListener('keydown', esc);
      addEventListener('pointerdown', stop);
      pointing = stop;
      done(true);
    };
    look();
  });
}

// Each page's tour: the control, and what we say of it. `key` is the
// assistant's shortcut, as this device has it.
const TOURS = [
  {
    at: (l) => l.pathname === '/_dd/home',
    steps: [
      ['services', () => 'Everything you can use is here. Open one to start.'],
      ['search', () => 'Search Commonty and its docs from any page.'],
      ['menu', () => 'Your name: Settings, and signing out, are in here.'],
      ['assistant', (key) => `Ask me about Commonty, or where something is. ${key} opens me from any page.`],
    ],
  },
  {
    at: (l) => l.pathname === '/_dd/settings',
    steps: [
      ['tab-profile', () => 'Your account, and how you sign in.'],
      ['tab-devices', () => 'Every browser and app signed in as you, and adding a new device.'],
      ['tab-storage', () => 'How much of your space each service uses.'],
      ['tab-network', () => 'Your home’s boxes and its Wi-Fi.'],
      ['tab-this-device', () => 'Things for this device only, like me.'],
      ['tab-leave', () => 'Take your things with you, or leave.'],
    ],
  },
  {
    at: (l) => l.pathname === '/_dd/files',
    steps: [
      ['upload', () => 'Add files from this device.'],
      ['new-folder', () => 'Make a folder to keep things together.'],
      ['trash', () => 'What you delete waits here a while before it goes for good.'],
      ['assistant', (key) => `Ask me if you get stuck. ${key} opens me from any page.`],
    ],
  },
  {
    at: (l) => l.hostname.startsWith('git.') && l.pathname === '/',
    steps: [
      ['new-repo', () => 'Start a new repository.'],
      ['search', () => 'Search Commonty and its docs from any page.'],
      ['menu', () => 'Your name: Settings, and signing out, are in here.'],
    ],
  },
];
const ANY = [
  ['menu', () => 'Your name: Settings, and signing out, are in here.'],
  ['search', () => 'Search Commonty and its docs from any page.'],
  ['assistant', (key) => `Ask me about Commonty, or where something is. ${key} opens me from any page.`],
];

/// the steps this page's tour has that show now, and whether the page has a tour of its own
export function stepsHere(key) {
  const own = TOURS.find((t) => t.at(location));
  const steps = (own?.steps || ANY).map(([name, say]) => ({ name, say: say(key) })).filter((s) => find(s.name));
  return { steps, own: !!own };
}

let touring = null;
export const inTour = () => !!touring;

/// The tour: a card by each control in turn, with Back, Next and Skip.
/// Keyboard: the card holds focus, the arrows step, Escape skips. Focus goes
/// back where it was after.
export function tour(key, onEnd) {
  touring?.end();
  const { steps } = stepsHere(key);
  if (!steps.length) return false;
  const was = document.activeElement;
  let at = 0;
  let off = null;
  const count = el('p', { class: 'count', id: 'dd-tour-n' });
  const say = el('p', { id: 'dd-tour-say' });
  const back = el('button', { type: 'button', class: 'quiet', text: 'Back' });
  const next = el('button', { type: 'button' });
  const skip = el('button', { type: 'button', class: 'quiet skip', text: 'Skip' });
  const card = el('div', { class: 'dd-tour', role: 'dialog', 'aria-modal': 'true', 'aria-labelledby': 'dd-tour-n', 'aria-describedby': 'dd-tour-say' },
    count, say, el('div', { class: 'row' }, skip, el('span', { class: 'grow' }), back, next));
  document.body.append(card);

  function show() {
    const s = steps[at];
    const t = find(s.name);
    if (!t) {
      steps.splice(at, 1);
      at = Math.min(at, steps.length - 1);
      if (steps.length) show();
      else end();
      return;
    }
    off?.();
    t.scrollIntoView({ block: 'center', inline: 'nearest', behavior: 'auto' });
    off = ring(t, 'step');
    count.textContent = `Step ${at + 1} of ${steps.length}`;
    say.textContent = s.say;
    back.disabled = at === 0;
    next.textContent = at === steps.length - 1 ? 'Done' : 'Next';
    // by the control: under it if there is room, else over it, inside the window
    const b = t.getBoundingClientRect();
    const w = Math.min(320, innerWidth - 32);
    card.style.width = `${w}px`;
    card.style.left = `${Math.max(16, Math.min(innerWidth - w - 16, b.left))}px`;
    const h = card.offsetHeight;
    const below = b.bottom + 14;
    card.style.top = `${below + h < innerHeight - 16 ? below : Math.max(16, b.top - h - 14)}px`;
    next.focus();
  }
  function end() {
    off?.();
    card.remove();
    removeEventListener('resize', show);
    touring = null;
    if (was?.isConnected && was !== document.body) was.focus();
    onEnd?.();
  }
  back.addEventListener('click', () => { if (at > 0) { at--; show(); } });
  next.addEventListener('click', () => { if (at < steps.length - 1) { at++; show(); } else end(); });
  skip.addEventListener('click', end);
  card.addEventListener('keydown', (e) => {
    if (e.key === 'Escape') { e.preventDefault(); end(); }
    if (e.key === 'ArrowRight') { e.preventDefault(); next.click(); }
    if (e.key === 'ArrowLeft') { e.preventDefault(); back.click(); }
    // Tab goes round the card's buttons while it is up
    if (e.key === 'Tab') {
      const keys = [skip, back, next].filter((b) => !b.disabled);
      const i = keys.indexOf(document.activeElement);
      e.preventDefault();
      keys[(i + (e.shiftKey ? -1 : 1) + keys.length) % keys.length].focus();
    }
  });
  addEventListener('resize', show);
  touring = { end };
  show();
  return true;
}
