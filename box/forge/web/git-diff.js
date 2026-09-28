// A diff, as git writes it, turned into files and drawn unified or side by
// side. Commits, pull requests and comparisons all use it.

import { el, ic } from './git-core.js';

export function parse(text) {
  const files = [];
  let f = null, h = null;
  for (const line of text.split('\n')) {
    if (line.startsWith('diff --git ')) {
      const m = line.match(/^diff --git a\/(.*) b\/(.*)$/);
      f = { from: m?.[1] || '', to: m?.[2] || '', hunks: [], adds: 0, dels: 0, kind: 'changed', binary: false };
      files.push(f);
      h = null;
      continue;
    }
    if (!f) continue;
    if (!h) {
      if (line.startsWith('new file mode')) f.kind = 'added';
      else if (line.startsWith('deleted file mode')) f.kind = 'deleted';
      else if (line.startsWith('rename from ')) { f.kind = 'renamed'; f.from = line.slice(12); }
      else if (line.startsWith('rename to ')) f.to = line.slice(10);
      else if (line.startsWith('Binary files') || line.startsWith('GIT binary patch')) f.binary = true;
      else if (line.startsWith('--- ') || line.startsWith('+++ ') || line.startsWith('index ') || line.startsWith('similarity')) { /* header */ }
    }
    const hm = line.match(/^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@(.*)$/);
    if (hm) {
      h = { head: line, old: +hm[1], neu: +hm[2], lines: [] };
      f.hunks.push(h);
      continue;
    }
    if (!h) continue;
    const c = line[0];
    if (c === '+') { h.lines.push(['+', line.slice(1)]); f.adds++; }
    else if (c === '-') { h.lines.push(['-', line.slice(1)]); f.dels++; }
    else if (c === ' ') h.lines.push([' ', line.slice(1)]);
    else if (c === '\\') { /* no newline at end of file */ }
  }
  return files;
}

const td = (cls, t) => el('td', { class: cls, text: t ?? '' });

// With `talk`, a pull request's line comments sit under their lines and a
// line's number opens a new one: talk.notes(path, side, line) gives the
// comments there, talk.write(path, side, line, tr) opens the form after tr.
function unified(file, talk) {
  const rows = [];
  const path = file.to || file.from;
  const num = (side, line) => {
    const cell = td('ln', line);
    if (talk?.write) {
      cell.classList.add('talk');
      cell.title = 'Comment on this line';
      cell.onclick = () => talk.write(path, side, line, cell.parentElement);
    }
    return cell;
  };
  const after = (side, line) => (talk ? talk.notes(path, side, line) : []);
  for (const h of file.hunks) {
    rows.push(el('tr', { class: 'hunk' }, td('ln'), td('ln'), td('', h.head)));
    let o = h.old, n = h.neu;
    for (const [c, t] of h.lines) {
      if (c === '+') { rows.push(el('tr', { class: 'add' }, td('ln'), num('new', n), td('', `+${t}`)), ...after('new', n)); n++; }
      else if (c === '-') { rows.push(el('tr', { class: 'del' }, num('old', o), td('ln'), td('', `-${t}`)), ...after('old', o)); o++; }
      else { rows.push(el('tr', {}, td('ln', o), num('new', n), td('', ` ${t}`)), ...after('new', n)); o++; n++; }
    }
  }
  return el('div', { class: 'code' }, el('table', {}, el('tbody', {}, ...rows)));
}

function split(file) {
  const rows = [];
  for (const h of file.hunks) {
    rows.push(el('tr', { class: 'hunk' }, td('ln'), el('td', { colspan: 4, text: h.head })));
    let o = h.old, n = h.neu, i = 0;
    const L = h.lines;
    while (i < L.length) {
      if (L[i][0] === ' ') {
        rows.push(el('tr', {}, td('ln', o++), td('', L[i][1]), el('td', { class: 'gap' }), td('ln', n++), td('', L[i][1])));
        i++;
        continue;
      }
      const dels = [], adds = [];
      while (i < L.length && L[i][0] === '-') dels.push(L[i++][1]);
      while (i < L.length && L[i][0] === '+') adds.push(L[i++][1]);
      for (let k = 0; k < Math.max(dels.length, adds.length); k++) {
        const d = k < dels.length, a = k < adds.length;
        const tr = el('tr', {},
          el('td', { class: `ln${d ? ' dl' : ''}`, text: d ? o++ : '' }), el('td', { class: d ? 'dl' : '', text: d ? dels[k] : '' }),
          el('td', { class: 'gap' }),
          el('td', { class: `ln${a ? ' ad' : ''}`, text: a ? n++ : '' }), el('td', { class: a ? 'ad' : '', text: a ? adds[k] : '' }));
        rows.push(tr);
      }
    }
  }
  return el('div', { class: 'code split' }, el('table', {}, el('tbody', {}, ...rows)));
}

function bar5(adds, dels) {
  const total = adds + dels || 1;
  const a = Math.round((adds / total) * 5);
  return el('span', { class: 'bar5', 'aria-hidden': 'true' }, ...[0, 1, 2, 3, 4].map((i) => el('i', { class: i < a ? 'a' : adds + dels ? 'd' : '' })));
}

// every file of a diff; big ones start folded
export function render(files, { mode = 'unified', viewed, talk } = {}) {
  return files.map((f, idx) => {
    const name = f.kind === 'renamed' ? `${f.from} → ${f.to}` : f.kind === 'deleted' ? f.from : f.to;
    const lines = f.hunks.reduce((t, h) => t + h.lines.length, 0);
    const toggle = el('button', { class: 'btn plain', style: 'padding:2px 6px', 'aria-label': 'Fold' }, ic('down'));
    const box = el('div', { class: `box dfile${lines > 400 ? ' folded' : ''}`, id: `diff-${idx}` },
      el('header', {}, toggle,
        el('span', { class: 'mono', text: name }),
        f.kind !== 'changed' ? el('span', { class: `chip${f.kind === 'added' ? ' ok' : f.kind === 'deleted' ? ' bad' : ''}`, text: f.kind }) : null,
        el('span', { class: 'spacer' }),
        el('span', { class: 'small' }, el('span', { class: 'adds', text: `+${f.adds}` }), ' ', el('span', { class: 'dels', text: `−${f.dels}` })),
        bar5(f.adds, f.dels)),
      f.binary ? el('div', { class: 'binary', text: 'A binary file: nothing to show line by line.' })
        : !f.hunks.length ? el('div', { class: 'binary', text: f.kind === 'renamed' ? 'Moved, not changed.' : 'No lines changed.' })
          : lines > 400 ? el('div', { class: 'binary' }, `${lines.toLocaleString()} lines changed. `, el('button', { class: 'btn plain', text: 'Show them', onclick: (e) => { box.classList.remove('folded'); e.target.parentElement.remove(); } }))
            : null,
      f.hunks.length ? (mode === 'split' ? split(f) : unified(f, talk)) : null);
    if (viewed) box.querySelector('header').append(viewed(f, box));
    toggle.onclick = () => box.classList.toggle('folded');
    return box;
  });
}

export function summary(files) {
  const adds = files.reduce((t, f) => t + f.adds, 0), dels = files.reduce((t, f) => t + f.dels, 0);
  return el('span', {}, el('b', { text: `${files.length} file${files.length === 1 ? '' : 's'} changed` }), ' ', el('span', { class: 'adds', text: `+${adds.toLocaleString()}` }), ' ', el('span', { class: 'dels', text: `−${dels.toLocaleString()}` }));
}

// Unified or split, remembered on this device
export function modeSwitch(redraw) {
  let mode = 'unified';
  try { mode = localStorage.getItem('dd-git-diff') || 'unified'; } catch { /* private window */ }
  const b = (m, t) => el('button', { class: 'btn', 'aria-pressed': String(mode === m), text: t, onclick: () => { mode = m; try { localStorage.setItem('dd-git-diff', m); } catch { /* ok */ } seg.querySelectorAll('button').forEach((x) => x.setAttribute('aria-pressed', String(x.textContent === t))); redraw(m); } });
  const seg = el('span', { class: 'gseg' }, b('unified', 'Unified'), b('split', 'Split'));
  return { seg, mode: () => mode };
}
