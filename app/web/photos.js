// Photos in the app's window (src/photos.rs): go on to the app's own Photos
// page, which asks the app for the password once; or, the first time here,
// have the browser's passkey send it.
// the app's routes want the secret it put in this page (app/src/site.rs)
const secret = () => ({ 'x-dd-app': document.querySelector('meta[name="dd-app"]')?.content || '' });
const $ = (id) => document.getElementById(id);

async function state() {
  const r = await fetch('/_dd/app/photos/state', { headers: secret() });
  if (!r.ok) throw new Error(await r.text());
  return r.json();
}

// the app's own copy of Photos, on this computer
function go(s) {
  location.replace(s.url);
}

async function poll() {
  for (;;) {
    const s = await state();
    if (s.ready) return go(s);
    if (s.expired || s.needs) {
      $('wait').hidden = true;
      $('first').hidden = false;
      $('msg').textContent = s.expired ? 'That took too long. Try again.' : 'Photos is not open on this device yet.';
      return;
    }
    await new Promise((ok) => setTimeout(ok, 2000));
  }
}

$('go').onclick = async () => {
  $('go').disabled = true;
  try {
    const r = await fetch('/_dd/app/photos/start', { headers: secret() });
    if (!r.ok) throw new Error(await r.text());
    $('first').hidden = true;
    $('wait').hidden = false;
    $('msg').textContent = 'Confirm in your browser.';
    await poll();
  } catch (e) {
    $('msg').textContent = String(e.message || e);
  } finally {
    $('go').disabled = false;
  }
};

poll().catch((e) => { $('msg').textContent = String(e.message || e); });
