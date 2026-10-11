// The photo account's password, made by the person's passkey: its PRF
// secret under a label of Photos' own. The sign-in page (photos.js) opens
// the account with it; Ente's own "confirm it's you" asks for it again
// instead of a typed password (nix/modules/photos, the menu patch). It is
// never stored, and nobody knows it to type.

import { b64u, u8b64 } from './webauthn.js';

export async function passkeySecret(user, cfg) {
  const e = await fetch('/_dd/directory/' + encodeURIComponent(user));
  if (!e.ok) throw new Error('no entry');
  const allow = ((await e.json()).entry.passkeys || []).map(p => ({ type: 'public-key', id: b64u(p.id) }));
  if (!allow.length) throw new Error('this account has no passkey in a browser yet');

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
  const prf = a.getClientExtensionResults().prf;
  const secret = prf && prf.results && prf.results.first;
  if (!secret) throw new Error('this passkey cannot make the photos key on this browser; try your phone');
  return u8b64(secret);
}

/// who is signed in, and the photo account's settings for them
export async function photosConfig() {
  const c = await fetch('/_dd/photos/config', { method: 'POST' });
  if (!c.ok) throw new Error('photos is not on this box');
  return c.json();
}

/// the password again, for a page that already has the account open
export async function photosPassword() {
  const me = await fetch('/_dd/me');
  if (!me.ok) throw new Error('signed out');
  const { user } = await me.json();
  const cfg = await photosConfig();
  return cfg.password || passkeySecret(user, cfg);
}
