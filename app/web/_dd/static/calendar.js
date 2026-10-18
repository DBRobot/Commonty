// Calendar: every calendar in the member's libraries, by week or month,
// and a list of days on a phone. Events are .ics files read and sealed in
// this tab (vdir.js, ical.js); a save names the version it was made from,
// so a change made on another device first is never overwritten.

import { $, el } from './kit.js';
import * as I from './ical.js';
import { collections, items, save, remove, make, first, fileName, forAccount } from './vdir.js';
import { unlock, libraries, share } from './library.js';
import { me } from './shell.js';

const DAY = 86400000;
const HOUR_PX = 48;
const PALETTE = ['#1D5C42', '#1D4470', '#7A3B69', '#8A5A12', '#B3401F', '#46557F', '#2F6FD6', '#5E6862'];
const tz = I.here();
const keptJSON = (k, d) => { try { return JSON.parse(localStorage.getItem(k)) ?? d; } catch { return d; } };
const keepJSON = (k, v) => { try { localStorage.setItem(k, JSON.stringify(v)); } catch { /* private window */ } };

let cals = [];                    // collections, own first
let libs = [];
const files = new Map();          // collection path id -> [{ name, path, etag, text }]
let hidden = new Set(keptJSON('dd-cal-hidden', []));
let birthdays = keptJSON('dd-cal-birthdays', false);
let people = [];                  // cards, for birthdays
let span = keptJSON('dd-cal-span', 'week');
let anchor = startOfDay(Date.now());
let picked = null;                // an occurrence's id
let shown = [];                   // the occurrences drawn
let ready = false;

const calId = (c) => `${c.lib.id}/${c.path}`;
const calColor = (c) => c.color || PALETTE[Math.max(0, cals.indexOf(c)) % PALETTE.length];

function startOfDay(ms) { const w = I.wall(ms, tz); return I.zoned(w.y, w.m, w.d, 0, 0, 0, tz); }
function addDays(ms, n) { const w = I.wall(ms, tz); return I.zoned(w.y, w.m, w.d + n, 0, 0, 0, tz); }
function weekStart(ms) { const w = I.wall(ms, tz); return addDays(startOfDay(ms), -((w.wd + 6) % 7)); }
function monthStart(ms) { const w = I.wall(ms, tz); return I.zoned(w.y, w.m, 1, 0, 0, 0, tz); }

const fmt = (o) => new Intl.DateTimeFormat(undefined, { timeZone: tz, ...o });
const fTime = fmt({ hour: 'numeric', minute: '2-digit' });
const fHour = fmt({ hour: 'numeric' });
const fDayName = fmt({ weekday: 'short' });
const fDayLong = fmt({ weekday: 'long', day: 'numeric', month: 'long' });
const fMonth = fmt({ month: 'long', year: 'numeric' });
const fDayMonth = fmt({ day: 'numeric', month: 'short' });
const hm = (ms) => fTime.format(ms);

function range() {
  if (span === 'month') {
    const m = monthStart(anchor);
    const from = weekStart(m);
    return { from, to: addDays(from, 42), first: m };
  }
  const from = weekStart(anchor);
  return { from, to: addDays(from, 7) };
}

/// every occurrence in [from, to) of every calendar shown
function occurrencesIn(from, to) {
  const out = [];
  for (const c of cals) {
    if (hidden.has(calId(c))) continue;
    for (const f of files.get(calId(c)) || []) {
      let evs;
      try { evs = I.events(f.text); } catch { continue; }
      const byUid = new Map();
      for (const e of evs) {
        if (!byUid.has(e.uid)) byUid.set(e.uid, { master: null, overrides: [] });
        const g = byUid.get(e.uid);
        if (e.recurrenceId) g.overrides.push(e); else g.master = e;
      }
      for (const [uid, g] of byUid) {
        const add = (ev, o, series) => out.push({
          id: `${calId(c)}/${f.name}/${uid}/${o.start}/${ev.recurrenceId ? 'x' : ''}`,
          cal: c, file: f, ev, series, start: o.start, end: o.end, original: ev.recurrenceId || o.original, allDay: ev.allDay,
        });
        if (g.master) for (const o of I.occurrences(g.master, from, to, g.overrides, tz)) add(g.master, o, g.master);
        for (const ov of g.overrides) for (const o of I.occurrences(ov, from, to, [], tz)) add(ov, o, g.master);
      }
    }
  }
  if (birthdays) {
    for (const p of people) {
      const b = I.birthday(p.bday);
      if (!b) continue;
      for (let y = I.wall(from, tz).y; y <= I.wall(to, tz).y; y++) {
        const s = I.zoned(y, b.m, b.d, 0, 0, 0, tz);
        if (s >= to || s + DAY <= from) continue;
        out.push({ id: `bday/${p.uid || p.fn}/${y}`, cal: null, person: p, start: s, end: s + DAY, allDay: true, ev: { summary: `${p.fn}'s birthday`, allDay: true }, years: b.y ? y - b.y : null });
      }
    }
  }
  return out.sort((a, b) => a.start - b.start || (b.end - b.start) - (a.end - a.start));
}

function chipColor(o) { return o.cal ? calColor(o.cal) : 'var(--off)'; }

function evButton(o, cls) {
  return el('button', {
    type: 'button', class: cls, style: `--col:${chipColor(o)}`, 'aria-pressed': String(picked === o.id),
    'aria-label': `${o.ev.summary || 'Untitled'}, ${o.allDay ? 'all day' : hm(o.start)}`,
    onclick: () => pick(picked === o.id ? null : o.id),
  });
}

function draw() {
  const r = range();
  shown = occurrencesIn(r.from, r.to);
  drawRange(r);
  drawMini();
  const grid = $('grid');
  grid.replaceChildren(span === 'month' ? monthView(r) : weekView(r), agendaView(r));
  drawDetail();
  if (span === 'week') {
    const sc = grid.querySelector('.wk-scroll');
    if (sc && !sc.dataset.scrolled) { sc.scrollTop = 7 * HOUR_PX; sc.dataset.scrolled = '1'; }
  }
}

function drawRange(r) {
  if (span === 'month') $('range').textContent = fMonth.format(r.first);
  else {
    const last = addDays(r.from, 6);
    $('range').textContent = `${fDayMonth.format(r.from)} – ${fDayMonth.format(last)} ${I.wall(last, tz).y}`;
  }
  for (const b of document.querySelectorAll('.seg button')) b.setAttribute('aria-pressed', String(b.dataset.span === span));
}

function weekView(r) {
  const days = Array.from({ length: 7 }, (_, i) => addDays(r.from, i));
  const today = startOfDay(Date.now());
  const head = el('div', { class: 'wk-head' }, el('div'),
    days.map((d) => el('div', { class: d === today ? 'is-today' : '' },
      el('div', { class: 'dname', text: fDayName.format(d) }), el('div', { class: 'dnum', text: String(I.wall(d, tz).d) }))));
  const allCols = days.map(() => el('div'));
  const timed = days.map(() => []);
  for (const o of shown) {
    const long = o.allDay || o.end - o.start >= DAY;
    days.forEach((d, i) => {
      const end = addDays(d, 1);
      if (o.start >= end || o.end <= d) return;
      if (long) {
        const b = evButton(o, 'chip');
        b.append(!o.allDay && o.start >= d ? el('span', { class: 't', text: hm(o.start) }) : '', o.ev.summary || 'Untitled');
        allCols[i].append(b);
      } else timed[i].push(o);
    });
  }
  const all = el('div', { class: 'wk-allday' }, el('div', { class: 'lab', text: 'all day' }), allCols);
  const hours = el('div', {}, Array.from({ length: 24 }, (_, h) => el('div', { class: 'hour', text: h ? fHour.format(I.zoned(2026, 1, 5, h, 0, 0, tz)) : '' })));
  const cols = days.map((d, i) => {
    const col = el('div', { class: 'daycol' + (d === today ? ' is-today' : ''), style: `height:${24 * HOUR_PX}px` });
    for (const { o, lane, lanes } of lanesOf(timed[i])) {
      const s = Math.max(o.start, d), e = Math.min(o.end, addDays(d, 1));
      const top = ((s - d) / 3600000) * HOUR_PX;
      const h = Math.max(((e - s) / 3600000) * HOUR_PX - 2, 22);
      const b = evButton(o, 'ev');
      b.style.top = `${top + 1}px`;
      b.style.height = `${h}px`;
      b.style.left = `calc(${(lane / lanes) * 100}% + 3px)`;
      b.style.right = `calc(${((lanes - lane - 1) / lanes) * 100}% + 3px)`;
      b.append(el('b', { text: o.ev.summary || 'Untitled' }), el('span', { class: 't', text: `${hm(o.start)}–${hm(o.end)}` }));
      col.append(b);
    }
    if (d === today) col.append(el('div', { class: 'now', style: `top:${((Date.now() - d) / 3600000) * HOUR_PX}px`, 'aria-hidden': 'true' }));
    return col;
  });
  return el('div', { class: 'week' }, head, all, el('div', { class: 'wk-scroll' }, el('div', { class: 'wk-grid' }, hours, cols)));
}

/// side by side where they overlap: each its lane, out of how many its
/// cluster needs
function lanesOf(list) {
  const out = [];
  let cluster = [], ends = [], clusterEnd = -Infinity;
  const flush = () => { const n = ends.length || 1; for (const x of cluster) x.lanes = n; cluster = []; ends = []; };
  for (const o of [...list].sort((a, b) => a.start - b.start)) {
    if (o.start >= clusterEnd) { flush(); clusterEnd = -Infinity; }
    let lane = ends.findIndex((e) => e <= o.start);
    if (lane < 0) { lane = ends.length; ends.push(o.end); } else ends[lane] = o.end;
    const x = { o, lane, lanes: 1 };
    cluster.push(x); out.push(x);
    clusterEnd = Math.max(clusterEnd, o.end);
  }
  flush();
  return out;
}

function monthView(r) {
  const today = startOfDay(Date.now());
  const m = I.wall(r.first, tz).m;
  const cells = [];
  for (let i = 0; i < 7; i++) cells.push(el('div', { class: 'dname', text: fDayName.format(addDays(r.from, i)) }));
  for (let i = 0; i < 42; i++) {
    const d = addDays(r.from, i), end = addDays(d, 1);
    const w = I.wall(d, tz);
    const cell = el('div', { class: 'mcell' + (w.m !== m ? ' out' : '') + (d === today ? ' is-today' : '') }, el('span', { class: 'n', text: String(w.d) }));
    const here = shown.filter((o) => o.start < end && o.end > d);
    for (const o of here.slice(0, 4)) {
      const b = evButton(o, 'chip');
      b.append(!o.allDay && o.start >= d ? el('span', { class: 't', text: hm(o.start) }) : '', o.ev.summary || 'Untitled');
      cell.append(b);
    }
    if (here.length > 4) cell.append(el('span', { class: 'more', text: `${here.length - 4} more` }));
    cells.push(cell);
  }
  return el('div', { class: 'month' }, cells);
}

function agendaView(r) {
  const today = startOfDay(Date.now());
  const n = span === 'month' ? 42 : 7;
  const out = el('div', { class: 'agenda' });
  for (let i = 0; i < n; i++) {
    const d = addDays(r.from, i), end = addDays(d, 1);
    const here = shown.filter((o) => o.start < end && o.end > d);
    if (span === 'month' && !here.length && d !== today) continue;
    const day = el('div', { class: 'day' + (d === today ? ' is-today' : '') },
      el('div', { class: 'dh' }, el('span', { text: fDayName.format(d) }), fDayLong.format(d) + (d === today ? ' · today' : '')));
    if (!here.length) day.append(el('div', { class: 'empty', text: 'Nothing planned' }));
    for (const o of here) {
      const b = evButton(o, 'item');
      b.append(el('span', { class: 't', text: o.allDay || o.end - o.start >= DAY ? 'all day' : `${hm(o.start)}–${hm(o.end)}` }), el('span', { text: o.ev.summary || 'Untitled' }));
      day.append(b);
    }
    out.append(day);
  }
  return out;
}

function drawMini() {
  const box = $('mini');
  const m0 = span === 'month' ? monthStart(anchor) : monthStart(anchor);
  const w0 = I.wall(m0, tz);
  const from = weekStart(m0);
  const r = range();
  const today = startOfDay(Date.now());
  const g = el('div', { class: 'mini-g' });
  for (let i = 0; i < 7; i++) g.append(el('span', { class: 'dow', text: fDayName.format(addDays(from, i)).slice(0, 2) }));
  for (let i = 0; i < 42; i++) {
    const d = addDays(from, i);
    const w = I.wall(d, tz);
    const cls = [w.m !== w0.m ? 'out' : '', d === today ? 'today' : '', span === 'week' && d >= r.from && d < r.to ? 'in-range' : ''].filter(Boolean).join(' ');
    g.append(el('button', { type: 'button', class: cls, 'aria-label': fDayLong.format(d), onclick: () => { anchor = d; span = 'week'; keepJSON('dd-cal-span', span); draw(); } }, String(w.d)));
  }
  box.replaceChildren(
    el('div', { class: 'mini-h' },
      el('button', { type: 'button', 'aria-label': 'Month before', onclick: () => { anchor = I.zoned(w0.y, w0.m - 1, 1, 0, 0, 0, tz); draw(); } }, '‹'),
      el('span', { text: fMonth.format(m0) }),
      el('button', { type: 'button', 'aria-label': 'Month after', onclick: () => { anchor = I.zoned(w0.y, w0.m + 1, 1, 0, 0, 0, tz); draw(); } }, '›')),
    g);
}

function drawCals() {
  const row = (c) => {
    const id = calId(c);
    const cb = el('input', { type: 'checkbox', id: `cal-${cals.indexOf(c)}` });
    cb.checked = !hidden.has(id);
    cb.addEventListener('change', () => {
      if (cb.checked) hidden.delete(id); else hidden.add(id);
      keepJSON('dd-cal-hidden', [...hidden]);
      draw();
    });
    const sharedOut = c.mine && c.readers?.length;
    return el('label', { class: 'cal-row', for: cb.id, style: `--swatch:${calColor(c)}` }, cb,
      el('span', { class: 'swatch' }), el('span', { class: 'name', text: c.name }),
      sharedOut ? el('button', { type: 'button', class: 'sharing', title: `Shared with ${c.readers.map((r) => r.name).join(', ')}`, onclick: (e) => { e.preventDefault(); openSharing(c); } }, 'Sharing') : '',
      !c.mine ? el('span', { class: 'sub', text: c.owner, title: c.writable ? `${c.owner} shares this with you` : `${c.owner} shares this with you to see` }) : '',
      !c.mine && !c.writable ? el('span', { class: 'sr-only', text: ', you can see it, not change it' }) : '');
  };
  $('cals-mine').replaceChildren(...cals.filter((c) => c.mine).map(row));
  const shared = cals.filter((c) => !c.mine);
  $('shared-box').hidden = !shared.length;
  $('cals-shared').replaceChildren(...shared.map(row));
  $('show-birthdays').checked = birthdays;
}

const find = (id) => shown.find((o) => o.id === id);

function pick(id) {
  picked = id;
  draw();
}

function describeRule(v) {
  if (!v) return '';
  const r = I.rule(v);
  const every = { DAILY: 'day', WEEKLY: 'week', MONTHLY: 'month', YEARLY: 'year' }[r.freq] || '';
  if (!every) return 'Repeats';
  let s = r.interval > 1 ? `Every ${r.interval} ${every}s` : `Every ${every}`;
  const names = ['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'];
  const nth = { 1: 'first', 2: 'second', 3: 'third', 4: 'fourth', 5: 'fifth', '-1': 'last' };
  if (r.byday.length) s += ' on ' + r.byday.map((b) => (b.n ? `the ${nth[b.n] || b.n} ` : '') + names[b.wd]).join(', ');
  if (r.count) s += `, ${r.count} times`;
  if (r.until) s += `, until ${fDayMonth.format(I.instant(r.until, tz))} ${r.until.y}`;
  return s;
}

function alarmText(ev) {
  if (!ev.alarms?.length) return '';
  return ev.alarms.map((a) => {
    if (a.at) return `At ${hm(I.instant(a.at, tz))}`;
    const m = Math.round((a.before || 0) / 60000);
    if (m <= 0) return 'At the start';
    if (m % 1440 === 0) return `${m / 1440} day${m > 1440 ? 's' : ''} before`;
    if (m % 60 === 0) return `${m / 60} hour${m > 60 ? 's' : ''} before`;
    return `${m} minutes before`;
  }).join(', ');
}

function drawDetail() {
  const box = $('detail');
  const o = picked && find(picked);
  $('cal').classList.toggle('with-detail', !!o);
  box.hidden = !o;
  if (!o) return;
  const dl = el('dl', { class: 'kv' });
  const add = (k, v) => { if (v) dl.append(el('dt', { text: k }), el('dd', {}, v)); };
  const multi = o.end - o.start > DAY;
  add('When', o.allDay
    ? (multi ? `${fDayLong.format(o.start)} – ${fDayLong.format(o.end - 1)}` : `${fDayLong.format(o.start)}, all day`)
    : `${fDayLong.format(o.start)}, ${hm(o.start)}–${hm(o.end)}`);
  if (o.person) {
    add('Turns', o.years ? String(o.years) : '');
    box.replaceChildren(
      el('div', { class: 'top' }, el('h2', { text: o.ev.summary }), el('button', { class: 'x', type: 'button', 'aria-label': 'Close', onclick: () => pick(null) }, '×')),
      dl, el('div', { class: 'acts' }, el('a', { href: '/_dd/contacts', text: 'Open in Contacts' })));
    return;
  }
  add('Repeats', describeRule(o.series?.rrule || o.ev.rrule));
  add('Where', o.ev.location);
  add('Calendar', el('span', { style: 'display:inline-flex;gap:8px;align-items:center' }, el('span', { class: 'swatch', style: `--swatch:${calColor(o.cal)}` }), o.cal.name + (o.cal.mine ? '' : ` · ${o.cal.owner}`)));
  add('Reminds', alarmText(o.ev));
  add('Notes', o.ev.description);
  if (o.ev.status === 'CANCELLED') add('Status', 'Cancelled');
  box.replaceChildren(
    el('div', { class: 'top' }, el('h2', { text: o.ev.summary || 'Untitled' }), el('button', { class: 'x', type: 'button', 'aria-label': 'Close', onclick: () => pick(null) }, '×')),
    dl,
    o.cal.writable ? el('div', { class: 'acts' },
      el('button', { type: 'button', onclick: () => openEdit(o) }, 'Edit'),
      el('button', { type: 'button', class: 'danger-quiet', onclick: () => askDelete(o) }, 'Delete')) : '',
  );
}

function say(text) { $('msg').textContent = text; }

/// a file's calendar parsed afresh, and the VEVENT with this uid (and
/// RECURRENCE-ID, for a changed one)
function fileComps(f, uid, rid) {
  const cal = I.parse(f.text).find((c) => c.name === 'VCALENDAR');
  const ev = cal?.comps.find((c) => c.name === 'VEVENT' && I.text(I.prop(c, 'UID')) === uid && !!I.prop(c, 'RECURRENCE-ID') === !!rid
    && (!rid || JSON.stringify(I.parseTime(I.prop(c, 'RECURRENCE-ID'))) === JSON.stringify(rid)));
  return { cal, ev };
}

async function store(coll, name, text, etag) {
  const r = await save(coll, name, text, etag);
  if (r.changed !== undefined) {
    await refresh(coll);
    say('This changed on another device first, so your change was not saved. This is how it is now; make your change again.');
    return null;
  }
  const list = (files.get(calId(coll)) || []).filter((f) => f.name !== name);
  list.push(r.saved);
  files.set(calId(coll), list);
  return r.saved;
}

async function drop(coll, f) {
  const ok = await remove(coll, f.name, f.etag);
  if (!ok) {
    await refresh(coll);
    say('This changed on another device first, so it was not deleted. This is how it is now.');
    return false;
  }
  files.set(calId(coll), (files.get(calId(coll)) || []).filter((x) => x.name !== f.name));
  return true;
}

async function refresh(coll) {
  files.set(calId(coll), await items(coll, '.ics'));
  draw();
}

// the form's fields as times on this device's clock
function formTimes() {
  const allDay = $('f-allday').checked;
  const [sy, sm, sd] = $('f-sd').value.split('-').map(Number);
  const [ey, em, ed] = $('f-ed').value.split('-').map(Number);
  if (allDay) return { allDay, start: I.zoned(sy, sm, sd, 0, 0, 0, tz), end: I.zoned(ey, em, ed + 1, 0, 0, 0, tz) };
  const [sh, smi] = ($('f-st').value || '09:00').split(':').map(Number);
  const [eh, emi] = ($('f-et').value || '10:00').split(':').map(Number);
  return { allDay, start: I.zoned(sy, sm, sd, sh, smi, 0, tz), end: I.zoned(ey, em, ed, eh, emi, 0, tz) };
}

const dateInput = (ms) => { const w = I.wall(ms, tz); return `${w.y}-${String(w.m).padStart(2, '0')}-${String(w.d).padStart(2, '0')}`; };
const timeInput = (ms) => { const w = I.wall(ms, tz); return `${String(w.h).padStart(2, '0')}:${String(w.mi).padStart(2, '0')}`; };

let editing = null;

function openEdit(o, at) {
  editing = o || null;
  const dlg = $('edit');
  $('edit-h').textContent = o ? 'Edit event' : 'New event';
  $('edit-err').textContent = '';
  const writable = cals.filter((c) => c.writable);
  $('f-cal').replaceChildren(...writable.map((c) => el('option', { value: calId(c), text: c.mine ? c.name : `${c.name} (${c.owner})` })));
  const start = o ? o.start : (at ?? (() => { const n = Date.now() + 3600000; const w = I.wall(n, tz); return I.zoned(w.y, w.m, w.d, w.h, 0, 0, tz); })());
  const end = o ? o.end : start + 3600000;
  const allDay = o ? o.allDay : false;
  $('f-title').value = o ? o.ev.summary : '';
  $('f-allday').checked = allDay;
  dlg.classList.toggle('all-day', allDay);
  $('f-sd').value = dateInput(start);
  $('f-ed').value = dateInput(allDay ? end - DAY : end);
  $('f-st').value = timeInput(start);
  $('f-et').value = timeInput(end);
  const rr = o ? (o.series?.rrule || '') : '';
  const known = [...$('f-repeat').options].some((x) => x.value === rr);
  $('f-repeat-keep').hidden = known;
  $('f-repeat').value = known ? rr : 'keep';
  $('f-cal').value = o ? calId(o.cal) : (writable.find((c) => c.mine && !hidden.has(calId(c))) ? calId(writable.find((c) => c.mine && !hidden.has(calId(c)))) : $('f-cal').value);
  $('f-where').value = o ? o.ev.location : '';
  $('f-notes').value = o ? o.ev.description : '';
  const a = o?.ev.alarms?.[0];
  $('f-alarm').value = a && a.before !== null ? String(Math.round(a.before / 60000)) : (o ? '' : '30');
  if (![...$('f-alarm').options].some((x) => x.value === $('f-alarm').value)) $('f-alarm').value = '';
  $('f-which').hidden = !(o && o.series?.rrule);
  for (const r of document.querySelectorAll('input[name="which"]')) r.checked = r.value === 'one';
  dlg.showModal();
  $('f-title').focus();
}

$('f-allday').addEventListener('change', () => $('edit').classList.toggle('all-day', $('f-allday').checked));
$('f-sd').addEventListener('change', () => { if ($('f-ed').value < $('f-sd').value) $('f-ed').value = $('f-sd').value; });
$('edit-cancel').addEventListener('click', () => $('edit').close());

$('edit-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const t = formTimes();
  if (!(t.end > t.start)) { $('edit-err').textContent = 'The end is before the start.'; return; }
  const repeat = $('f-repeat').value;
  const alarm = $('f-alarm').value;
  const fields = {
    summary: $('f-title').value.trim() || 'Untitled', location: $('f-where').value.trim(), description: $('f-notes').value.trim(),
    allDay: t.allDay, start: t.start, end: t.end, alarms: alarm === '' ? [] : [Number(alarm)],
  };
  if (repeat !== 'keep') fields.rrule = repeat;
  const target = cals.find((c) => calId(c) === $('f-cal').value);
  const which = document.querySelector('input[name="which"]:checked')?.value || 'one';
  $('edit-ok').disabled = true;
  try {
    const done = editing ? await change(editing, fields, target, which) : await create(fields, target);
    // not done: changed elsewhere first, and the page now shows how it is
    $('edit').close();
    if (done) { say('Saved'); picked = null; draw(); }
  } catch (err) {
    $('edit-err').textContent = `Not saved: ${err.message}`;
  } finally {
    $('edit-ok').disabled = false;
  }
});

async function create(fields, coll, uid = I.newUid()) {
  const cal = I.newCalendar(uid);
  I.writeEvent(cal.comps[0], fields, tz);
  return store(coll, fileName(uid, '.ics'), I.serialize(cal), 'new');
}

async function change(o, fields, target, which) {
  const coll = o.cal;
  const f = o.file;
  const uid = o.series?.uid || o.ev.uid;
  const recurring = !!o.series?.rrule;
  // to another calendar: the whole series moves, as it is with this change
  if (target && calId(target) !== calId(coll)) {
    const { cal, ev } = fileComps(f, uid, null);
    if (!cal || !ev) return false;
    if (!recurring) I.writeEvent(ev, fields, tz);
    const moved = await store(target, f.name, I.serialize(cal), 'new');
    if (!moved) return false;
    return drop(coll, f);
  }
  if (!recurring || which === 'all') {
    const { cal, ev } = fileComps(f, uid, o.ev.recurrenceId && !recurring ? o.ev.recurrenceId : null);
    if (!cal || !ev) return false;
    if (recurring) {
      // the series moves by as much as this one did
      const first = I.instant(o.series.start, tz);
      const shift = fields.start - o.start;
      const len = fields.end - fields.start;
      I.writeEvent(ev, { ...fields, start: first + shift, end: first + shift + len }, tz);
    } else I.writeEvent(ev, fields, tz);
    return store(coll, f.name, I.serialize(cal), f.etag);
  }
  if (which === 'one') {
    const { cal } = fileComps(f, uid, null);
    if (!cal) return false;
    let ov = o.ev.recurrenceId ? fileComps(f, uid, o.ev.recurrenceId).ev : null;
    if (!ov) {
      const master = fileComps(f, uid, null).ev;
      ov = { name: 'VEVENT', props: master.props.filter((p) => !['RRULE', 'EXDATE', 'RDATE', 'DTSTART', 'DTEND', 'DURATION'].includes(p.name)).map((p) => ({ ...p })), comps: [] };
      const rid = o.original;
      ov.props.push(rid.date
        ? { name: 'RECURRENCE-ID', params: { VALUE: 'DATE' }, value: I.dateValue(rid.y, rid.m, rid.d) }
        : { name: 'RECURRENCE-ID', params: rid.tz && rid.tz !== 'UTC' ? { TZID: rid.tz } : {}, value: I.timeValue(rid.y, rid.m, rid.d, rid.h, rid.mi, rid.s) + (rid.tz === 'UTC' ? 'Z' : '') });
      cal.comps.push(ov);
    }
    const { rrule: _drop, ...one } = fields;
    I.writeEvent(ov, one, tz);
    return store(coll, f.name, I.serialize(cal), f.etag);
  }
  // this and later: the old series ends before it, a new one starts here
  const { cal, ev } = fileComps(f, uid, null);
  if (!cal || !ev) return false;
  I.endBefore(ev, o.original, tz);
  const kept = await store(coll, f.name, I.serialize(cal), f.etag);
  if (!kept) return false;
  return create({ ...fields, rrule: fields.rrule ?? o.series.rrule }, coll);
}

function ask(title, text, buttons) {
  return new Promise((ok) => {
    const dlg = $('ask');
    $('ask-h').textContent = title;
    $('ask-text').textContent = text;
    $('ask-body').replaceChildren();
    $('ask-actions').replaceChildren(...buttons.map(([label, value, cls]) =>
      el('button', { type: 'button', class: cls || '', onclick: () => { dlg.close(); ok(value); } }, label)));
    dlg.onclose = () => ok(null);
    dlg.showModal();
  });
}

async function askDelete(o) {
  const recurring = !!o.series?.rrule;
  const which = recurring
    ? await ask('Delete a repeating event', `${o.ev.summary || 'Untitled'} repeats. Which do you want to delete?`, [
      ['Cancel', null, 'quiet'], ['Every one', 'all', 'quiet'], ['This and later ones', 'after', 'quiet'], ['This one', 'one', 'danger']])
    : await ask('Delete this event?', `${o.ev.summary || 'Untitled'} goes to the trash, where it stays for 30 days.`, [['Cancel', null, 'quiet'], ['Delete', 'all', 'danger']]);
  if (!which) return;
  const f = o.file, coll = o.cal;
  const uid = o.series?.uid || o.ev.uid;
  let done;
  try {
    if (which === 'all') done = await drop(coll, f);
    else {
      const { cal, ev } = fileComps(f, uid, null);
      if (!cal || !ev) return;
      if (which === 'one') {
        I.addExdate(ev, o.original);
        cal.comps = cal.comps.filter((c) => !(c.name === 'VEVENT' && I.prop(c, 'RECURRENCE-ID') && JSON.stringify(I.parseTime(I.prop(c, 'RECURRENCE-ID'))) === JSON.stringify(o.original)));
      } else I.endBefore(ev, o.original, tz);
      done = await store(coll, f.name, I.serialize(cal), f.etag);
    }
  } catch (err) {
    say(`Not deleted: ${err.message}`);
    return;
  }
  if (done) { picked = null; say('Deleted'); draw(); }
}

$('new-cal').addEventListener('click', async () => {
  const own = libs.find((l) => !l.reader && !l.writer);
  if (!own) return;
  const dlg = $('ask');
  $('ask-h').textContent = 'New calendar';
  $('ask-text').textContent = '';
  const name = el('input', { id: 'nc-name', autocomplete: 'off', required: true });
  let color = PALETTE[cals.filter((c) => c.mine).length % PALETTE.length];
  const sw = el('div', { class: 'acts', role: 'radiogroup', 'aria-label': 'Colour' }, PALETTE.map((p) => {
    const b = el('button', { type: 'button', class: 'quiet', role: 'radio', 'aria-checked': String(p === color), 'aria-label': p, style: `width:34px;padding:0;background:${p}` });
    b.addEventListener('click', () => { color = p; for (const x of sw.children) x.setAttribute('aria-checked', String(x === b)); for (const x of sw.children) x.style.outline = x === b ? '3px solid var(--focus)' : ''; });
    if (p === color) b.style.outline = '3px solid var(--focus)';
    return b;
  }));
  $('ask-body').replaceChildren(el('label', { for: 'nc-name' }, 'Name', name), el('div', { class: 'fine', text: 'Colour' }), sw);
  $('ask-actions').replaceChildren(
    el('button', { type: 'button', class: 'quiet', onclick: () => dlg.close() }, 'Cancel'),
    el('button', { type: 'button', onclick: async () => {
      if (!name.value.trim()) { name.focus(); return; }
      dlg.close();
      try {
        const c = await make(own, 'Calendars', name.value.trim(), color);
        cals.push(c); files.set(calId(c), []);
        drawCals(); draw(); say('Made');
      } catch (err) { say(`Not made: ${err.message}`); }
    } }, 'Make'));
  dlg.onclose = null;
  dlg.showModal();
  name.focus();
});

// a calendar kept with others is a library of its own

function sharingForm(title, readers, then) {
  const dlg = $('ask');
  $('ask-h').textContent = title;
  $('ask-text').textContent = '';
  const rows = el('div', { class: 'share-rows' });
  const addRow = (r = { name: '', writes: false }) => {
    const n = rows.children.length;
    const name = el('input', { value: r.name, autocomplete: 'off', 'aria-label': 'Member', placeholder: 'Their name on Commonty' });
    const w = el('input', { type: 'checkbox', id: `sh-w-${n}` });
    w.checked = r.writes;
    const row = el('div', { class: 'share-row' }, name,
      el('label', { class: 'check', for: w.id }, w, 'Can change it'),
      el('button', { type: 'button', class: 'quiet', 'aria-label': 'Remove', onclick: () => row.remove() }, '×'));
    rows.append(row);
  };
  for (const r of readers.length ? readers : [{ name: '', writes: false }]) addRow(r);
  const err = el('p', { class: 'form-err', role: 'alert' });
  return { dlg, rows, err, addRow, read: () => {
    // a name given twice is one member, with the last box ticked or not
    const by = new Map();
    for (const r of rows.children) {
      const name = r.querySelector('input').value.trim().toLowerCase();
      if (name) by.set(name, { name, writes: r.querySelector('input[type=checkbox]').checked });
    }
    return [...by.values()];
  }, then };
}

async function afterShare(run, err, dlg) {
  try {
    const r = await run();
    if (r.app) { err.textContent = 'Sharing is done in the Commonty app’s own window, with your account’s main key.'; return; }
    dlg.close();
    say('Shared');
    await reload();
  } catch (e) {
    err.textContent = `Not shared: ${e.message}`;
  }
}

$('new-shared').addEventListener('click', () => {
  const f = sharingForm('New shared calendar', []);
  const name = el('input', { id: 'ns-name', autocomplete: 'off' });
  let color = PALETTE[(cals.length + 2) % PALETTE.length];
  $('ask-body').replaceChildren(
    el('label', { for: 'ns-name' }, 'Name', name),
    el('div', { class: 'fine', text: 'Shared with' }), f.rows,
    el('button', { type: 'button', class: 'quiet small', onclick: () => f.addRow() }, 'Add someone'), f.err);
  $('ask-actions').replaceChildren(
    el('button', { type: 'button', class: 'quiet', onclick: () => f.dlg.close() }, 'Cancel'),
    el('button', { type: 'button', onclick: () => {
      if (!name.value.trim()) { name.focus(); return; }
      const readers = f.read();
      if (!readers.length) { f.err.textContent = 'Add the member to share it with.'; return; }
      afterShare(async () => {
        const r = await share(null, readers);
        if (r.app) return r;
        const lib = (await libraries()).find((l) => l.id === r.id);
        if (lib) await make(lib, 'Calendars', name.value.trim(), color);
        return r;
      }, f.err, f.dlg);
    } }, 'Share'));
  f.dlg.onclose = null;
  f.dlg.showModal();
  name.focus();
});

function openSharing(c) {
  const f = sharingForm(`Sharing ${c.name}`, c.readers);
  $('ask-body').replaceChildren(f.rows,
    el('button', { type: 'button', class: 'quiet small', onclick: () => f.addRow() }, 'Add someone'), f.err);
  $('ask-actions').replaceChildren(
    el('button', { type: 'button', class: 'quiet', onclick: () => f.dlg.close() }, 'Cancel'),
    el('button', { type: 'button', onclick: () => afterShare(() => share(c.lib.id, f.read()), f.err, f.dlg) }, 'Save'));
  f.dlg.onclose = null;
  f.dlg.showModal();
}

async function reload() {
  const got = await collections('Calendars');
  libs = got.libs;
  cals = got.list;
  drawCals();
  await Promise.all(cals.map(async (c) => { try { files.set(calId(c), await items(c, '.ics')); } catch { /* next time */ } }));
  draw();
}

$('import').addEventListener('click', () => $('import-file').click());
$('import-file').addEventListener('change', async (e) => {
  const chosen = [...e.target.files];
  e.target.value = '';
  if (!chosen.length) return;
  const writable = cals.filter((c) => c.writable);
  const target = writable.length > 1
    ? await new Promise((ok) => {
      const dlg = $('ask');
      $('ask-h').textContent = 'Import into which calendar?';
      $('ask-text').textContent = '';
      const sel = el('select', { id: 'imp-cal', 'aria-label': 'Calendar' }, writable.map((c) => el('option', { value: calId(c), text: c.mine ? c.name : `${c.name} (${c.owner})` })));
      $('ask-body').replaceChildren(sel);
      $('ask-actions').replaceChildren(
        el('button', { type: 'button', class: 'quiet', onclick: () => { dlg.close(); } }, 'Cancel'),
        el('button', { type: 'button', onclick: () => { const c = writable.find((x) => calId(x) === sel.value); dlg.onclose = null; dlg.close(); ok(c); } }, 'Import'));
      dlg.onclose = () => ok(null);
      dlg.showModal();
    })
    : writable[0];
  if (!target) return;
  let added = 0, there = 0, failed = 0;
  for (const file of chosen) {
    const text = await file.text();
    for (const cal of I.parse(text).filter((c) => c.name === 'VCALENDAR')) {
      const zones = cal.comps.filter((c) => c.name === 'VTIMEZONE');
      const byUid = new Map();
      for (const c of cal.comps.filter((x) => x.name === 'VEVENT')) {
        let uid = I.text(I.prop(c, 'UID'));
        if (!uid) { uid = I.newUid(); c.props.push({ name: 'UID', params: {}, value: uid }); }
        if (!byUid.has(uid)) byUid.set(uid, []);
        byUid.get(uid).push(c);
      }
      const work = [...byUid];
      const one = async () => {
        while (work.length) {
          const [uid, comps] = work.shift();
          const out = { name: 'VCALENDAR', props: [{ name: 'VERSION', params: {}, value: '2.0' }, { name: 'PRODID', params: {}, value: '-//Commonty//Calendar//EN' }], comps: [...zones, ...comps] };
          try {
            const r = await save(target, fileName(uid, '.ics'), I.serialize(out), 'new');
            if (r.changed !== undefined) there++; else added++;
          } catch { failed++; }
          say(`Importing: ${added + there + failed} done…`);
        }
      };
      await Promise.all(Array.from({ length: 6 }, one));
    }
  }
  await refresh(target);
  say(`Imported ${added} event${added === 1 ? '' : 's'}` + (there ? `; ${there} were there already` : '') + (failed ? `; ${failed} could not be saved` : '') + '.');
});

$('today').addEventListener('click', () => { anchor = startOfDay(Date.now()); draw(); });
$('prev').addEventListener('click', () => { const w = I.wall(anchor, tz); anchor = span === 'month' ? I.zoned(w.y, w.m - 1, 1, 0, 0, 0, tz) : addDays(anchor, -7); draw(); });
$('next').addEventListener('click', () => { const w = I.wall(anchor, tz); anchor = span === 'month' ? I.zoned(w.y, w.m + 1, 1, 0, 0, 0, tz) : addDays(anchor, 7); draw(); });
for (const b of document.querySelectorAll('.seg button')) b.addEventListener('click', () => { span = b.dataset.span; keepJSON('dd-cal-span', span); draw(); });
$('new-event').addEventListener('click', () => openEdit(null));
$('show-birthdays').addEventListener('change', (e) => { birthdays = e.target.checked; keepJSON('dd-cal-birthdays', birthdays); if (birthdays) loadPeople(); draw(); });
document.addEventListener('keydown', (e) => {
  if (e.target.closest('input, textarea, select, dialog')) return;
  if (e.key === 'Escape' && picked) pick(null);
  else if (e.key === 't') $('today').click();
  else if (e.key === 'ArrowLeft' && e.altKey) $('prev').click();
  else if (e.key === 'ArrowRight' && e.altKey) $('next').click();
});
setInterval(() => { if (ready && !document.hidden) draw(); }, 60000);

async function loadPeople() {
  try {
    const { list } = await collections('Contacts');
    const all = await Promise.all(list.map((c) => items(c, '.vcf').catch(() => [])));
    people = all.flat().flatMap((f) => { try { return I.cards(f.text); } catch { return []; } }).filter((p) => p.bday);
    draw();
  } catch { /* birthdays wait for next time */ }
}

async function start() {
  // the frame first, from what this device kept
  $('body').hidden = false;
  $('grid').replaceChildren(el('div', { class: 'ph', 'aria-hidden': 'true' }));
  drawRange(range());
  drawMini();
  let lib;
  try {
    const [m, u] = await Promise.all([me().catch(() => null), unlock()]);
    forAccount(m?.user);
    lib = u;
  } catch (e) { $('body').hidden = true; say(`Your calendar did not open: ${e.message}`); return; }
  if (lib.app) { $('body').hidden = true; $('link').hidden = false; return; }
  if (lib.none) { $('body').hidden = true; say('You have no library yet. The app makes one when you sign in on it.'); return; }
  const drawKept = (kept) => {
    if (cals.length) return;
    cals = kept;
    drawCals();
    Promise.all(cals.map(async (c) => files.set(calId(c), await items(c, '.ics')))).then(draw);
  };
  let got;
  try { got = await collections('Calendars', drawKept); } catch (e) { say(`Your calendars did not open: ${e.message}`); return; }
  libs = got.libs;
  cals = got.list;
  if (!cals.some((c) => c.mine) && !lib.ok.demo && !lib.ok.reader) {
    try { const c = await first(libs, 'Calendars', cals, 'Personal'); if (c) cals.unshift(c); } catch { /* made next time */ }
  }
  drawCals();
  $('actions').hidden = !cals.some((c) => c.writable);
  $('new-cal').hidden = !libs.some((l) => !l.reader && !l.writer) || !!lib.ok.demo;
  $('new-shared').hidden = $('new-cal').hidden;
  await Promise.all(cals.map(async (c) => {
    const id = calId(c);
    try { files.set(id, await items(c, '.ics', (kept) => { if (!files.has(id)) { files.set(id, kept); draw(); } })); }
    catch (e) { say(`${c.name} did not open: ${e.message}`); }
  }));
  ready = true;
  draw();
  if (birthdays) loadPeople();
  if (!shown.length && !cals.some((c) => (files.get(calId(c)) || []).length)) say('Nothing here yet. Add an event with New event, or bring in a calendar with Import .ics.');
}

start();
