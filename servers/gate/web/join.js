// An account in a browser: a name and a new passkey, signed into a
// directory entry by that same passkey. It signs in and holds no keys: a
// friend's guest, or someone waiting to be let in. A member's account, with
// an invite code, is made in the Commonty app, on the member's own device.
// The age question is answered here and never leaves the page; the box is
// told only that the terms were agreed to, and the entry records which.

import { creationOptions, requestOptions, attestation, assertion, post, say, safeRd } from './webauthn.js';
import { consent } from './age.js';

const button = document.getElementById('go');
const ready = await consent(button);

// The terms open in this tab: the name typed is kept for this tab alone
// and put back on the way back. The birth date is never kept, so it is
// asked again.
const nameField = document.getElementById('u');
const KEEP = 'dd_join_name';
try { nameField.value ||= sessionStorage.getItem(KEEP) || ''; } catch { /* storage off */ }
nameField.addEventListener('input', () => {
  try { sessionStorage.setItem(KEEP, nameField.value); } catch { /* storage off */ }
});

async function go() {
  if (!ready()) return;
  button.disabled = true;
  say('…');
  try {
    const username = nameField.value.trim().toLowerCase();

    // the passkey
    const start = await post('/_dd/join/start', { username, agree: true });
    const { publicKey, ceremony } = await start.json();
    const cred = await navigator.credentials.create({ publicKey: creationOptions(publicKey) });
    const finish = await post('/_dd/join/finish', attestation(cred), { 'x-dd-ceremony': ceremony });
    const sign = await finish.json();

    // the entry, signed with it
    say('Once more, to sign your entry with it…');
    const a = await navigator.credentials.get({ publicKey: requestOptions(sign.publicKey) });
    await post('/_dd/join/sign', assertion(a), { 'x-dd-ceremony': sign.ceremony });

    try { localStorage.setItem('dd_user', username); sessionStorage.removeItem(KEEP); } catch { /* storage off */ }
    location.href = new URL(safeRd(new URLSearchParams(location.search).get('rd') || '/_dd/home'), location.href).href;
  } catch (e) {
    say('Could not create the account: ' + e.message);
    button.disabled = !ready();
  }
}

button.onclick = go;
