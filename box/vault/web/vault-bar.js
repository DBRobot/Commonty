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

// On the pages about the browser extension: how to point it at this
// server. Out of the box it signs in to Bitwarden's own cloud, where no
// Commonty account exists, and says the password is wrong.
function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text) e.textContent = text;
  return e;
}
const server = location.origin;
const help = el('section', 'dd-ext-help');
help.setAttribute('aria-label', 'Setting up the browser extension for Commonty');
const url = el('code', null, server);
const copy = el('button', null, 'Copy');
copy.type = 'button';
copy.onclick = async () => {
  await navigator.clipboard.writeText(server);
  copy.textContent = 'Copied';
  setTimeout(() => { copy.textContent = 'Copy'; }, 1500);
};
const steps = el('ol');
const step = (...parts) => {
  const li = el('li');
  li.append(...parts);
  steps.append(li);
};
step('On the extension\'s login screen, open ', el('b', null, 'Logging in on'), ' and choose ', el('b', null, 'Self-hosted'), '.');
const row = el('span', 'dd-ext-url');
row.append(url, copy);
step('Set ', el('b', null, 'Server URL'), ' to ', row, ' exactly, with the https:// part, and save. Leave the other addresses empty.');
step('Enter your Commonty address (yourname@', location.hostname.replace(/^vault\./, ''), ') and choose ', el('b', null, 'Use single sign-on'), '. If it asks for an SSO identifier, type commonty.');
step('A tab signs you in through Commonty and closes itself. Unlock with your master password.');
help.append(el('h2', null, 'Using the extension with Commonty'), steps,
  el('p', 'dd-ext-note', 'The vault answers only on the Commonty network: keep this device on it (the Commonty app on a phone).'));

const ABOUT_EXTENSION = /^#\/(setup-extension|browser-extension-prompt)/;
function place() {
  const want = ABOUT_EXTENSION.test(location.hash);
  if (want && !help.isConnected) header.after(help);
  if (!want && help.isConnected) help.remove();
}
addEventListener('hashchange', place);
place();

await import('/_dd/static/shell.js');
