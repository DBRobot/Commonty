// Turning on a locked inbox: the app opens this with its mail key's public
// half and its device key in the fragment (never sent to anyone), the
// member checks the code matches what their app shows, and their passkey,
// on this Worker's own page, says it is theirs (inbox.js).
import { signIn, post } from './passkey.js';

const $ = (id) => document.getElementById(id);
const q = new URLSearchParams(location.hash.slice(1));
const name = q.get('name') || '';
const pk = q.get('pk') || '';
const fp = q.get('fp') || '';
const key = q.get('key') || '';
const back = q.get('back') || '';
const sig = q.get('sig') || '';
// the code both sides show: the device and the key it locks to, short enough to read
async function code() {
  const d = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(`${name}\0${fp}\0${pk}`)));
  const hex = [...d.slice(0, 6)].map((b) => b.toString(16).padStart(2, '0')).join('');
  return hex.match(/..../g).join('-');
}

if (!name || !pk || !fp || !key || !sig) {
  $('s1').hidden = true;
  $('s0').hidden = false;
} else {
  $('addr').textContent = `${name}@${document.body.dataset.domain}`;
  code().then((c) => { $('code').textContent = c; });
  // back only to the fleet's own pages, or the app's on this computer
  try {
    const u = new URL(back);
    const d = document.body.dataset.domain;
    const ours = (u.protocol === 'https:' && (u.hostname === d || u.hostname.endsWith(`.${d}`))) || (u.protocol === 'http:' && /^s-[0-9a-f]{32}\.localhost$/.test(u.hostname));
    if (ours) $('back').href = u.href; else $('back').hidden = true;
  } catch { $('back').hidden = true; }
}

$('pass').onclick = async () => {
  $('pass').disabled = true;
  $('s1-msg').hidden = true;
  try {
    await signIn(name);
    await post('/api/inbox', { pk, sig, device: { fp, key }, fwd: $('fwd').checked });
    $('s1').hidden = true;
    $('s2').hidden = false;
  } catch (e) {
    $('s1-msg').textContent = e.name === 'NotAllowedError' ? 'Your passkey was not used. Try again.' : String(e.message || e);
    $('s1-msg').hidden = false;
  } finally {
    $('pass').disabled = false;
  }
};
