// Settings: the member's own, a tab at a time. Profile is their account as
// their entry has it, and their email - which no box holds: the line is a
// frame from the mail Worker (client/mail), and changing it happens there.
// Backups is their disk images (box/fleet/web/backups.js).
import { start as backups, FILES } from './backups.js';
import { start as devices } from './devices.js';

const $ = (id) => document.getElementById(id);

function tab(name) {
  if (!['profile', 'devices', 'backups'].includes(name)) name = 'profile';
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
  if (name === 'devices') devices();
}

for (const a of document.querySelectorAll('[data-tab]')) {
  a.addEventListener('click', (e) => {
    e.preventDefault();
    history.replaceState(null, '', `#${a.dataset.tab}`);
    tab(a.dataset.tab);
  });
}
window.addEventListener('hashchange', () => tab(location.hash.slice(1)));


async function profile() {
  const me = await (await fetch('/_dd/me')).json();
  // a guest has devices and sign-ins here, and no email or disk images
  if (me.role === 'guest') {
    $('email').closest('.se-row').hidden = true;
    document.querySelector('[data-tab="backups"]').hidden = true;
  }
  // from the front door's name, not this page's: the app's copy is on its own
  const domain = new URL(me.home).hostname.split('.').slice(-2).join('.');
  $('name').textContent = me.user;
  $('role').textContent = me.role === 'member' ? 'Member' : me.role === 'guest' ? 'Guest' : me.role;
  const back = `${location.origin}/_dd/settings`;
  $('email').src = `https://mail.${domain}/row?name=${encodeURIComponent(me.user)}&back=${encodeURIComponent(back)}`;
  const { entry } = await (await fetch('/_dd/directory/' + encodeURIComponent(me.user))).json();
  const n = (x, one) => `${x} ${one}${x === 1 ? '' : 's'}`;
  $('devices').firstChild.textContent = `${n((entry.devices || []).length, 'device')}, ${n((entry.passkeys || []).length, 'passkey')}`;
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
