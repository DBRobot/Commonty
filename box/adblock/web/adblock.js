// Ad blocking at home: Pi-hole on the house's box, through the gate
// (box/verify/src/adblock.rs). Totals only: Pi-hole keeps nothing about
// who looked up what.
const $ = (id) => document.getElementById(id);
const num = (n) => (typeof n === 'number' ? n.toLocaleString() : '—');
const svgNS = 'http://www.w3.org/2000/svg';

async function call(path, body, method = 'POST') {
  const r = await fetch(path, {
    method,
    headers: body ? { 'content-type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  if (r.status === 401) { location.href = '/_dd/login?rd=/_dd/adblock'; throw new Error('signed out'); }
  if (!r.ok) throw new Error((await r.text()) || `the box said ${r.status}`);
  return r.status === 204 ? null : r.json();
}

function when(ts) {
  if (!ts) return '';
  const d = new Date(ts * 1000);
  return d.toLocaleDateString(undefined, { weekday: 'long' }) + ' ' + d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' });
}

function drawState(s) {
  const paused = !s.on && s.resumesIn;
  $('state-title').textContent = s.on ? 'Blocking is on' : paused ? 'Blocking is paused' : 'Blocking is off';
  $('state-sub').textContent = s.on
    ? 'For every device on the home network.'
    : paused
      ? `Until ${new Date(Date.now() + s.resumesIn * 1000).toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' })}. It comes back on by itself.`
      : 'Ads get through on the home network until someone switches it back on.';
  $('switch').textContent = s.on ? 'Turn off' : 'Turn on';
  $('switch').classList.toggle('ab-go', !s.on);
  $('pause').hidden = !s.on;
  document.querySelector('.ab-shield').classList.toggle('off', !s.on);
}

// ten-minute buckets into hours, the last day
function hours(history) {
  const now = new Date();
  now.setMinutes(0, 0, 0);
  const first = now.getTime() / 1000 - 23 * 3600;
  const out = [...Array(24)].map((_, i) => ({ t: first + i * 3600, total: 0, blocked: 0 }));
  for (const b of history || []) {
    const i = Math.floor((b.t - first) / 3600);
    if (i >= 0 && i < 24) { out[i].total += b.total || 0; out[i].blocked += b.blocked || 0; }
  }
  return out;
}

function niceMax(v) {
  if (v <= 10) return 10;
  const p = 10 ** Math.floor(Math.log10(v));
  for (const m of [1, 2, 2.5, 5, 10]) if (m * p >= v) return m * p;
  return 10 * p;
}

function chart(history) {
  const svg = $('chart');
  svg.replaceChildren();
  const hs = hours(history);
  const W = 960, H = 220, L = 48, R = 10, T = 12, B = 28;
  const max = niceMax(Math.max(1, ...hs.map((h) => h.total)));
  const cw = (W - L - R) / 24, bw = cw * 0.62;
  const y = (v) => T + (H - T - B) * (1 - v / max);
  const add = (tag, a, text) => { const e = document.createElementNS(svgNS, tag); for (const k in a) e.setAttribute(k, a[k]); if (text !== undefined) e.textContent = text; svg.append(e); return e; };
  for (let i = 0; i <= 4; i++) {
    const v = (max / 4) * i;
    add('line', { x1: L, x2: W - R, y1: y(v), y2: y(v), class: 'ab-grid' });
    add('text', { x: L - 8, y: y(v) + 4, 'text-anchor': 'end', class: 'ab-axis' }, v >= 1000 ? `${+(v / 1000).toFixed(1)}k` : String(Math.round(v)));
  }
  hs.forEach((h, i) => {
    const x = L + i * cw + (cw - bw) / 2;
    if (h.total) add('rect', { x, y: y(h.total), width: bw, height: y(0) - y(h.total), rx: 1.5, class: 'ab-answered' });
    if (h.blocked) add('rect', { x, y: y(h.blocked), width: bw, height: y(0) - y(h.blocked), rx: 1.5, class: 'ab-blocked' });
    const hr = new Date(h.t * 1000).getHours();
    if (hr % 3 === 0) add('text', { x: x + bw / 2, y: H - 8, 'text-anchor': 'middle', class: 'ab-axis' }, `${String(hr).padStart(2, '0')}:00`);
  });
}

function row(...kids) {
  const d = document.createElement('div');
  d.append(...kids);
  return d;
}
function span(cls, text) {
  const s = document.createElement('span');
  s.className = cls;
  s.textContent = text;
  return s;
}

function drawAllowed(list) {
  const box = $('allowed');
  if (!list.length) { box.replaceChildren(row(span('ab-muted ab-small', 'Nothing let through yet.'))); return; }
  box.replaceChildren(...list.map((a) => {
    const rm = document.createElement('button');
    rm.type = 'button';
    rm.className = 'ab-btn';
    rm.textContent = 'Block again';
    rm.onclick = async () => {
      rm.disabled = true;
      try { await call(`/_dd/adblock/allow/${encodeURIComponent(a.domain)}`, null, 'DELETE'); await load(); }
      catch (e) { rm.disabled = false; $('allow-err').textContent = e.message; $('allow-err').hidden = false; }
    };
    return row(span('ab-dom', a.domain), span('ab-who', [a.by, a.added ? when(a.added) : ''].filter(Boolean).join(' · ')), rm);
  }));
}

function drawLists(s) {
  $('lists').replaceChildren(...(s.lists.length ? s.lists : [{ name: 'No list loaded yet', count: null }]).map((l) =>
    row(span('ab-dom ab-plain', l.name || l.address || 'List'), span('ab-n', l.count == null ? '' : `${num(l.count)} sites`))));
  $('lists-updated').textContent = s.listsUpdated ? `updated ${when(s.listsUpdated)}` : '';
}

async function load() {
  try {
    const s = await call('/_dd/adblock/state', null, 'GET');
    $('health').textContent = 'answering';
    $('health').className = 'ab-pill';
    drawState(s);
    const t = s.today || {};
    $('pct').textContent = typeof t.percent === 'number' ? `${t.percent.toFixed(1)}%` : '—';
    $('blocked').textContent = num(t.blocked);
    $('of').textContent = typeof t.total === 'number' ? `blocked, of ${num(t.total)} today` : 'blocked today';
    $('sites').textContent = num(s.sites);
    $('lan').textContent = s.lan || '—';
    chart(s.history);
    drawAllowed(s.allowed || []);
    drawLists(s);
  } catch (e) {
    $('health').textContent = 'not answering';
    $('health').className = 'ab-pill bad';
    $('state-title').textContent = 'The box is not answering';
    $('state-sub').textContent = e.message;
  }
}

$('switch').onclick = async () => {
  const turningOn = $('switch').textContent === 'Turn on';
  $('switch').disabled = true;
  try { drawState(await call('/_dd/adblock/switch', { on: turningOn })); } finally { $('switch').disabled = false; }
};
for (const b of document.querySelectorAll('#pause button')) {
  b.onclick = async () => drawState(await call('/_dd/adblock/switch', { pause_minutes: Number(b.dataset.min) }));
}
$('allow-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const v = $('allow-input').value.trim();
  if (!v) return;
  $('allow-err').hidden = true;
  try { await call('/_dd/adblock/allow', { domain: v }); $('allow-input').value = ''; await load(); }
  catch (err) { $('allow-err').textContent = err.message; $('allow-err').hidden = false; }
});
for (const b of document.querySelectorAll('[data-copy-from]')) {
  b.onclick = async () => {
    const text = $(b.dataset.copyFrom).textContent;
    try { await navigator.clipboard.writeText(text); b.textContent = 'Copied'; }
    catch { getSelection().selectAllChildren($(b.dataset.copyFrom)); }
    setTimeout(() => { b.textContent = 'Copy'; }, 1500);
  };
}

load();
setInterval(load, 60000);
