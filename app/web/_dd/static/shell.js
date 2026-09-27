// Every signed-in page: who is looking, the bar's menu, and on the home
// page the services. One answer from /_dd/me fills them all, so the page
// itself is a plain file: the box serves it to a browser and the app
// carries its own copy, and both look the same because they are the same.

let answer = null;

/// who this is and what they may open; asked once per page
export function me() {
  answer ??= fetch('/_dd/me').then(async (r) => {
    if (r.status === 401) {
      location.href = '/_dd/login?rd=' + encodeURIComponent(location.pathname);
      throw new Error('signed out');
    }
    if (!r.ok) throw new Error(`the box said ${r.status}`);
    return r.json();
  });
  return answer;
}

/// Running inside the app rather than a browser tab: the app serves these
/// same files under its own address (app/src/site.rs), and answers the
/// library's key itself instead of a passkey.
export const inApp = location.protocol === 'commonty:' || location.hostname === 'commonty.localhost';

// the app's own screen for this device, where it signs out; ?stay keeps
// it from coming straight back here
const deviceScreen = (location.protocol === 'commonty:' ? 'tauri://localhost/' : 'http://tauri.localhost/') + '?stay';

// pages the app carries: a service that is one of them opens the app's
// copy; any other (photos, games, code) opens in the device's browser
const carried = ['/_dd/files', '/_dd/media', '/_dd/boxes', '/_dd/backups', '/_dd/devices', '/_dd/network'];
function here(url) {
  if (!inApp) return url;
  try {
    const u = new URL(url, location.href);
    if (carried.includes(u.pathname)) return u.pathname;
    if (u.origin !== location.origin) return '/_dd/app/open?url=' + encodeURIComponent(u.href);
    return url;
  } catch {
    return url;
  }
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

// the name is the control: it opens everything that is not a service
function bar(m, slot) {
  const nav = el('nav');
  m.menu.forEach((group, i) => {
    if (i) nav.append(el('hr'));
    for (const item of group) {
      // the app has no browser session to end; its own screen is where
      // this device signs out
      if (inApp && item.url === '/_dd/logout') {
        nav.append(el('a', { href: deviceScreen, text: 'This device' }));
        continue;
      }
      nav.append(el('a', { href: here(item.url), text: item.label }));
    }
  });
  const summary = el('summary', {},
    el('span', { class: 'avatar', 'aria-hidden': 'true', text: m.initial }),
    el('span', { text: m.user }),
    el('span', { class: 'chev', 'aria-hidden': 'true' }));
  const menu = el('details', { class: 'menu' }, summary, nav);
  slot.replaceChildren(menu);
  // a menu closes when you are done with it: a click anywhere else, or Escape
  document.addEventListener('click', (e) => {
    if (menu.open && !menu.contains(e.target)) menu.open = false;
  });
  document.addEventListener('keydown', (e) => {
    if (e.key === 'Escape' && menu.open) {
      menu.open = false;
      summary.focus();
    }
  });
}

// the home page: one row per service
function services(m, list) {
  if (!m.services.length) {
    list.replaceWith(el('p', { class: 'empty', text: 'Nothing runs here yet.' }));
    return;
  }
  list.replaceChildren(...m.services.map((s) => {
    const row = s.shut
      ? el('div', { class: 'service off', 'aria-disabled': 'true' })
      : el('a', { class: 'service', href: here(s.url) });
    const mark = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    mark.setAttribute('viewBox', '0 0 20 20');
    mark.setAttribute('fill', 'none');
    mark.setAttribute('stroke-width', '1.5');
    mark.setAttribute('aria-hidden', 'true');
    // the mark is the box's own svg, not anything a person wrote
    mark.innerHTML = s.icon;
    if (s.color) row.style.setProperty('--c', s.color);
    row.append(
      mark,
      el('span', {},
        el('span', { class: 'name', text: s.name }),
        el('span', { class: 'blurb', text: s.shut ? 'Not in the demo.' : s.blurb })),
      el('span', { class: 'host', text: s.host }),
      el('span', { class: 'open', text: s.shut ? 'Shut' : 'Open' }));
    return el('li', {}, row);
  }));
}

me().then((m) => {
  const slot = document.getElementById('me');
  if (slot) bar(m, slot);
  const list = document.getElementById('services');
  if (list) services(m, list);
  const banner = document.getElementById('demo');
  if (banner) banner.hidden = !m.demo;
  for (const e of document.querySelectorAll('[data-user]')) e.dataset.user = m.user;
}).catch((e) => {
  const list = document.getElementById('services');
  if (list) list.replaceWith(el('p', { class: 'empty', text: String(e.message || e) }));
});
