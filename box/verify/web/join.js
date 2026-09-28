// Create an account: a name and a new passkey, signed into a directory
// entry by that same passkey. With an invite code, the grant rides along.

import { creationOptions, requestOptions, attestation, assertion, post, say, u8b64, safeRd } from './webauthn.js';
import { checkInvite, claim } from './invite.js';

async function go() {
  say('…');
  try {
    const username = document.getElementById('u').value.trim().toLowerCase();
    const code = document.getElementById('c').value.trim();
    if (code) await checkInvite(code);

    // the passkey
    const start = await post('/_dd/join/start', { username });
    const { publicKey, ceremony } = await start.json();
    const cred = await navigator.credentials.create({ publicKey: creationOptions(publicKey) });

    const headers = { 'x-dd-ceremony': ceremony };
    if (code) {
      headers['x-dd-grant'] = btoa(JSON.stringify(await claim(code, 'webauthn:' + u8b64(cred.rawId))));
    }
    const finish = await post('/_dd/join/finish', attestation(cred), headers);
    const sign = await finish.json();

    // the entry, signed with it
    say('Once more, to sign your entry with it…');
    const a = await navigator.credentials.get({ publicKey: requestOptions(sign.publicKey) });
    await post('/_dd/join/sign', assertion(a), { 'x-dd-ceremony': sign.ceremony });

    try { localStorage.setItem('dd_user', username); } catch (e) {}
    // back where they came from (a friend link), or home
    location.href = safeRd(new URLSearchParams(location.search).get('rd') || '/_dd/home');
  } catch (e) {
    say('Could not create the account: ' + e.message);
  }
}

document.getElementById('go').onclick = go;
