// The front page: the way in is on home.<this name>, and the services drift by.

const parts = location.hostname.split('.');
const base = parts.length > 2 ? parts.slice(1).join('.') : location.hostname;
for (const a of document.querySelectorAll('a[data-home]')) a.href = `https://home.${base}${a.dataset.home}`;

// a slow, endless drift; a second copy of the cards follows the first so the loop has no seam
const track = document.getElementById('track');
const first = [...track.children];
for (const c of first) {
  const copy = c.cloneNode(true);
  copy.setAttribute('aria-hidden', 'true');
  copy.querySelector('a').tabIndex = -1;
  track.append(copy);
}
const width = () => track.scrollWidth / 2;
const step = () => first[1].offsetLeft - first[0].offsetLeft;
const still = matchMedia('(prefers-reduced-motion: reduce)').matches;
let paused = false;
let nudge = 0;
let last = performance.now();
for (const e of ['mouseenter', 'focusin', 'touchstart']) track.addEventListener(e, () => { paused = true; }, { passive: true });
for (const e of ['mouseleave', 'focusout', 'touchend']) track.addEventListener(e, () => { paused = false; }, { passive: true });
// a pause that stays, for anyone the motion bothers (WCAG 2.2.2)
let held = still;
const hold = document.getElementById('hold');
const showHold = () => {
  hold.setAttribute('aria-pressed', String(held));
  hold.setAttribute('aria-label', held ? 'Play the services' : 'Pause the services');
};
showHold();
hold.addEventListener('click', () => { held = !held; showHold(); });
document.getElementById('prev').addEventListener('click', () => { nudge -= step(); });
document.getElementById('next').addEventListener('click', () => { nudge += step(); });
function frame(now) {
  const dt = Math.min(64, now - last);
  last = now;
  let x = track.scrollLeft;
  if (!paused && !held && !document.hidden) x += dt * 0.03;
  if (nudge) {
    const d = nudge * Math.min(1, dt / 160);
    x += d;
    nudge -= d;
    if (Math.abs(nudge) < 0.5) nudge = 0;
  }
  const w = width();
  if (x >= w) x -= w;
  if (x < 0) x += w;
  track.scrollLeft = x;
  requestAnimationFrame(frame);
}
requestAnimationFrame(frame);
