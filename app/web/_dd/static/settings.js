// Settings: the member's own, a tab at a time. Profile is their account as
// their entry has it, and their email - which no box holds: the line is a
// frame from the mail Worker (client/mail), and changing it happens there.
// Backups is their disk images (box/fleet/web/backups.js).
import { start as backups, FILES } from './backups.js';

const $ = (id) => document.getElementById(id);

function tab(name) {
  if (!['profile', 'backups'].includes(name)) name = 'profile';
  // the disk images are read from the files site (backups.js)
  if (name === 'backups' && location.protocol === 'https:' && location.hostname !== FILES) {
    location.href = `https://${FILES}/_dd/settings#backups`;
    return;
  }
  for (const a of document.querySelectorAll('[data-tab]')) {
    if (a.dataset.tab === name) a.setAttribute('aria-current', 'page');
    else a.removeAttribute('aria-current');
  }
  for (const p of document.querySelectorAll('[data-pane]')) p.hidden = p.dataset.pane !== name;
  if (name === 'backups') backups();
}

for (const a of document.querySelectorAll('[data-tab]')) {
  a.addEventListener('click', (e) => {
    e.preventDefault();
    history.replaceState(null, '', `#${a.dataset.tab}`);
    tab(a.dataset.tab);
  });
}
window.addEventListener('hashchange', () => tab(location.hash.slice(1)));

const day = (t) => new Date((t < 1e11 ? t : t / 1000) * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });

function added(list) {
  const times = list.map((x) => x.added).filter(Boolean).sort((a, b) => a - b);
  if (!times.length) return '';
  if (times.length === 1) return `added ${day(times[0])}`;
  if (times.length === 2) return `added ${day(times[0])} and ${day(times[1])}`;
  return `first added ${day(times[0])}, latest ${day(times[times.length - 1])}`;
}

function count(el, n, small) {
  el.textContent = String(n);
  if (small) {
    const s = document.createElement('small');
    s.textContent = small;
    el.append(s);
  }
}

async function profile() {
  const me = await (await fetch('/_dd/me')).json();
  // from the front door's name, not this page's: the app's copy is on its own
  const domain = new URL(me.home).hostname.split('.').slice(-2).join('.');
  $('name').textContent = me.user;
  $('address').textContent = `${me.user}@${domain}`;
  $('role').textContent = me.role === 'member' ? 'Member' : me.role === 'guest' ? 'Guest' : me.role;
  const back = `${location.origin}/_dd/settings`;
  $('email').src = `https://mail.${domain}/row?name=${encodeURIComponent(me.user)}&back=${encodeURIComponent(back)}`;
  const { entry } = await (await fetch('/_dd/directory/' + encodeURIComponent(me.user))).json();
  count($('passkeys'), (entry.passkeys || []).length, added(entry.passkeys || []));
  count($('devices'), (entry.devices || []).length, added(entry.devices || []));
  $('recovery').replaceChildren();
  const r = document.createElement('span');
  if (entry.recovery) {
    r.className = 'se-ok';
    r.textContent = 'On file';
  } else {
    r.textContent = 'None';
  }
  $('recovery').append(r);
}

// back from changing the email: say so once, and forget it
const q = new URLSearchParams(location.search);
if (q.get('email') === 'changed') {
  $('banner').hidden = false;
  q.delete('email');
  history.replaceState(null, '', location.pathname + (q.size ? `?${q}` : '') + location.hash);
}
$('banner-x').onclick = () => { $('banner').hidden = true; };

tab(location.hash.slice(1));
profile().catch(() => {});
