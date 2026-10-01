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
const carried = ['/_dd/home', '/_dd/files', '/_dd/media', '/_dd/boxes', '/_dd/settings', '/_dd/devices', '/_dd/network'];
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

// A setting that belongs at hand (Ad blocking at home): its name opens its
// page, the switch beside it flips it. Its state is asked for when the menu
// opens, not on every page.
function toggleRow(item) {
  const sw = el('button', { type: 'button', class: 'switch', role: 'switch', 'aria-checked': 'false', 'aria-label': item.label });
  sw.disabled = true;
  const show = (s) => {
    sw.disabled = false;
    sw.setAttribute('aria-checked', String(!!s.on));
    sw.title = s.on ? 'On' : s.resumesIn ? 'Paused' : 'Off';
  };
  const read = () => fetch(item.toggle).then((r) => (r.ok ? r.json() : Promise.reject(r))).then(show).catch(() => { sw.title = 'Not answering'; });
  sw.addEventListener('click', async (e) => {
    e.preventDefault();
    const on = sw.getAttribute('aria-checked') !== 'true';
    sw.disabled = true;
    try {
      const r = await fetch(item.switch, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ on }) });
      if (r.ok) show(await r.json()); else read();
    } catch { read(); }
  });
  const row = el('div', { class: 'toggle' }, el('a', { href: here(item.home || item.url), text: item.label }), sw);
  row.read = read;
  return row;
}

// The bar's own pages live on the home site. On another of the fleet's
// sites (Passwords, Git, Metrics) a bare /_dd/ link would open them there,
// and every page after it would stay on that site.
function homeLink(m, url) {
  return url.startsWith('/') && m.home ? m.home + url : url;
}

// Every page's tab shows the Commonty mark. Without one of its own, a
// browser keeps whatever icon it last stored for the address.
if (!document.querySelector('link[rel~="icon"]')) {
  document.head.append(Object.assign(document.createElement('link'), { rel: 'icon', type: 'image/svg+xml', href: '/_dd/static/favicon.svg' }));
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
      if (item.toggle) {
        nav.append(toggleRow({ ...item, home: homeLink(m, item.url) }));
        continue;
      }
      nav.append(el('a', { href: here(homeLink(m, item.url)), text: item.label }));
    }
  });
  const summary = el('summary', {},
    el('span', { class: 'avatar', 'aria-hidden': 'true', text: m.initial }),
    el('span', { text: m.user }),
    el('span', { class: 'chev', 'aria-hidden': 'true' }));
  // the logo goes home: the app's home, when this page is in the app
  const brand = document.querySelector('header.dd-bar .brand');
  if (brand && appOrigin) brand.setAttribute('href', appOrigin + '/_dd/home');
  else if (brand) brand.setAttribute('href', homeLink(m, '/_dd/home'));
  const menu = el('details', { class: 'menu' }, summary, nav);
  menu.addEventListener('toggle', () => {
    if (menu.open) nav.querySelectorAll('.toggle').forEach((t) => t.read());
  });
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

// A service's mark: the box's own svg, not anything a person wrote
function mark(s) {
  const m = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  m.setAttribute('viewBox', '0 0 20 20');
  m.setAttribute('fill', 'none');
  m.setAttribute('stroke-width', '1.5');
  m.setAttribute('aria-hidden', 'true');
  m.innerHTML = s.icon;
  return m;
}

// Left of the name: every service, to go from one to another without
// going home first, in the home page's order; and Friends.
function links(m) {
  const row = document.querySelector('header.dd-bar .bar');
  const brand = row?.querySelector('.brand');
  if (!row || !brand || row.querySelector('.links')) return;
  const nav = el('nav', { class: 'links', 'aria-label': 'Main' });
  const open = m.services.filter((s) => !s.shut);
  if (open.length) {
    const chev = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
    chev.setAttribute('viewBox', '0 0 12 12');
    chev.setAttribute('aria-hidden', 'true');
    chev.innerHTML = '<path d="M2.5 4.5 6 8l3.5-3.5" fill="none" stroke="currentColor" stroke-width="1.6"/>';
    const btn = el('button', { type: 'button', class: 'svc-btn', 'aria-expanded': 'false', 'aria-controls': 'dd-services' }, el('span', { text: 'Services' }), chev);
    const list = el('div', { class: 'svc-menu', id: 'dd-services' });
    list.hidden = true;
    for (const s of open) {
      const a = el('a', { class: 's', href: here(s.url) },
        mark(s),
        el('span', {}, el('b', { text: s.name }), el('span', { text: s.blurb })));
      if (s.color) a.style.setProperty('--c', s.color);
      if (s.host === location.host) a.setAttribute('aria-current', 'page');
      list.append(a);
    }
    list.append(el('a', { class: 'all', href: here(m.home + '/_dd/home'), text: 'All services on the home page' }));
    const show = (on) => {
      list.hidden = !on;
      btn.setAttribute('aria-expanded', String(on));
    };
    btn.onclick = (e) => {
      e.stopPropagation();
      show(list.hidden);
    };
    document.addEventListener('click', (e) => {
      if (!list.hidden && !list.contains(e.target)) show(false);
    });
    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape' && !list.hidden) {
        show(false);
        btn.focus();
      }
    });
    nav.append(el('div', { class: 'svc' }, btn, list));
  }
  if (!m.demo) {
    const f = el('a', { class: 'link', href: here(m.home + '/_dd/friends'), text: 'Friends' });
    if (location.pathname === '/_dd/friends') f.setAttribute('aria-current', 'page');
    nav.append(f);
  }
  brand.after(nav);
}

// a guest's home: what they are, and a way to see the rest
function guestHome(m) {
  const intro = document.querySelector('.intro');
  if (intro) {
    intro.querySelector('h1').textContent = `Hello, ${m.user}`;
    intro.querySelector('p').textContent = m.guestOf
      ? `You're a guest of ${m.guestOf}'s. You can join the game servers your friends invite you to.`
      : "You're a guest here. You can join the game servers your friends invite you to.";
  }
  const main = document.querySelector('main.home');
  if (!main || main.querySelector('.guest-more')) return;
  main.append(el('section', { class: 'guest-more' },
    el('h2', { text: 'Want to see what else Commonty can do?' }),
    el('p', { text: 'Files, photos, films and more, kept on boxes the people in it own. The demo shows a member\'s view. Opening it signs you out of your account here; sign back in after.' }),
    el('a', { class: 'button', href: m.home + '/_dd/demo', text: 'Try the demo' })));
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
    if (s.color) row.style.setProperty('--c', s.color);
    row.append(
      mark(s),
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
  links(m);
  if (m.role === 'guest' && document.getElementById('services')) guestHome(m);
  const list = document.getElementById('services');
  if (list) services(m, list);
  const banner = document.getElementById('demo');
  if (banner) banner.hidden = !m.demo;
  for (const e of document.querySelectorAll('[data-user]')) e.dataset.user = m.user;
}).catch((e) => {
  const list = document.getElementById('services');
  if (list) list.replaceWith(el('p', { class: 'empty', text: String(e.message || e) }));
});
