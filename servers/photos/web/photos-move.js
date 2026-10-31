// The move of a photo account to the member's app (servers/gate/src/photos.rs,
// move_page). Before the app held the password, a passkey's PRF made it,
// under the label 'dd-photos'. Any page of the domain can ask a passkey for
// that, so it was never a secret from the boxes. This page asks for it one
// last time and hands it to the app sealed to a key only the app holds; the
// app gives the account a password of its own straight away, and after
// that the secret asked here opens nothing. It is the only page that asks
// a passkey's PRF (a test in servers/gate/src/pages.rs keeps it so), and it
// asks for this label and no other.

import init, { seal_for_app } from '/_dd/web/dd_web.js';
import { b64u, u8b64, say } from './webauthn.js';
import { readEntry } from './kit.js';

const q = new URLSearchParams(location.search);
const slot = q.get('slot');
const to = q.get('to');

async function oldPassword(user) {
  const e = await fetch('/_dd/directory/' + encodeURIComponent(user));
  if (!e.ok) throw new Error('no entry');
  const allow = (readEntry(await e.text()).entry.passkeys || []).map((p) => ({ type: 'public-key', id: b64u(p.id) }));
  if (!allow.length) throw new Error('this account has no passkey, so there is nothing to move');
  const cfg = await (await fetch('/_dd/config')).json();
  const salt = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode('dd-photos')));
  const a = await navigator.credentials.get({
    publicKey: {
      challenge: crypto.getRandomValues(new Uint8Array(32)),
      rpId: cfg.rpId,
      allowCredentials: allow,
      userVerification: 'preferred',
      extensions: { prf: { eval: { first: salt } } },
    },
  });
  const secret = a.getClientExtensionResults().prf?.results?.first;
  if (!secret) throw new Error('this passkey cannot answer on this browser; try the one you used Photos with');
  return u8b64(secret);
}

async function go() {
  document.getElementById('go').disabled = true;
  try {
    if (!slot || !to) throw new Error('open this from the Commonty app');
    const user = document.querySelector('.card').dataset.user;
    say('Confirm with your passkey.');
    const old = await oldPassword(user);
    await init();
    const r = await fetch('/_dd/photos/handoff/' + encodeURIComponent(slot), { method: 'PUT', body: seal_for_app(to, old) });
    if (r.status === 404) throw new Error('this link has expired: start the move from the app again');
    if (!r.ok) throw new Error(`the box said ${r.status}`);
    say('Done. The app is taking Photos over; you can close this tab.');
  } catch (e) {
    say(e.name === 'NotAllowedError' ? 'Your passkey was not used. Nothing moved.' : `That did not work: ${e.message}`);
    document.getElementById('go').disabled = false;
  }
}

document.getElementById('go').onclick = go;
