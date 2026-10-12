// The uptime page: whether each service works now, and each of the last 90
// days, from /_dd/status.json (servers/gate/src/status.rs).

import { $, el, load } from './kit.js';

const DAY = 86400;
const narrow = matchMedia('(max-width: 640px)');

const time = (t) => new Date(t * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
const sameDay = (t) => new Date(t * 1000).toDateString() === new Date().toDateString();
// a moment, in the viewer's own time: today's by its time alone
const when = (t) => sameDay(t)
  ? time(t)
  : new Date(t * 1000).toLocaleString([], { day: 'numeric', month: 'short', hour: '2-digit', minute: '2-digit' });
// a bar's day is a UTC day, named as such
const dayName = (t) => new Date(t * 1000).toLocaleDateString([], { timeZone: 'UTC', day: 'numeric', month: 'short', year: 'numeric' });

function minutes(m) {
  if (m < 1) return 'under a minute';
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60);
  return m % 60 ? `${h} h ${m % 60} min` : `${h} h`;
}

// one day's bar: its colour and what it says
function day(share, start, i, checked) {
  const at = start + i * DAY;
  const name = dayName(at);
  if (share === null || share === undefined) return { state: 'off', text: `${name}: not measured` };
  if (share >= 1) return { state: 'ok', text: `${name}: working all day` };
  // today is a day so far
  const long = Math.min(DAY, Math.max(0, checked - at)) / 60;
  const down = minutes(Math.round((1 - share) * long));
  return { state: share >= 0.9 ? 'attention' : 'broken', text: `${name}: down about ${down}` };
}

function share(days) {
  const seen = days.filter((d) => d !== null && d !== undefined);
  if (!seen.length) return null;
  return seen.reduce((a, b) => a + b, 0) / seen.length;
}

function percent(x) {
  const p = x * 100;
  return `${p >= 99.995 ? '100' : p.toFixed(p >= 99 ? 2 : 1)}%`;
}

function row(s, start, checked) {
  const days = s.days || [];
  const shown = narrow.matches ? days.slice(-30) : days;
  const offset = days.length - shown.length;
  const first = shown.findIndex((d) => d !== null && d !== undefined);
  const up = share(shown);
  const span = shown.length === 90 ? '90 days' : `${shown.length} days`;
  const over = first > 0 ? `since ${dayName(start + (offset + first) * DAY)}` : `over ${span}`;
  const bars = el('ol', { class: 'bars', 'aria-label': `${s.label}, each of the last ${span}` },
    shown.map((d, i) => {
      const b = day(d, start, offset + i, checked);
      return el('li', { 'data-state': b.state, title: b.text }, el('span', { class: 'sr-only', text: b.text }));
    }));
  const word = s.up
    ? el('span', { class: 'state-word', 'data-state': 'ok', text: 'Working' })
    : el('span', { class: 'state-word', 'data-state': 'broken', text: s.since ? `Down since ${when(s.since)}` : 'Down' });
  return el('li', { class: 'svc' },
    el('div', { class: 'svc-head' }, el('h3', { text: s.label }), word),
    bars,
    el('div', { class: 'svc-foot' },
      el('span', { text: `${span} ago` }),
      el('span', { class: 'pct', text: up === null ? '—' : `${percent(up)} up ${over}` }),
      el('span', { text: 'Today' })));
}

function overall(services) {
  const down = services.filter((s) => !s.up);
  if (!services.length) return { state: 'off', text: 'Nothing is measured yet.' };
  if (!down.length) return { state: 'ok', text: 'Everything is working' };
  if (down.length === 1) {
    const s = down[0];
    return { state: 'broken', text: s.since ? `${s.label} is down since ${when(s.since)}` : `${s.label} is down` };
  }
  const names = down.map((s) => s.label);
  const list = `${names.slice(0, -1).join(', ')} and ${names[names.length - 1]}`;
  return { state: 'broken', text: `${list} are down` };
}

let last = null;

function draw(st) {
  last = st;
  const o = overall(st.services || []);
  const line = $('overall');
  line.removeAttribute('aria-busy');
  line.dataset.state = o.state;
  line.replaceChildren(el('span', { class: 'dot', 'aria-hidden': 'true' }), el('span', { text: o.text }));
  $('checked').textContent = st.checked ? `Checked at ${when(st.checked)}` : '—';
  const list = $('services');
  list.removeAttribute('aria-busy');
  list.replaceChildren(...(st.services || []).map((s) => row(s, st.start, st.checked)));
}

function failed() {
  if (last) {
    $('checked').textContent = `Could not check just now. This is from ${when(last.checked)}.`;
    return;
  }
  const line = $('overall');
  line.removeAttribute('aria-busy');
  line.dataset.state = 'attention';
  line.replaceChildren(el('span', { class: 'dot', 'aria-hidden': 'true' }), el('span', { text: 'We can\u2019t check the services right now. Try again in a few minutes.' }));
  $('checked').textContent = '—';
  const list = $('services');
  list.removeAttribute('aria-busy');
  list.replaceChildren();
}

const refresh = () => load('/_dd/status.json', draw).catch(failed);
refresh();
// a page left open stays current
setInterval(() => { if (!document.hidden) refresh(); }, 60000);
narrow.addEventListener('change', () => { if (last) draw(last); });
