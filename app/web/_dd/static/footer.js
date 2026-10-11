// The footer every page ends with: shell.js imports this, and a page
// without the shell loads it on its own. Not in the app's own window,
// which is an app, not the site.

const inApp = location.protocol === 'commonty:' || location.hostname === 'commonty.localhost' || (() => {
  try { return !!sessionStorage.getItem('dd-app-origin'); } catch { return false; }
})();

// the fleet's bare name: what the app wrote into a page it serves, else
// this host less its first label, or the host itself when it is the bare name
function bareName() {
  const meta = document.querySelector('meta[name="dd-domain"]')?.content;
  if (meta) return meta;
  const parts = location.hostname.split('.');
  return parts.length > 2 ? parts.slice(1).join('.') : location.hostname;
}

function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else e.setAttribute(k, v);
  }
  e.append(...kids);
  return e;
}

const mark = '<rect x="1" y="1" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1.6"/>'
  + '<rect x="12" y="1" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1.6"/>'
  + '<rect x="1" y="12" width="9" height="9" fill="none" stroke="currentColor" stroke-width="1.6"/>'
  + '<rect x="12" y="12" width="9" height="9" fill="#9CC3EA"/>';

export function footer() {
  if (inApp || document.querySelector('footer.dd-foot')) return;
  const base = bareName();
  const site = `https://${base}`;
  const home = `https://home.${base}`;
  const column = (title, links, ...extra) => el('div', {},
    el('h2', { text: title }),
    el('ul', {}, ...links.map(([text, href]) => el('li', {}, el('a', { href, text }))), ...extra));

  const svg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  svg.setAttribute('viewBox', '0 0 22 22');
  svg.setAttribute('aria-hidden', 'true');
  svg.innerHTML = mark;

  const foot = el('footer', { class: 'dd-foot', 'aria-label': 'Commonty' },
    el('div', { class: 'in' },
      el('div', {},
        el('a', { class: 'brand', href: `${site}/` }, svg, el('span', { text: 'Commonty' })),
        el('p', { class: 'lic', text: 'Free software under the AGPL 3.0.' })),
      column('Use it', [
        ['Get the app', `${site}/download`],
        ['Demo', `${home}/_dd/demo`],
        ['Sign in', `${home}/`],
        ['Joining', `${site}/docs/guide/joining`],
      ]),
      column('Learn', [
        ['Docs', `${site}/docs`],
        ['Code', `https://code.${base}/david/commonty`],
        ['Host a box', `${site}/docs/guide/hosting`],
      ]),
      column('Policies', [
        ['Privacy', `${site}/docs/policies/privacy`],
        ['Terms of use', `${site}/docs/policies/terms`],
        ['Security', `${site}/docs/policies/security`],
      ], el('li', {}, el('span', { class: 'mail', text: `security@${base}` })))));

  // after the page's own frame (the element holding main), so a page that
  // fills the window, like Chat, is scrolled past to reach it
  const main = document.querySelector('main');
  let frame = main;
  while (frame && frame.parentElement !== document.body) frame = frame.parentElement;
  if (frame) frame.after(foot);
  else document.body.append(foot);
}
footer();
