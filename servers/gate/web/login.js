// Sign in: a name, then the passkey.

import { requestOptions, assertion, post, say, safeRd } from './webauthn.js';

const rd = safeRd(new URLSearchParams(location.search).get('rd'));

async function go() {
  say('…');
  try {
    const username = document.getElementById('u').value.trim();
    try { localStorage.setItem('dd_user', username); } catch (e) {}

    const start = await post('/_dd/login/start', { username });
    const { publicKey, ceremony } = await start.json();
    const cred = await navigator.credentials.get({ publicKey: requestOptions(publicKey) });
    await post('/_dd/login/finish', assertion(cred), { 'x-dd-ceremony': ceremony });
    location.href = rd;
  } catch (e) {
    say('Sign-in failed: ' + e.message);
  }
}

document.getElementById('go').onclick = go;
try { document.getElementById('u').value = localStorage.getItem('dd_user') || ''; } catch (e) {}
