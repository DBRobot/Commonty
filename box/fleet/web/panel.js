// What the four pages behind the menu have in common: a place to put
// rows, times a person can read, and one way of saying nothing is here.

const $ = (id) => document.getElementById(id);

export function ready() {
  $('msg').hidden = true;
  $('body').hidden = false;
  return $('body');
}

export function failed(e) {
  $('msg').hidden = false;
  $('msg').textContent = String(e.message || e);
}

/// a heading and a list of rows; each row is [name, ...cells]
export function section(into, title, rows, empty) {
  const s = document.createElement('section');
  s.className = 'shelf';
  const h = document.createElement('h2');
  h.textContent = title;
  s.append(h);
  if (!rows.length) {
    const p = document.createElement('p');
    p.className = 'note';
    p.textContent = empty;
    s.append(p);
  } else {
    const ul = document.createElement('ul');
    ul.className = 'rows';
    for (const cells of rows) {
      const li = document.createElement('li');
      cells.forEach((c, i) => {
        const span = document.createElement('span');
        span.className = i === 0 ? 'rowname' : 'tag';
        span.textContent = c;
        li.append(span);
      });
      ul.append(li);
    }
    s.append(ul);
  }
  into.append(s);
  return s;
}

/// how long ago, in the words a person would use
export function ago(secs) {
  if (!secs) return 'never';
  const d = Math.max(0, Date.now() / 1000 - secs);
  if (d < 90) return 'just now';
  if (d < 5400) return `${Math.round(d / 60)} minutes ago`;
  if (d < 172800) return `${Math.round(d / 3600)} hours ago`;
  return `${Math.round(d / 86400)} days ago`;
}

export function when(secs) {
  return secs ? new Date(secs * 1000).toLocaleString() : '—';
}

export function human(n) {
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1024 && i < u.length - 1) { n /= 1024; i++; }
  return `${n < 10 && i ? n.toFixed(1) : Math.round(n)} ${u[i]}`;
}
