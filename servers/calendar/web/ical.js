// iCalendar and vCard as the pages read and write them: one event or one
// person per file, as every calendar and contacts program keeps them. A
// file is parsed into its components and properties, and what a page
// changes is set on those, so anything another program wrote that the page
// does not know is written back as it was. Times are worked out in the
// event's own time zone with the browser's own zone data (Intl), so a
// weekly meeting stays at 9:00 across a change of the clocks.

/// A file's components: [{ name, props: [{ name, params, value }], comps }].
export function parse(text) {
  const lines = String(text).replace(/\r?\n[ \t]/g, '').split(/\r?\n/);
  const top = { name: '', props: [], comps: [] };
  const stack = [top];
  for (const line of lines) {
    if (!line.trim()) continue;
    const p = contentLine(line);
    if (!p) continue;
    const here = stack[stack.length - 1];
    if (p.name === 'BEGIN') {
      const c = { name: p.value.toUpperCase(), props: [], comps: [] };
      here.comps.push(c);
      stack.push(c);
    } else if (p.name === 'END') {
      if (stack.length > 1) stack.pop();
    } else {
      here.props.push(p);
    }
  }
  return top.comps;
}

function contentLine(line) {
  // the name and its parameters end at the first colon outside quotes
  let i = 0, quoted = false;
  for (; i < line.length; i++) {
    const c = line[i];
    if (c === '"') quoted = !quoted;
    else if (c === ':' && !quoted) break;
  }
  if (i >= line.length) return null;
  const head = line.slice(0, i), value = line.slice(i + 1);
  const parts = [];
  let cur = '';
  quoted = false;
  for (const c of head) {
    if (c === '"') { quoted = !quoted; cur += c; }
    else if (c === ';' && !quoted) { parts.push(cur); cur = ''; }
    else cur += c;
  }
  parts.push(cur);
  // a group ("item1.TEL") is kept on the name as it was
  const name = parts.shift().toUpperCase();
  const params = {};
  for (const p of parts) {
    const eq = p.indexOf('=');
    if (eq < 0) { params[p.toUpperCase()] = ''; continue; }
    params[p.slice(0, eq).toUpperCase()] = p.slice(eq + 1).replace(/^"(.*)"$/, '$1');
  }
  return { name, params, value };
}

/// A component back as text, lines folded at 75 bytes, CRLF.
export function serialize(comp) {
  const out = [];
  const walk = (c) => {
    out.push(`BEGIN:${c.name}`);
    for (const p of c.props) {
      let head = p.name;
      for (const [k, v] of Object.entries(p.params || {})) {
        head += `;${k}=${/[;:,]/.test(v) ? `"${v}"` : v}`;
      }
      out.push(fold(`${head}:${p.value}`));
    }
    for (const k of c.comps) walk(k);
    out.push(`END:${c.name}`);
  };
  walk(comp);
  return out.join('\r\n') + '\r\n';
}

const enc = new TextEncoder();
function fold(line) {
  if (enc.encode(line).length <= 75) return line;
  const parts = [];
  let cur = '', n = 0, limit = 75;
  for (const ch of line) {
    const b = enc.encode(ch).length;
    if (n + b > limit) { parts.push(cur); cur = ''; n = 0; limit = 74; }
    cur += ch; n += b;
  }
  parts.push(cur);
  return parts.join('\r\n ');
}

export const prop = (c, name) => c.props.find((p) => p.name === name || p.name.endsWith('.' + name));
export const props = (c, name) => c.props.filter((p) => p.name === name || p.name.endsWith('.' + name));

/// a property's value as text: \n, \, \; and \\ turned back
export function text(p) {
  return p ? p.value.replace(/\\([nN,;\\])/g, (_, c) => (c === 'n' || c === 'N' ? '\n' : c)) : '';
}
export function escapeText(s) {
  return String(s).replace(/\\/g, '\\\\').replace(/\n/g, '\\n').replace(/,/g, '\\,').replace(/;/g, '\\;');
}

/// One property set to `value` (replacing every one of that name), or
/// taken out when value is empty.
export function setProp(c, name, value, params = {}) {
  const at = c.props.findIndex((p) => p.name === name);
  c.props = c.props.filter((p) => p.name !== name);
  if (value === '' || value === null || value === undefined) return;
  const p = { name, params, value };
  if (at < 0) c.props.push(p); else c.props.splice(at, 0, p);
}

/// A DATE or DATE-TIME property: { y, m, d } for a day, with h, mi, s and
/// tz ('UTC', a zone, or null for floating time) for a moment.
export function parseTime(p) {
  if (!p) return null;
  const v = p.value.trim();
  const m = v.match(/^(\d{4})(\d{2})(\d{2})(?:T(\d{2})(\d{2})(\d{2})?(Z)?)?$/);
  if (!m) return null;
  const t = { y: +m[1], m: +m[2], d: +m[3] };
  if (m[4] === undefined || p.params.VALUE === 'DATE') return { ...t, date: true };
  return { ...t, h: +m[4], mi: +m[5], s: +(m[6] || 0), tz: m[7] ? 'UTC' : zoneName(p.params.TZID) };
}

// Outlook writes Windows' names for zones; the common ones, as IANA's
const WINDOWS = {
  'Eastern Standard Time': 'America/New_York', 'Central Standard Time': 'America/Chicago',
  'Mountain Standard Time': 'America/Denver', 'US Mountain Standard Time': 'America/Phoenix',
  'Pacific Standard Time': 'America/Los_Angeles', 'Alaskan Standard Time': 'America/Anchorage',
  'Hawaiian Standard Time': 'Pacific/Honolulu', 'Atlantic Standard Time': 'America/Halifax',
  'GMT Standard Time': 'Europe/London', 'Greenwich Standard Time': 'Atlantic/Reykjavik',
  'W. Europe Standard Time': 'Europe/Berlin', 'Romance Standard Time': 'Europe/Paris',
  'Central Europe Standard Time': 'Europe/Budapest', 'Central European Standard Time': 'Europe/Warsaw',
  'E. Europe Standard Time': 'Europe/Chisinau', 'FLE Standard Time': 'Europe/Kiev',
  'GTB Standard Time': 'Europe/Bucharest', 'Russian Standard Time': 'Europe/Moscow',
  'China Standard Time': 'Asia/Shanghai', 'Tokyo Standard Time': 'Asia/Tokyo',
  'Korea Standard Time': 'Asia/Seoul', 'Singapore Standard Time': 'Asia/Singapore',
  'India Standard Time': 'Asia/Kolkata', 'Taipei Standard Time': 'Asia/Taipei',
  'AUS Eastern Standard Time': 'Australia/Sydney', 'E. Australia Standard Time': 'Australia/Brisbane',
  'New Zealand Standard Time': 'Pacific/Auckland', 'UTC': 'UTC', 'Coordinated Universal Time': 'UTC',
};

const zoneOk = new Map();
function known(tz) {
  if (!zoneOk.has(tz)) {
    try { new Intl.DateTimeFormat('en-US', { timeZone: tz }); zoneOk.set(tz, true); }
    catch { zoneOk.set(tz, false); }
  }
  return zoneOk.get(tz);
}

/// a TZID as a zone the browser knows, or null (then it is floating time,
/// the viewer's own clock)
export function zoneName(tzid) {
  if (!tzid) return null;
  if (WINDOWS[tzid]) return WINDOWS[tzid];
  if (known(tzid)) return tzid;
  // "/mozilla.org/20050126_1/America/New_York" and the like
  const m = tzid.match(/([A-Z][A-Za-z_]+\/[A-Za-z_]+(?:\/[A-Za-z_]+)?)$/);
  return m && known(m[1]) ? m[1] : null;
}

export const here = () => Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';

const fmts = new Map();
function offset(ms, tz) {
  if (tz === 'UTC') return 0;
  if (!fmts.has(tz)) {
    fmts.set(tz, new Intl.DateTimeFormat('en-US', {
      timeZone: tz, hourCycle: 'h23', year: 'numeric', month: 'numeric', day: 'numeric',
      hour: 'numeric', minute: 'numeric', second: 'numeric',
    }));
  }
  const p = {};
  for (const x of fmts.get(tz).formatToParts(new Date(ms))) p[x.type] = +x.value;
  return Date.UTC(p.year, p.month - 1, p.day, p.hour % 24, p.minute, p.second) - Math.floor(ms / 1000) * 1000;
}

/// a wall clock in a zone, as a moment
export function zoned(y, m, d, h, mi, s, tz) {
  const wall = Date.UTC(y, m - 1, d, h, mi, s);
  if (tz === 'UTC') return wall;
  let t = wall - offset(wall, tz);
  // once more, for a moment near a change of the clocks
  t = wall - offset(t, tz);
  return t;
}

/// a moment as the wall clock in a zone: { y, m, d, h, mi, s, wd }
export function wall(ms, tz = here()) {
  const x = new Date(ms + offset(ms, tz));
  return { y: x.getUTCFullYear(), m: x.getUTCMonth() + 1, d: x.getUTCDate(), h: x.getUTCHours(), mi: x.getUTCMinutes(), s: x.getUTCSeconds(), wd: x.getUTCDay() };
}

/// a parsed time as a moment; a day is its midnight on the viewer's clock
export function instant(t, viewer = here()) {
  if (!t) return NaN;
  if (t.date) return zoned(t.y, t.m, t.d, 0, 0, 0, viewer);
  return zoned(t.y, t.m, t.d, t.h, t.mi, t.s, t.tz || viewer);
}

const pad = (n, w = 2) => String(n).padStart(w, '0');
export const dateValue = (y, m, d) => `${pad(y, 4)}${pad(m)}${pad(d)}`;
export const timeValue = (y, m, d, h, mi, s = 0) => `${dateValue(y, m, d)}T${pad(h)}${pad(mi)}${pad(s)}`;
export function utcValue(ms) {
  const x = new Date(ms);
  return `${timeValue(x.getUTCFullYear(), x.getUTCMonth() + 1, x.getUTCDate(), x.getUTCHours(), x.getUTCMinutes(), x.getUTCSeconds())}Z`;
}

/// "P1D", "PT1H30M", "-PT15M" as milliseconds
export function duration(v) {
  const m = String(v || '').match(/^([+-])?P(?:(\d+)W)?(?:(\d+)D)?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?)?$/);
  if (!m) return null;
  const ms = (((+(m[2] || 0) * 7 + +(m[3] || 0)) * 24 + +(m[4] || 0)) * 60 + +(m[5] || 0)) * 60000 + +(m[6] || 0) * 1000;
  return m[1] === '-' ? -ms : ms;
}

export function durationValue(ms) {
  const neg = ms < 0;
  let s = Math.round(Math.abs(ms) / 1000);
  const d = Math.floor(s / 86400); s -= d * 86400;
  const h = Math.floor(s / 3600); s -= h * 3600;
  const mi = Math.floor(s / 60); s -= mi * 60;
  let out = `${neg ? '-' : ''}P${d ? `${d}D` : ''}`;
  if (h || mi || s || !d) out += `T${h ? `${h}H` : ''}${mi ? `${mi}M` : ''}${s || (!h && !mi) ? `${s}S` : ''}`;
  return out;
}

/// Each VEVENT (and VTODO with a due or start) of a calendar file, as a
/// page uses it. `comp` is the component itself, for writing back.
export function events(text) {
  const out = [];
  for (const cal of parse(text)) {
    if (cal.name !== 'VCALENDAR') continue;
    for (const c of cal.comps) {
      if (c.name !== 'VEVENT') continue;
      const ev = event(c);
      if (ev) out.push(ev);
    }
  }
  return out;
}

function event(c) {
  const start = parseTime(prop(c, 'DTSTART'));
  if (!start) return null;
  let end = parseTime(prop(c, 'DTEND'));
  const dur = duration(prop(c, 'DURATION')?.value);
  const allDay = !!start.date;
  const rrule = prop(c, 'RRULE')?.value || '';
  const times = (name) => props(c, name).flatMap((p) => p.value.split(',').map((v) => parseTime({ ...p, value: v }))).filter(Boolean);
  const alarms = c.comps.filter((a) => a.name === 'VALARM').map((a) => {
    const t = prop(a, 'TRIGGER');
    return { before: t && t.params.VALUE !== 'DATE-TIME' ? -(duration(t.value) || 0) : null, at: t && t.params.VALUE === 'DATE-TIME' ? parseTime(t) : null };
  });
  return {
    comp: c,
    uid: text(prop(c, 'UID')),
    summary: text(prop(c, 'SUMMARY')),
    location: text(prop(c, 'LOCATION')),
    description: text(prop(c, 'DESCRIPTION')),
    status: (prop(c, 'STATUS')?.value || '').toUpperCase(),
    start, end, dur, allDay, rrule,
    exdates: times('EXDATE'),
    rdates: times('RDATE'),
    recurrenceId: parseTime(prop(c, 'RECURRENCE-ID')),
    alarms,
  };
}

/// how long an event lasts, in ms; a day for a day's event with no end
export function length(ev, viewer = here()) {
  if (ev.end) return Math.max(0, instant(ev.end, viewer) - instant(ev.start, viewer));
  if (ev.dur !== null && ev.dur !== undefined) return ev.dur;
  return ev.allDay ? 86400000 : 0;
}

const DAYS = ['SU', 'MO', 'TU', 'WE', 'TH', 'FR', 'SA'];

/// an RRULE value as { freq, interval, count, until, byday: [{ n, wd }], ... }
export function rule(v) {
  const r = { freq: '', interval: 1, count: 0, until: null, byday: [], bymonthday: [], bymonth: [], bysetpos: [], wkst: 1 };
  for (const part of String(v).split(';')) {
    const [k, val = ''] = part.split('=');
    const list = val.split(',').filter(Boolean);
    switch (k.toUpperCase()) {
      case 'FREQ': r.freq = val.toUpperCase(); break;
      case 'INTERVAL': r.interval = Math.max(1, +val || 1); break;
      case 'COUNT': r.count = +val || 0; break;
      case 'UNTIL': r.until = parseTime({ value: val, params: {} }); break;
      case 'BYDAY': r.byday = list.map((x) => { const m = x.match(/^([+-]?\d+)?([A-Z]{2})$/i); return m ? { n: m[1] ? +m[1] : 0, wd: DAYS.indexOf(m[2].toUpperCase()) } : null; }).filter((x) => x && x.wd >= 0); break;
      case 'BYMONTHDAY': r.bymonthday = list.map(Number).filter(Boolean); break;
      case 'BYMONTH': r.bymonth = list.map(Number).filter(Boolean); break;
      case 'BYSETPOS': r.bysetpos = list.map(Number).filter(Boolean); break;
      case 'WKST': r.wkst = Math.max(0, DAYS.indexOf(val.toUpperCase())); break;
      default: break;
    }
  }
  return r;
}

// day arithmetic on plain dates, as days since 1970-01-01
const dayNo = (y, m, d) => Math.floor(Date.UTC(y, m - 1, d) / 86400000);
const ymd = (n) => { const x = new Date(n * 86400000); return { y: x.getUTCFullYear(), m: x.getUTCMonth() + 1, d: x.getUTCDate(), wd: x.getUTCDay() }; };
const monthLen = (y, m) => new Date(Date.UTC(y, m, 0)).getUTCDate();

// the days of one month a rule picks, in order
function inMonth(r, y, m, start) {
  const len = monthLen(y, m);
  let days = [];
  if (r.bymonthday.length) {
    for (const md of r.bymonthday) {
      const d = md > 0 ? md : len + md + 1;
      if (d >= 1 && d <= len) days.push(d);
    }
  }
  if (r.byday.length) {
    const fromDays = [];
    for (const { n, wd } of r.byday) {
      const all = [];
      for (let d = 1; d <= len; d++) if (ymd(dayNo(y, m, d)).wd === wd) all.push(d);
      if (n === 0) fromDays.push(...all);
      else { const d = n > 0 ? all[n - 1] : all[all.length + n]; if (d) fromDays.push(d); }
    }
    days = r.bymonthday.length ? days.filter((d) => fromDays.includes(d)) : fromDays;
  }
  if (!r.bymonthday.length && !r.byday.length) {
    if (start.d <= len) days.push(start.d);
  }
  days = [...new Set(days)].sort((a, b) => a - b);
  return setpos(r, days);
}

function setpos(r, list) {
  if (!r.bysetpos.length) return list;
  return r.bysetpos.map((p) => (p > 0 ? list[p - 1] : list[list.length + p])).filter((x) => x !== undefined).sort((a, b) => a - b);
}

/// The days (as day numbers) a rule makes, from the event's first day on,
/// in order, one period at a time; `stop(day)` says when to stop looking.
function* ruleDays(r, start, stop) {
  const first = dayNo(start.y, start.m, start.d);
  const step = r.interval;
  if (r.freq === 'DAILY') {
    for (let n = first; !stop(n); n += step) {
      const t = ymd(n);
      if (r.bymonth.length && !r.bymonth.includes(t.m)) continue;
      if (r.bymonthday.length && !r.bymonthday.includes(t.d) && !r.bymonthday.includes(t.d - monthLen(t.y, t.m) - 1)) continue;
      if (r.byday.length && !r.byday.some((b) => b.wd === t.wd)) continue;
      yield n;
    }
  } else if (r.freq === 'WEEKLY') {
    const wds = r.byday.length ? r.byday.map((b) => b.wd) : [ymd(first).wd];
    // the week the first day is in, from the rule's week start
    let week = first - ((ymd(first).wd - r.wkst + 7) % 7);
    for (; !stop(week); week += 7 * step) {
      const days = [];
      for (let i = 0; i < 7; i++) {
        const n = week + i;
        const t = ymd(n);
        if (!wds.includes(t.wd)) continue;
        if (r.bymonth.length && !r.bymonth.includes(t.m)) continue;
        days.push(n);
      }
      yield* setpos(r, days);
    }
  } else if (r.freq === 'MONTHLY') {
    for (let k = 0; ; k += step) {
      const y = start.y + Math.floor((start.m - 1 + k) / 12), m = ((start.m - 1 + k) % 12) + 1;
      if (stop(dayNo(y, m, 1))) return;
      if (r.bymonth.length && !r.bymonth.includes(m)) continue;
      for (const d of inMonth(r, y, m, start)) yield dayNo(y, m, d);
    }
  } else if (r.freq === 'YEARLY') {
    for (let y = start.y; !stop(dayNo(y, 1, 1)); y += step) {
      const months = r.bymonth.length ? r.bymonth : (r.byday.length && !r.bymonthday.length ? null : [start.m]);
      if (months) {
        for (const m of months) for (const d of inMonth(r, y, m, start)) yield dayNo(y, m, d);
      } else {
        // a weekday of the year itself ("the 20th Monday"), or every one
        const all = [];
        for (let n = dayNo(y, 1, 1); n < dayNo(y + 1, 1, 1); n++) {
          const wd = ymd(n).wd;
          for (const b of r.byday) if (b.wd === wd) all.push({ n, wd });
        }
        const picked = [];
        for (const b of r.byday) {
          const of = all.filter((x) => x.wd === b.wd).map((x) => x.n);
          if (b.n === 0) picked.push(...of);
          else { const x = b.n > 0 ? of[b.n - 1] : of[of.length + b.n]; if (x !== undefined) picked.push(x); }
        }
        yield* setpos(r, [...new Set(picked)].sort((a, b) => a - b));
      }
    }
  }
}

const sameTime = (a, b) => a.y === b.y && a.m === b.m && a.d === b.d && (a.date || b.date || (a.h === b.h && a.mi === b.mi));

/// The times an event happens between `from` and `to` (ms): [{ start, end,
/// original }], where `original` is the start the series gave it (what a
/// RECURRENCE-ID names). `overrides` are the series' changed ones, which
/// stand in for theirs.
export function occurrences(ev, from, to, overrides = [], viewer = here()) {
  const len = length(ev, viewer);
  const out = [];
  const put = (t) => {
    const s = instant(t, viewer);
    if (s + Math.max(len, 1) > from && s < to) out.push({ start: s, end: s + len, original: t });
  };
  if (!ev.rrule) {
    put(ev.start);
    for (const t of ev.rdates) put(t.date || ev.allDay ? { ...t, date: ev.allDay } : t);
    return out;
  }
  const r = rule(ev.rrule);
  if (!r.freq) { put(ev.start); return out; }
  const s0 = ev.start;
  const until = r.until ? instant(r.until, viewer) : Infinity;
  const begin = instant(s0, viewer);
  const skip = ev.exdates;
  let made = 0;
  const limit = r.count || Infinity;
  // a day number past which nothing can fall in the range
  const lastDay = Math.floor((Math.min(to, until) + 86400000 * 2) / 86400000);
  let guard = 0;
  for (const n of ruleDays(r, s0, (day) => day > lastDay || made >= limit || ++guard > 200000)) {
    const d = ymd(n);
    const t = s0.date ? { y: d.y, m: d.m, d: d.d, date: true } : { y: d.y, m: d.m, d: d.d, h: s0.h, mi: s0.mi, s: s0.s, tz: s0.tz };
    const at = instant(t, viewer);
    if (at < begin) continue;
    if (at > until) break;
    made++;
    if (made > limit) break;
    if (skip.some((x) => sameTime(x, t))) continue;
    if (at >= to) break;
    if (overrides.some((o) => o.recurrenceId && sameTime(o.recurrenceId, t))) continue;
    if (at + Math.max(len, 1) > from) out.push({ start: at, end: at + len, original: t });
  }
  for (const t of ev.rdates) put(t);
  return out;
}

/// A new calendar file holding one event.
export function newCalendar(uid) {
  return {
    name: 'VCALENDAR',
    props: [
      { name: 'VERSION', params: {}, value: '2.0' },
      { name: 'PRODID', params: {}, value: '-//Commonty//Calendar//EN' },
    ],
    comps: [{ name: 'VEVENT', props: [{ name: 'UID', params: {}, value: uid }], comps: [] }],
  };
}

/// Set what a page edits on an event's component: { summary, location,
/// description, allDay, start, end (ms, on `tz`'s clock), rrule, alarms
/// (minutes before) }. Everything else it had stays.
export function writeEvent(c, f, tz = here()) {
  setProp(c, 'SUMMARY', f.summary ? escapeText(f.summary) : '');
  setProp(c, 'LOCATION', f.location ? escapeText(f.location) : '');
  setProp(c, 'DESCRIPTION', f.description ? escapeText(f.description) : '');
  setProp(c, 'DURATION', '');
  if (f.allDay) {
    const s = wall(f.start, tz), e = wall(f.end, tz);
    setProp(c, 'DTSTART', dateValue(s.y, s.m, s.d), { VALUE: 'DATE' });
    setProp(c, 'DTEND', dateValue(e.y, e.m, e.d), { VALUE: 'DATE' });
  } else {
    const s = wall(f.start, tz), e = wall(f.end, tz);
    const zp = tz === 'UTC' ? {} : { TZID: tz };
    setProp(c, 'DTSTART', timeValue(s.y, s.m, s.d, s.h, s.mi) + (tz === 'UTC' ? 'Z' : ''), zp);
    setProp(c, 'DTEND', timeValue(e.y, e.m, e.d, e.h, e.mi) + (tz === 'UTC' ? 'Z' : ''), zp);
  }
  if (f.rrule !== undefined) setProp(c, 'RRULE', f.rrule);
  const now = utcValue(Date.now());
  setProp(c, 'DTSTAMP', now);
  setProp(c, 'LAST-MODIFIED', now);
  const seq = +(prop(c, 'SEQUENCE')?.value || 0);
  if (prop(c, 'CREATED')) setProp(c, 'SEQUENCE', String(seq + 1));
  else { setProp(c, 'CREATED', now); setProp(c, 'SEQUENCE', '0'); }
  if (f.alarms) {
    c.comps = c.comps.filter((a) => a.name !== 'VALARM');
    for (const min of f.alarms) {
      c.comps.push({
        name: 'VALARM',
        props: [
          { name: 'ACTION', params: {}, value: 'DISPLAY' },
          { name: 'DESCRIPTION', params: {}, value: escapeText(f.summary || 'Reminder') },
          { name: 'TRIGGER', params: {}, value: durationValue(-min * 60000) },
        ],
        comps: [],
      });
    }
  }
}

/// one more date the series skips (an occurrence deleted on its own)
export function addExdate(c, t) {
  const v = t.date ? dateValue(t.y, t.m, t.d) : timeValue(t.y, t.m, t.d, t.h, t.mi, t.s) + (t.tz === 'UTC' ? 'Z' : '');
  const params = t.date ? { VALUE: 'DATE' } : (t.tz && t.tz !== 'UTC' ? { TZID: t.tz } : {});
  c.props.push({ name: 'EXDATE', params, value: v });
}

/// a series that ends before `t` (this and following, changed or deleted)
export function endBefore(c, t, viewer = here()) {
  const r = prop(c, 'RRULE');
  if (!r) return;
  const before = instant(t, viewer) - 1000;
  const parts = r.value.split(';').filter((p) => !/^(UNTIL|COUNT)=/i.test(p));
  parts.push(`UNTIL=${utcValue(before)}`);
  r.value = parts.join(';');
}

/// a fresh id for a new event or person
export function newUid() {
  const b = crypto.getRandomValues(new Uint8Array(16));
  return [...b].map((x) => x.toString(16).padStart(2, '0')).join('').replace(/^(.{8})(.{4})(.{4})(.{4})/, '$1-$2-$3-$4-');
}

/// The VCARDs of a file: { comp, uid, fn, n, org, title, tels, emails,
/// adrs, bday, note, categories, url }
export function cards(text) {
  return parse(text).filter((c) => c.name === 'VCARD').map(card);
}

function card(c) {
  const kind = (p) => (p.params.TYPE || '').toLowerCase().split(',').filter((t) => t && t !== 'pref' && t !== 'internet' && t !== 'voice')[0] || '';
  const n = (prop(c, 'N')?.value || '').split(';').map((x) => text({ value: x }));
  const fn = text(prop(c, 'FN')) || [n[3], n[1], n[2], n[0], n[4]].filter(Boolean).join(' ').trim();
  const adr = (p) => {
    const a = p.value.split(';').map((x) => text({ value: x }));
    return { kind: kind(p), street: [a[1], a[2]].filter(Boolean).join(', '), city: a[3] || '', region: a[4] || '', code: a[5] || '', country: a[6] || '' };
  };
  const bday = prop(c, 'BDAY')?.value || '';
  return {
    comp: c,
    uid: text(prop(c, 'UID')),
    fn,
    n,
    org: text(prop(c, 'ORG')).split(';').filter(Boolean).join(', '),
    title: text(prop(c, 'TITLE')),
    tels: props(c, 'TEL').map((p) => ({ kind: kind(p), value: text(p).replace(/^tel:/i, '') })),
    emails: props(c, 'EMAIL').map((p) => ({ kind: kind(p), value: text(p).replace(/^mailto:/i, '') })),
    adrs: props(c, 'ADR').map(adr),
    bday,
    note: text(prop(c, 'NOTE')),
    categories: text(prop(c, 'CATEGORIES')).split(',').map((s) => s.trim()).filter(Boolean),
    url: text(prop(c, 'URL')),
    photo: !!prop(c, 'PHOTO'),
  };
}

/// "--1018", "19900218", "1990-02-18" as { y, m, d } (y may be null)
export function birthday(v) {
  const m = String(v || '').match(/^(?:(\d{4})|--)-?(\d{2})-?(\d{2})/);
  return m ? { y: m[1] ? +m[1] : null, m: +m[2], d: +m[3] } : null;
}

export function newCard(uid) {
  return {
    name: 'VCARD',
    props: [{ name: 'VERSION', params: {}, value: '3.0' }, { name: 'UID', params: {}, value: uid }],
    comps: [],
  };
}

/// Set what a page edits on a card: { fn, org, title, tels, emails, adrs,
/// bday, note }. Lists replace what was there; anything else stays (its
/// photo, its groups, what another program added).
export function writeCard(c, f) {
  const parts = f.fn.trim().split(/\s+/);
  const family = parts.length > 1 ? parts.pop() : '';
  setProp(c, 'FN', escapeText(f.fn.trim()));
  setProp(c, 'N', [family, parts.join(' '), '', '', ''].map(escapeText).join(';'));
  setProp(c, 'ORG', f.org ? escapeText(f.org) : '');
  setProp(c, 'TITLE', f.title ? escapeText(f.title) : '');
  const many = (name, list, value) => {
    c.props = c.props.filter((p) => p.name !== name && !p.name.endsWith('.' + name));
    for (const x of list) c.props.push({ name, params: x.kind ? { TYPE: x.kind.toUpperCase() } : {}, value: value(x) });
  };
  many('TEL', f.tels.filter((x) => x.value.trim()), (x) => escapeText(x.value.trim()));
  many('EMAIL', f.emails.filter((x) => x.value.trim()), (x) => escapeText(x.value.trim()));
  many('ADR', f.adrs.filter((a) => a.street || a.city || a.code || a.country), (a) => ['', '', a.street, a.city, a.region, a.code, a.country].map(escapeText).join(';'));
  setProp(c, 'BDAY', f.bday || '');
  setProp(c, 'NOTE', f.note ? escapeText(f.note) : '');
  setProp(c, 'REV', utcValue(Date.now()));
}
