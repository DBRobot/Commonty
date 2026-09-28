// The bar above Vaultwarden's own pages: nginx adds this script to its
// index.html (modules/vault/vaultwarden.nix), and the bar is the same one
// every Commonty page has, filled by shell.js.
const svg = 'http://www.w3.org/2000/svg';

function mark() {
  const s = document.createElementNS(svg, 'svg');
  s.setAttribute('viewBox', '0 0 22 22');
  s.setAttribute('aria-hidden', 'true');
  for (const [x, y, fill] of [[1, 1, false], [12, 1, false], [1, 12, false], [12, 12, true]]) {
    const r = document.createElementNS(svg, 'rect');
    r.setAttribute('class', fill ? 'mark-fill' : 'mark-line');
    for (const [k, v] of Object.entries({ x, y, width: 9, height: 9 })) r.setAttribute(k, v);
    if (!fill) {
      r.setAttribute('fill', 'none');
      r.setAttribute('stroke-width', '1.6');
    }
    s.append(r);
  }
  return s;
}

const header = document.createElement('header');
header.className = 'dd-bar';
const bar = document.createElement('div');
bar.className = 'bar';
const brand = document.createElement('a');
brand.className = 'brand';
brand.href = '/_dd/home';
brand.setAttribute('aria-label', 'Commonty');
const name = document.createElement('span');
name.textContent = 'Commonty';
brand.append(mark(), name);
const me = document.createElement('div');
me.className = 'me';
me.id = 'me';
bar.append(brand, me);
header.append(bar);
document.body.prepend(header);
document.documentElement.classList.add('dd-vault');

await import('/_dd/static/shell.js');
