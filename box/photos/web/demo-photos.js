// The demo's Photos: its library as a grid, read through the gate
// (box/verify/src/demo_photos.rs), which opens the pictures on the box.
// Tap one to see it whole; arrows and swipes step through.
const $ = (id) => document.getElementById(id);
let photos = [];
let at = -1;

const day = (t) => new Date(t * 1000).toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' });

function show(i) {
  if (i < 0 || i >= photos.length) return;
  at = i;
  const p = photos[i];
  const big = $('big');
  // the thumbnail at once, the whole picture when it arrives
  big.src = `/_dd/photos/demo/thumb/${p.id}`;
  if (p.still) {
    const whole = new Image();
    whole.onload = () => { if (at === i) big.src = whole.src; };
    whole.src = `/_dd/photos/demo/photo/${p.id}`;
  }
  big.alt = p.title || '';
  $('cap').textContent = [p.title, day(p.taken)].filter(Boolean).join(' · ');
  $('prev').hidden = i === 0;
  $('next').hidden = i === photos.length - 1;
  if (!$('view').open) $('view').showModal();
}

async function load() {
  const r = await fetch('/_dd/photos/demo/list');
  if (r.redirected || r.status === 401) { location.href = '/_dd/demo'; return; }
  if (!r.ok) { $('state').textContent = 'The photos could not be opened just now. Try again in a minute.'; return; }
  photos = await r.json();
  $('state').hidden = photos.length > 0;
  if (!photos.length) $('state').textContent = 'No photos in the demo yet.';
  const grid = $('grid');
  photos.forEach((p, i) => {
    const li = document.createElement('li');
    const b = document.createElement('button');
    b.setAttribute('aria-label', p.title || 'Photo');
    const img = document.createElement('img');
    img.loading = 'lazy';
    img.decoding = 'async';
    img.alt = '';
    img.src = `/_dd/photos/demo/thumb/${p.id}`;
    b.append(img);
    b.onclick = () => show(i);
    li.append(b);
    grid.append(li);
  });
}

$('close').onclick = () => $('view').close();
$('prev').onclick = () => show(at - 1);
$('next').onclick = () => show(at + 1);
$('view').addEventListener('click', (e) => { if (e.target === $('view')) $('view').close(); });
document.addEventListener('keydown', (e) => {
  if (!$('view').open) return;
  if (e.key === 'ArrowLeft') show(at - 1);
  if (e.key === 'ArrowRight') show(at + 1);
});
let x0 = null;
$('view').addEventListener('touchstart', (e) => { x0 = e.touches[0].clientX; }, { passive: true });
$('view').addEventListener('touchend', (e) => {
  if (x0 === null) return;
  const dx = e.changedTouches[0].clientX - x0;
  if (Math.abs(dx) > 40) show(at + (dx < 0 ? 1 : -1));
  x0 = null;
});
load();
