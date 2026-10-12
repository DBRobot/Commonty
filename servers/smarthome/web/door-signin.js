// Signing in at the home's door (servers/smarthome/door/src/signin.rs): the
// member's passkey answers the door's challenge for this name. The passkey's
// relying party is the bare domain, which the door's name sits under.
import { $ } from './kit.js';
import { u8b64 } from './webauthn.js';

const host = location.hostname;
// <label>.home.<domain> → <domain>
const rpId = host.split('.').slice(2).join('.');
const say = (t) => { $('msg').textContent = t; };

async function challenge(nonce) {
  const m = new TextEncoder().encode(`commonty door v1\0${host}\0${nonce}`);
  return new Uint8Array(await crypto.subtle.digest('SHA-256', m));
}

// asked for before the click, so the click goes straight to the passkey
let nonce = fetch('/_door/challenge').then((r) => r.json()).then((v) => v.nonce);

async function signIn() {
  $('go').disabled = true;
  say('');
  try {
    const n = await nonce;
    // each challenge is good once: the next try gets a fresh one
    nonce = fetch('/_door/challenge').then((r) => r.json()).then((v) => v.nonce);
    const a = await navigator.credentials.get({
      publicKey: { challenge: await challenge(n), rpId, userVerification: 'preferred', timeout: 60000 },
    });
    const body = {
      id: a.id,
      nonce: n,
      assertion: {
        authenticatorData: u8b64(a.response.authenticatorData),
        clientDataJSON: u8b64(a.response.clientDataJSON),
        signature: u8b64(a.response.signature),
      },
    };
    const name = $('name').value.trim();
    if (!$('who').hidden && name) body.name = name;
    const r = await fetch('/_door/signin', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body),
    });
    if (r.status === 404 && $('who').hidden) {
      $('who').hidden = false;
      $('name').focus();
      say('This home does not know that passkey yet. Say your name and sign in again.');
      return;
    }
    if (!r.ok) { say((await r.text()) || 'This home did not let that sign in.'); return; }
    // in: the page asked for, now through the door; not in: where to ask
    location.reload();
  } catch (e) {
    say(e && e.name === 'NotAllowedError' ? 'Signing in was cancelled.' : 'Signing in did not work here. Try again, or open the Commonty app.');
  } finally {
    $('go').disabled = false;
  }
}

$('go').addEventListener('click', signIn);
