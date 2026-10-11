// An account in a browser: a name and a new passkey, signed into a
// directory entry by that same passkey. It signs in and holds no keys: a
// friend's guest, or someone waiting to be let in. A member's account, with
// an invite code, is made in the Commonty app, on the member's own device.

import { creationOptions, requestOptions, attestation, assertion, post, say, safeRd } from './webauthn.js';

async function go() {
  say('…');
  try {
    const username = document.getElementById('u').value.trim().toLowerCase();

    // the passkey
    const start = await post('/_dd/join/start', { username });
    const { publicKey, ceremony } = await start.json();
    const cred = await navigator.credentials.create({ publicKey: creationOptions(publicKey) });
    const finish = await post('/_dd/join/finish', attestation(cred), { 'x-dd-ceremony': ceremony });
    const sign = await finish.json();

    // the entry, signed with it
    say('Once more, to sign your entry with it…');
    const a = await navigator.credentials.get({ publicKey: requestOptions(sign.publicKey) });
    await post('/_dd/join/sign', assertion(a), { 'x-dd-ceremony': sign.ceremony });

    try { localStorage.setItem('dd_user', username); } catch (e) {}
    location.href = new URL(safeRd(new URLSearchParams(location.search).get('rd') || '/_dd/home'), location.href).href;
  } catch (e) {
    say('Could not create the account: ' + e.message);
  }
}

document.getElementById('go').onclick = go;
