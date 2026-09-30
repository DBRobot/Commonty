// The demo's Photos: its library laid out the way the photo app lays one
// out - albums along the top, then the pictures by day - read through the
// gate (box/verify/src/demo_photos.rs), which opens them on the box. Tap a
// picture to see it whole; arrows and swipes step through.
const $ = (id) => document.getElementById(id);
let albums = [];
let all = [];
let shown = [];
let at = -1;

const thumb = (id) => `/_dd/photos/demo/thumb/${id}`;
const dayOf = (t) => {
  const d = new Date(t * 1000);
  const sameYear = d.getFullYear() === new Date().getFullYear();
  return d.toLocaleDateString(undefined, sameYear
    ? { weekday: 'short', month: 'short', day: 'numeric' }
    : { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' });
};

function show(i) {
  if (i < 0 || i >= shown.length) return;
  at = i;
  const p = shown[i];
  const big = $('big');
  // the thumbnail at once, the whole picture when it arrives
  big.src = thumb(p.id);
  if (p.still) {
    const whole = new Image();
    whole.onload = () => { if (at === i) big.src = whole.src; };
    whole.src = `/_dd/photos/demo/photo/${p.id}`;
  }
  big.alt = p.title || '';
  $('cap').textContent = [p.title, dayOf(p.taken)].filter(Boolean).join(' · ');
  $('prev').hidden = i === 0;
  $('next').hidden = i === shown.length - 1;
  if (!$('view').open) $('view').showModal();
}

function draw(album) {
  shown = album ? all.filter((p) => p.albums.includes(album.id)) : all;
  $('title').textContent = album ? album.name : 'All';
  $('count').textContent = `${shown.length} ${shown.length === 1 ? 'memory' : 'memories'}`;
  for (const b of $('strip').children) b.setAttribute('aria-selected', String(b.dataset.album === String(album ? album.id : '')));
  const days = $('days');
  days.replaceChildren();
  let grid = null;
  let last = '';
  shown.forEach((p, i) => {
    const d = dayOf(p.taken);
    if (d !== last) {
      last = d;
      const h = document.createElement('h3');
      h.className = 'dp-day';
      h.textContent = d;
      grid = document.createElement('ul');
      grid.className = 'dp-grid';
      days.append(h, grid);
    }
    const li = document.createElement('li');
    const b = document.createElement('button');
    b.setAttribute('aria-label', p.title || 'Photo');
    const img = document.createElement('img');
    img.loading = 'lazy';
    img.decoding = 'async';
    img.alt = '';
    img.src = thumb(p.id);
    b.append(img);
    b.onclick = () => show(i);
    li.append(b);
    grid.append(li);
  });
}

function tile(name, cover, key, album) {
  const b = document.createElement('button');
  b.setAttribute('role', 'tab');
  b.dataset.album = key;
  if (cover) b.style.backgroundImage = `url("${thumb(cover)}")`;
  const s = document.createElement('span');
  s.textContent = name;
  b.append(s);
  b.onclick = () => draw(album);
  return b;
}

async function load() {
  const r = await fetch('/_dd/photos/demo/list');
  if (r.redirected || r.status === 401) { location.href = '/_dd/demo'; return; }
  if (!r.ok) { $('state').textContent = 'The photos could not be opened just now. Try again in a minute.'; return; }
  ({ albums, photos: all } = await r.json());
  $('state').hidden = all.length > 0;
  if (!all.length) $('state').textContent = 'No photos in the demo yet.';
  $('strip').append(tile('All', all[0] && all[0].id, '', null), ...albums.map((a) => tile(a.name, a.cover, String(a.id), a)));
  draw(null);
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
