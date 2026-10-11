// The year's squares and the week's line, drawn as SVG from counts by day.

import { el } from './git-core.js';

const NS = 'http://www.w3.org/2000/svg';
const svg = (w, h, inner, label) => {
  const s = document.createElementNS(NS, 'svg');
  s.setAttribute('width', w);
  s.setAttribute('height', h);
  s.setAttribute('viewBox', `0 0 ${w} ${h}`);
  s.setAttribute('role', 'img');
  s.setAttribute('aria-label', label);
  s.innerHTML = inner;
  return s;
};
const esc = (t) => String(t).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
export const dayKey = (d) => {
  const x = new Date(d);
  return `${x.getFullYear()}-${String(x.getMonth() + 1).padStart(2, '0')}-${String(x.getDate()).padStart(2, '0')}`;
};

// One square a day for the last year, a column a week, as GitHub has it.
// `days` is { 'YYYY-MM-DD': count }.
export function heatmap(days) {
  const end = new Date();
  end.setHours(12, 0, 0, 0);
  const start = new Date(end);
  start.setDate(start.getDate() - 364 - end.getDay());
  const cell = 11, gap = 3, left = 30, top = 18;
  const max = Math.max(1, ...Object.values(days));
  const lvl = (n) => (n === 0 ? 0 : Math.min(4, 1 + Math.floor((n / max) * 3.999)));
  let out = '', w = 0, month = -1;
  for (const d = new Date(start); d <= end; d.setDate(d.getDate() + 1)) {
    const n = days[dayKey(d)] || 0, dow = d.getDay();
    if (dow === 0 && d > start) w++;
    if (dow === 0 && d.getMonth() !== month && d.getDate() <= 7) {
      month = d.getMonth();
      out += `<text x="${left + w * (cell + gap)}" y="11" font-size="10" fill="var(--ink-3)">${d.toLocaleString(undefined, { month: 'short' })}</text>`;
    }
    const tip = `${n || 'No'} contribution${n === 1 ? '' : 's'} on ${d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric', month: 'short', year: 'numeric' })}`;
    out += `<rect x="${left + w * (cell + gap)}" y="${top + dow * (cell + gap)}" width="${cell}" height="${cell}" rx="2" fill="var(--heat-${lvl(n)})"><title>${esc(tip)}</title></rect>`;
  }
  for (const [i, t] of [[1, 'Mon'], [3, 'Wed'], [5, 'Fri']]) out += `<text x="0" y="${top + i * (cell + gap) + 9}" font-size="10" fill="var(--ink-3)">${t}</text>`;
  const W = left + (w + 1) * (cell + gap), H = top + 7 * (cell + gap);
  return el('div', { class: 'heat' },
    svg(W, H, out, 'Contributions by day over the last year'),
    el('div', { class: 'legend' }, 'Less', ...[0, 1, 2, 3, 4].map((i) => el('i', { style: `background:var(--heat-${i})` })), 'More'));
}

// Lines over days: `series` is [{ name, color, dashed, values }] and
// `labels` one per day.
export function lineChart(labels, series) {
  const W = 1200, H = 240, L = 36, R = 16, T = 26, B = 26;
  const top = Math.max(4, ...series.flatMap((s) => s.values));
  const step = Math.pow(10, Math.floor(Math.log10(top))) * (top / Math.pow(10, Math.floor(Math.log10(top))) > 5 ? 2 : 1);
  const max = Math.ceil(top / step) * step;
  const x = (i) => L + (labels.length === 1 ? 0 : (i * (W - L - R)) / (labels.length - 1));
  const y = (v) => T + (H - T - B) * (1 - v / max);
  let g = '';
  for (let v = 0; v <= max; v += step) g += `<line x1="${L}" x2="${W - R}" y1="${y(v)}" y2="${y(v)}" stroke="var(--line-2)"/><text x="${L - 8}" y="${y(v) + 4}" font-size="11" text-anchor="end" fill="var(--ink-3)">${v}</text>`;
  const every = Math.ceil(labels.length / 10);
  labels.forEach((d, i) => { if (i % every === 0 || i === labels.length - 1) g += `<text x="${x(i)}" y="${H - 6}" font-size="11" text-anchor="middle" fill="var(--ink-3)">${esc(d)}</text>`; });
  series.forEach((s, k) => {
    const path = s.values.map((v, i) => `${i ? 'L' : 'M'}${x(i).toFixed(1)},${y(v).toFixed(1)}`).join(' ');
    if (k === 0) g += `<path d="${path} L${x(s.values.length - 1)},${y(0)} L${x(0)},${y(0)} Z" fill="${s.color}" opacity=".10"/>`;
    g += `<path d="${path}" fill="none" stroke="${s.color}" stroke-width="${k === 0 ? 2.2 : 2}"${s.dashed ? ' stroke-dasharray="5 4"' : ''}/>`;
    s.values.forEach((v, i) => { g += `<circle cx="${x(i)}" cy="${y(v)}" r="${i === s.values.length - 1 ? 4 : 2.6}" fill="${s.color}"><title>${esc(labels[i])}: ${v} ${esc(s.name)}</title></circle>`; });
    g += `<line x1="${L + 6 + k * 130}" x2="${L + 24 + k * 130}" y1="10" y2="10" stroke="${s.color}" stroke-width="2"${s.dashed ? ' stroke-dasharray="5 4"' : ''}/><text x="${L + 30 + k * 130}" y="14" font-size="11.5" fill="var(--ink-2)">${esc(s.name)}</text>`;
  });
  const out = svg(W, H, g, series.map((s) => `${s.name} by day`).join(', '));
  out.removeAttribute('width');
  out.removeAttribute('height');
  return el('div', { class: 'line' }, out);
}

// the top ten languages by bytes, then the rest as one
export function languages(byLang) {
  const COLORS = ['#1D5C42', '#1D4470', '#C69214', '#B3401F', '#4F7F9E', '#7A8B3A', '#8A5A12', '#9CC3EA', '#2F9A8A', '#8C6BA8'];
  const all = Object.entries(byLang).sort((a, b) => b[1] - a[1]);
  const sum = all.reduce((t, [, n]) => t + n, 0) || 1;
  const top = all.slice(0, 10).map(([n, b], i) => [n, (b / sum) * 100, COLORS[i]]);
  const rest = all.slice(10).reduce((t, [, b]) => t + b, 0);
  if (rest) top.push(['Other', (rest / sum) * 100, 'var(--line)']);
  const pct = (p) => (p >= 10 ? p.toFixed(0) : p.toFixed(1));
  return [
    el('div', { class: 'langbar' }, ...top.map(([n, p, c]) => el('span', { title: `${n} ${pct(p)}%`, style: `flex:${p};background:${c}` }))),
    el('ul', { class: 'langs' }, ...top.map(([n, p, c]) => el('li', {}, el('span', { class: 'dot', style: `background:${c}` }), el('b', { text: n }), el('span', { class: 'p', text: `${pct(p)}%` })))),
  ];
}
