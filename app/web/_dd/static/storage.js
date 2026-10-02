// Storage, a tab of Settings: one allowance across every service, as the
// storage ledger last counted it (box/verify/src/storage.rs). One bar,
// split by service.
const $ = (id) => document.getElementById(id);
const services = [
  ['photos', 'Photos', '#d9822b'],
  ['libraries', 'Files & Movies', '#2f6fd6'],
  ['code', 'Code', '#4a5a8a'],
  ['passwords', 'Passwords', '#1b8aa0'],
];

function size(b) {
  if (b >= 1e12) return `${(b / 1e12).toFixed(2)} TB`;
  if (b >= 1e9) return `${(b / 1e9).toFixed(1).replace(/\.0$/, '')} GB`;
  if (b >= 1e6) return `${Math.round(b / 1e6)} MB`;
  if (b > 0) return `${Math.max(1, Math.round(b / 1e3))} KB`;
  return '0';
}

function ago(t) {
  const m = Math.round((Date.now() / 1000 - t) / 60);
  return m < 1 ? 'Updated just now' : m === 1 ? 'Updated a minute ago' : `Updated ${m} minutes ago`;
}

async function load() {
  const r = await fetch('/_dd/storage/mine');
  if (r.status === 401) { location.href = '/_dd/login?rd=/_dd/settings%23storage'; return; }
  const d = await r.json();
  if (!d.counted) { $('st-state').textContent = 'Your storage has not been counted yet. It is counted every 10 minutes.'; return; }
  $('st-state').hidden = true;
  $('st-updated').textContent = ago(d.updated);
  $('st-budget').textContent = size(d.budget);
  const pct = d.budget ? Math.round((d.used / d.budget) * 100) : 0;
  $('st-used').textContent = size(d.used);
  $('st-of').textContent = `of ${size(d.budget)} used · ${size(Math.max(0, d.budget - d.used))} left`;
  $('st-pct').textContent = `${pct}%`;
  $('st-pct').classList.toggle('warn', pct >= 90);
  $('st-figure').hidden = false;
  const bar = $('st-bar');
  bar.replaceChildren();
  bar.setAttribute('aria-label', `${size(d.used)} of ${size(d.budget)} used`);
  const legend = $('st-legend');
  legend.replaceChildren();
  for (const [key, name, colour] of services) {
    const b = d.parts[key] || 0;
    if (b > 0 && d.budget) {
      const seg = document.createElement('i');
      seg.style.width = `${Math.min(100, (b / d.budget) * 100)}%`;
      seg.style.background = colour;
      seg.title = `${name} ${size(b)}`;
      bar.append(seg);
    }
    const li = document.createElement('li');
    const dot = document.createElement('span');
    dot.className = 'st-dot';
    dot.style.background = colour;
    const label = document.createElement('span');
    label.textContent = name;
    const num = document.createElement('span');
    num.className = 'st-num';
    num.textContent = size(b);
    li.append(dot, label, num);
    legend.append(li);
  }
  bar.hidden = false;
}
let started = false;

/// the Storage tab, the first time it is shown
export function start() {
  if (started) return;
  started = true;
  load();
}
