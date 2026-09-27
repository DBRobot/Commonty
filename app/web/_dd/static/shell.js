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
// Photos in the app is Ente's page, on the photos host, in the app's own
// window. It knows it is there by what the app's page left in this tab
// (photos.js), and its bar goes back to the app's pages, not the site's.
const appOrigin = (() => {
  try { return sessionStorage.getItem('dd-app-origin'); } catch { return null; }
})();
export const inApp = location.protocol === 'commonty:' || location.hostname === 'commonty.localhost' || !!appOrigin;
// where the app's pages are, from wherever this page is
const appPages = appOrigin || '';

// the app's own screen for this device, where it signs out; ?stay keeps
// it from coming straight back here
const deviceScreen = ((appOrigin || location.origin).startsWith('commonty:') ? 'tauri://localhost/' : 'http://tauri.localhost/') + '?stay';

// pages the app carries: a service that is one of them opens the app's
// copy, Photos opens in the app's window, and any other (games, code)
// opens in the device's browser
const carried = ['/_dd/home', '/_dd/files', '/_dd/media', '/_dd/boxes', '/_dd/backups', '/_dd/devices', '/_dd/network'];
function here(url) {
  if (!inApp) return url;
  try {
    const u = new URL(url, location.href);
    if (carried.includes(u.pathname)) return appPages + u.pathname;
    if (u.pathname === '/_dd/photos') return appPages + '/_dd/app/photos';
    // in the app's window already (Photos): the window may go there itself
    if (appOrigin) return url;
    if (u.origin !== location.origin) return '/_dd/app/open?url=' + encodeURIComponent(u.href);
    return url;
  } catch {
    return url;
  }
}

// A link out of the app (Photos, Games, Code) is the app's to open in the
// device's browser. Followed as a link, the window went to an answer with
// nothing in it and stayed blank; asked for instead, the page stays put.
if (inApp) {
  document.addEventListener('click', (e) => {
    const a = e.target.closest?.('a[href^="/_dd/app/open"]');
    if (!a) return;
    e.preventDefault();
    fetch(a.getAttribute('href')).catch(() => {});
  });
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
  // the logo goes home: the app's home, when this page is in the app
  const brand = document.querySelector('header.dd-bar .brand');
  if (brand && appOrigin) brand.setAttribute('href', appOrigin + '/_dd/home');
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
