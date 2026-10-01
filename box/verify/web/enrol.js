// Add a passkey to an entry the terminal made: the link carries a token;
// the terminal signs the new passkey into the entry once it appears.

import { creationOptions, attestation, post, say, safeRd, u8b64 } from './webauthn.js';
import init, { library_device_key } from '/_dd/web/dd_web.js';

// The key the Files and Movies pages open the library with, made from this
// passkey's own secret under the same label they use (library.js). Sent
// with the new passkey, so the device that signs it into the entry seals
// the libraries to it at the same time and nobody links anything by hand.
async function libraryKey(cred, rpId) {
  const salt = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode('dd-library')));
  // some browsers answer at creation; the rest need the passkey once more
  let secret = cred.getClientExtensionResults().prf?.results?.first;
  if (!secret) {
    const a = await navigator.credentials.get({
      publicKey: {
        challenge: crypto.getRandomValues(new Uint8Array(32)),
        rpId,
        allowCredentials: [{ type: 'public-key', id: cred.rawId }],
        userVerification: 'preferred',
        extensions: { prf: { eval: { first: salt } } },
      },
    });
    secret = a.getClientExtensionResults().prf?.results?.first;
  }
  if (!secret) return null;
  await init();
  return library_device_key(u8b64(secret));
}

const q = new URLSearchParams(location.search);
const rd = safeRd(q.get('rd'));
const token = q.get('t');
const auth = token ? { authorization: 'Bearer ' + token } : {};

async function go() {
  say('…');
  try {
    const r = await fetch('/_dd/enrol/start', { method: 'POST', headers: auth });
    if (r.status === 401) {
      say('This link is not valid, or has expired.');
      return;
    }
    if (!r.ok) throw new Error(await r.text());
    const { publicKey, ceremony } = await r.json();
    const options = creationOptions(publicKey);
    const salt = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode('dd-library')));
    options.extensions = { ...options.extensions, prf: { eval: { first: salt } } };
    const cred = await navigator.credentials.create({ publicKey: options });
    say('One more touch, for the key your files open with…');
    let library_key = null;
    try {
      library_key = await libraryKey(cred, options.rp.id);
    } catch (e) {
      // no key: the passkey still signs in; the files page asks for it later
    }
    const finish = await post('/_dd/enrol/finish', { ...attestation(cred), library_key }, { 'x-dd-ceremony': ceremony, ...auth });
    const { id, user } = await finish.json();

    say('Passkey made. Waiting for the terminal to sign it into your entry…');
    for (let i = 0; i < 90; i++) {
      await new Promise(res => setTimeout(res, 2000));
      try {
        const e = await fetch('/_dd/directory/' + encodeURIComponent(user));
        if (!e.ok) continue;
        const entry = (await e.json()).entry;
        if ((entry.passkeys || []).some(p => p.id === id)) {
          say('Signed in to your entry. Taking you to sign in…');
          location.href = '/_dd/login?rd=' + encodeURIComponent(rd);
          return;
        }
      } catch (e) {}
    }
    say('Passkey made, but it has not appeared in your entry yet. Once the terminal reports it published, sign in.');
  } catch (e) {
    say('Failed: ' + e.message);
  }
}

document.getElementById('go').onclick = go;
