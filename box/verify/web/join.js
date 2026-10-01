// Create an account: a name and a new passkey, signed into a directory
// entry by that same passkey. With an invite code, the grant rides along.

import { creationOptions, requestOptions, attestation, assertion, post, say, u8b64, safeRd } from './webauthn.js';
import { checkInvite, claim } from './invite.js';

async function go() {
  say('…');
  try {
    const username = document.getElementById('u').value.trim().toLowerCase();
    const code = document.getElementById('c').value.trim();
    const email = document.getElementById('e').value.trim();
    if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) throw new Error('give the email address you read your mail at');
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
    // where their mail goes: handed on to be forwarded, and not kept here
    // (box/verify/src/mail_forward.rs). Someone not let in yet sets it from
    // the Email page once they are.
    const onward = safeRd(new URLSearchParams(location.search).get('rd') || '/_dd/home');
    let handed = false;
    try {
      const r = await fetch('/_dd/email', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email }) });
      handed = r.ok && (await r.json()).confirm === true;
    } catch (e) {}
    // a new address: wait here for its owner to confirm it, and move on by
    // ourselves when they have; anything else, straight on
    if (handed) waitForConfirm(email, onward);
    else location.href = onward;
  } catch (e) {
    say('Could not create the account: ' + e.message);
  }
}

// Cloudflare's link opens Cloudflare's page; this one notices and carries on
function waitForConfirm(email, onward) {
  document.getElementById('form').hidden = true;
  document.getElementById('confirm').hidden = false;
  document.getElementById('to').textContent = email;
  document.getElementById('later').href = onward;
  const waiting = document.getElementById('waiting');
  const check = async () => {
    try {
      const r = await fetch('/_dd/email/state');
      if (r.ok && (await r.json()).confirmed === true) {
        waiting.textContent = 'Confirmed. Taking you in…';
        location.href = onward;
        return;
      }
    } catch (e) {}
    setTimeout(check, 3000);
  };
  setTimeout(check, 3000);
  document.getElementById('again').onclick = async () => {
    waiting.textContent = 'Sending it again…';
    try {
      const r = await fetch('/_dd/email/resend', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' });
      waiting.textContent = r.ok ? 'Sent again. Waiting for you to confirm…' : 'That did not work; try again in a minute.';
    } catch (e) {
      waiting.textContent = 'That did not work; try again in a minute.';
    }
  };
}

document.getElementById('go').onclick = go;
