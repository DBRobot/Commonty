// The email line on Settings: whether your email is confirmed, the address
// itself once your passkey has been shown here, and the way to change it.
import { signIn, me, post } from './passkey.js';

const $ = (id) => document.getElementById(id);
const q = new URLSearchParams(location.search);
const name = q.get('name') || '';
const settings = q.get('back') || '';

function status(forwarding, confirmed) {
  $('status').textContent = !forwarding ? 'No email yet' : confirmed ? 'Confirmed' : 'Not confirmed yet';
  $('status').className = confirmed ? 'ok' : '';
}

async function load() {
  const m = await me().catch(() => null);
  if (m && m.name === name) {
    $('addr').textContent = m.email || '';
    $('addr').hidden = !m.email;
    $('show').hidden = true;
    status(!!m.email, m.confirmed);
    return;
  }
  const r = await fetch(`/api/state?name=${encodeURIComponent(name)}`);
  const s = r.ok ? await r.json() : { forwarding: false, confirmed: false };
  $('show').hidden = !s.forwarding;
  status(s.forwarding, s.confirmed);
}

$('change').href = `/change?name=${encodeURIComponent(name)}&back=${encodeURIComponent(settings)}`;
$('show').onclick = async () => {
  $('show').disabled = true;
  try {
    await signIn(name);
    await // Settings, on "sign out everywhere else" or a device removed: this
// browser's session stays, the others here end. Only the fleet's own pages
// may ask, and all they learn is whether it was done.
const domain = location.hostname.split('.').slice(1).join('.');
window.addEventListener('message', async (e) => {
  if (e.data !== 'end-others' || e.source !== parent) return;
  if (!/^https:\/\/[a-z0-9.-]+$/.test(e.origin) || !e.origin.endsWith(`.${domain}`)) return;
  const ok = await post('/api/end-others', {}).then(() => true, () => false);
  e.source.postMessage({ endedOthers: ok }, e.origin);
});

load();
  } catch {
    $('show').disabled = false;
  }
};

// Settings, on "sign out everywhere else" or a device removed: this
// browser's session stays, the others here end. Only the fleet's own pages
// may ask, and all they learn is whether it was done.
const domain = location.hostname.split('.').slice(1).join('.');
window.addEventListener('message', async (e) => {
  if (e.data !== 'end-others' || e.source !== parent) return;
  if (!/^https:\/\/[a-z0-9.-]+$/.test(e.origin) || !e.origin.endsWith(`.${domain}`)) return;
  const ok = await post('/api/end-others', {}).then(() => true, () => false);
  e.source.postMessage({ endedOthers: ok }, e.origin);
});

load();
