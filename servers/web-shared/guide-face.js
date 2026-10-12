// The face the assistant's button makes now and then (guide.js): its eyes
// over the chat mark as a mouth, a look picked at random, sleepy at night
// and a hat on its days, all by this device's clock. Now and then, in its
// corner, it looks up and follows the pointer a few seconds. Drawn from
// parts, never from markup; nothing here reads the page.

const NS = 'http://www.w3.org/2000/svg';
export const LOOKS = ['look', 'wink', 'surprised', 'happy'];
// a moment after 20 to 60 minutes of being used, three a day at most
const MIN = 60e3;
const TICK = 5e3;
const A_DAY = 3;
const KEY = 'dd-guide-eyes';

/// the hat for a day, if it has one
export function hatFor(now) {
  const m = now.getMonth() + 1;
  const d = now.getDate();
  if ((m === 12 && d === 31) || (m === 1 && d === 1)) return 'party';
  if (m === 12 && d >= 18 && d <= 26) return 'santa';
  if (m === 10 && d >= 24) return 'witch';
  return null;
}

/// the face for a moment at `now`: between eleven at night and six it is sleepy
export function faceFor(now, rand = Math.random) {
  const h = now.getHours();
  const look = h >= 23 || h < 6 ? 'sleepy' : LOOKS[Math.min(LOOKS.length - 1, Math.floor(rand() * LOOKS.length))];
  return { look, hat: hatFor(now) };
}

function part(tag, attrs) {
  const e = document.createElementNS(NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  return e;
}

const HATS = {
  party: () => [
    part('path', { d: 'M8 26 17 2l9 24z', fill: '#E2557B' }),
    part('path', { d: 'm11.5 17 9.5-4M10 21.5l13-5.5M14 11l5.5-2.4', stroke: '#FFD45C', 'stroke-width': '2', 'stroke-linecap': 'round' }),
    part('circle', { cx: '17', cy: '2.5', r: '2.5', fill: '#FFD45C' }),
  ],
  witch: () => [
    part('path', { d: 'M10 22 18 1l7 21z', fill: '#2B2140' }),
    part('path', { d: 'M11 19h13v3H11z', fill: '#8E6BD6' }),
    part('ellipse', { cx: '17', cy: '23.5', rx: '16', ry: '3', fill: '#2B2140' }),
  ],
  santa: () => [
    part('path', { d: 'M7 22C9 10 16 3 26 6c-4 3-5 9-4 16z', fill: '#D3313A' }),
    part('circle', { cx: '27', cy: '7', r: '3.2', fill: '#FFFFFF' }),
    part('rect', { x: '5', y: '20', width: '22', height: '5.5', rx: '2.75', fill: '#FFFFFF' }),
  ],
};

/// put a face on the button: its look, a hat, the z of a sleepy one
export function dress(btn, face) {
  undress(btn);
  btn.dataset.face = face.look;
  if (face.hat && HATS[face.hat]) {
    const s = part('svg', { viewBox: '0 0 34 27', class: `hat hat-${face.hat}`, 'aria-hidden': 'true' });
    s.append(...HATS[face.hat]());
    btn.append(s);
  }
  if (face.look === 'sleepy') {
    const z = document.createElement('span');
    z.className = 'zz';
    z.setAttribute('aria-hidden', 'true');
    z.textContent = 'z';
    btn.append(z);
  }
}

export function undress(btn) {
  delete btn.dataset.face;
  for (const e of btn.querySelectorAll('.hat, .zz')) e.remove();
}

const local = {
  get: (k) => { try { return localStorage.getItem(k); } catch { return null; } },
  set: (k, v) => { try { localStorage.setItem(k, v); } catch { /* refused: this page only */ } },
};
const gap = () => (20 + Math.random() * 40) * MIN;

// how long it has been used since the last moment, across pages, and how
// many moments today
function kept() {
  const today = new Date().toDateString();
  try {
    const k = JSON.parse(local.get(KEY) || 'null');
    if (k && typeof k.active === 'number' && typeof k.next === 'number') {
      return { day: today, count: k.day === today ? k.count || 0 : 0, active: k.active, next: k.next };
    }
  } catch { /* none kept */ }
  return { day: today, count: 0, active: 0, next: gap() };
}

/// The eyes moments: `can()` says whether one may play now (the member is
/// there, the button in its corner, nothing open). Returns a way to end one
/// early and to stop for good.
export function moments(btn, can) {
  const body = document.body;
  let playing = null;
  function play() {
    dress(btn, faceFor(new Date()));
    body.classList.add('dd-eyes');
    // the pupils go toward the pointer, at most 3px
    const follow = (e) => {
      const r = btn.getBoundingClientRect();
      const dx = e.clientX - (r.left + r.width / 2);
      const dy = e.clientY - (r.top + r.height / 2 - 14);
      const d = Math.hypot(dx, dy) || 1;
      const k = 3 * Math.min(1, d / 120);
      btn.style.setProperty('--px', `${((dx / d) * k).toFixed(2)}px`);
      btn.style.setProperty('--py', `${((dy / d) * k).toFixed(2)}px`);
    };
    addEventListener('pointermove', follow, { passive: true });
    playing = { follow, timer: setTimeout(end, 4000 + Math.random() * 2000) };
  }
  function end() {
    if (!playing) return;
    removeEventListener('pointermove', playing.follow);
    clearTimeout(playing.timer);
    playing = null;
    body.classList.remove('dd-eyes');
    btn.style.removeProperty('--px');
    btn.style.removeProperty('--py');
    // back to the chat button first, then the hat goes
    setTimeout(() => { if (!playing && !body.classList.contains('dd-peek')) undress(btn); }, 400);
  }
  const tick = setInterval(() => {
    if (playing || !can()) return;
    const k = kept();
    k.active += TICK;
    if (k.active >= k.next && k.count < A_DAY) {
      local.set(KEY, JSON.stringify({ day: k.day, count: k.count + 1, active: 0, next: gap() }));
      play();
      return;
    }
    local.set(KEY, JSON.stringify(k));
  }, TICK);
  return {
    end,
    get playing() { return !!playing; },
    stop() { clearInterval(tick); end(); },
  };
}
