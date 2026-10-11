// What Ente's own "confirm it's you" asks for the account's password (the
// menu patch in servers/photos imports this as /_dd/static/photos-passkey.js).
// In the app it is the password this device keeps, from the app that serves
// this page (src/local.rs); nobody types it, and no passkey makes it.

export async function photosPassword() {
  const r = await fetch('/_dd/app/photos-confirm', { method: 'POST' });
  if (!r.ok) throw new Error('Photos has no key on this device');
  return (await r.json()).pw;
}
