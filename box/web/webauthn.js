// What every passkey page needs: base64url both ways, and the shapes the
// browser's credential API takes and gives, for the JSON the verifier speaks.

export const b64u = s =>
  Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), c => c.charCodeAt(0));

export const u8b64 = a =>
  btoa(String.fromCharCode(...new Uint8Array(a)))
    .replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');

// Where a page may send you after signing in. Only a path on this box:
// anything else - another origin, a protocol-relative //host, a
// javascript: url - is someone else's idea of where you should go, and the
// `rd` it travels in comes from the query string.
export function safeRd(raw) {
  const p = raw || '/';
  return p.startsWith('/') && !p.startsWith('//') && !p.includes('\\') ? p : '/';
}

// the options the verifier sends, with their byte fields decoded
export function creationOptions(publicKey) {
  publicKey.challenge = b64u(publicKey.challenge);
  publicKey.user.id = b64u(publicKey.user.id);
  if (publicKey.excludeCredentials) {
    publicKey.excludeCredentials = publicKey.excludeCredentials.map(c => ({ ...c, id: b64u(c.id) }));
  }
  // ask for the PRF extension: Photos derives its key from it
  publicKey.extensions = { ...(publicKey.extensions || {}), prf: {} };
  return publicKey;
}

export function requestOptions(publicKey) {
  publicKey.challenge = b64u(publicKey.challenge);
  if (publicKey.allowCredentials) {
    publicKey.allowCredentials = publicKey.allowCredentials.map(c => ({ ...c, id: b64u(c.id) }));
  }
  return publicKey;
}

// a new credential, as the verifier reads it
export function attestation(cred) {
  return {
    id: cred.id,
    rawId: u8b64(cred.rawId),
    type: cred.type,
    extensions: cred.getClientExtensionResults(),
    response: {
      attestationObject: u8b64(cred.response.attestationObject),
      clientDataJSON: u8b64(cred.response.clientDataJSON),
    },
  };
}

// a signature with an existing credential, as the verifier reads it
export function assertion(a) {
  return {
    id: a.id,
    rawId: u8b64(a.rawId),
    type: a.type,
    extensions: a.getClientExtensionResults(),
    response: {
      authenticatorData: u8b64(a.response.authenticatorData),
      clientDataJSON: u8b64(a.response.clientDataJSON),
      signature: u8b64(a.response.signature),
      userHandle: a.response.userHandle ? u8b64(a.response.userHandle) : null,
    },
  };
}

// POST json, throw the server's words on failure
export async function post(url, body, headers = {}) {
  const r = await fetch(url, {
    method: 'POST',
    headers: { 'content-type': 'application/json', ...headers },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  if (!r.ok) throw new Error(await r.text());
  return r;
}

export const say = text => { document.getElementById('msg').textContent = text; };
