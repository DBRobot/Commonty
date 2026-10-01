// Your passkey, asked here: the Worker hands out a challenge, the browser
// signs it, and the Worker checks the answer against your signed entry.
const b64u = (a) => btoa(String.fromCharCode(...new Uint8Array(a))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const unb64u = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));

async function post(url, body) {
  const r = await fetch(url, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  const v = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(v.error || `that did not work (${r.status})`);
  return v;
}

export async function signIn(name) {
  const c = await post('/api/challenge', { name });
  const a = await navigator.credentials.get({
    publicKey: {
      challenge: unb64u(c.challenge),
      rpId: c.rpId,
      allowCredentials: c.allow.map((id) => ({ type: 'public-key', id: unb64u(id) })),
      userVerification: 'preferred',
    },
  });
  await post('/api/login', {
    token: c.token,
    id: b64u(a.rawId),
    assertion: {
      authenticatorData: b64u(a.response.authenticatorData),
      clientDataJSON: b64u(a.response.clientDataJSON),
      signature: b64u(a.response.signature),
    },
  });
}

/// who this browser is signed in as here, and their email; null if nobody
export async function me() {
  const r = await fetch('/api/me');
  return r.ok ? r.json() : null;
}

export { post };
