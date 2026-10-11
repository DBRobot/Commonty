// Photos in the app's window (src/photos.rs): on to the app's own Photos
// page, which is handed the password this device keeps, once.
// the app's routes want the secret it put in this page (app/src/site.rs)
const secret = () => ({ 'x-dd-app': document.querySelector('meta[name="dd-app"]')?.content || '' });

async function go() {
  const r = await fetch('/_dd/app/photos/state', { headers: secret() });
  if (!r.ok) throw new Error(await r.text());
  const s = await r.json();
  if (s.ready) location.replace(s.url);
}

go().catch((e) => { document.getElementById('msg').textContent = String(e.message || e); });
