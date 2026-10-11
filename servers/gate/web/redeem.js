// An account that is made but not yet a member: a code from whoever runs
// this network gets it in. The grant is signed with the code's key and
// confirmed with the passkey.

import { requestOptions, assertion, post, say } from './webauthn.js';
import { checkInvite, claim } from './invite.js';

async function go() {
  say('…');
  try {
    const code = document.getElementById('c').value.trim();
    if (!code) throw new Error('type the code');
    await checkInvite(code);

    const user = document.querySelector('.card').dataset.user;
    const e = await fetch('/_dd/directory/' + encodeURIComponent(user));
    if (!e.ok) throw new Error('no entry');
    const root = (await e.json()).entry.root;

    const grant = btoa(JSON.stringify(await claim(code, root)));
    const start = await fetch('/_dd/redeem/start', { method: 'POST', headers: { 'x-dd-grant': grant } });
    if (!start.ok) throw new Error(await start.text());
    const s = await start.json();

    say('Confirm with your passkey…');
    const a = await navigator.credentials.get({ publicKey: requestOptions(s.publicKey) });
    await post('/_dd/redeem/sign', assertion(a), { 'x-dd-ceremony': s.ceremony });
    location.href = '/_dd/home';
  } catch (e) {
    say('That did not work: ' + e.message);
  }
}

document.getElementById('go').onclick = go;
