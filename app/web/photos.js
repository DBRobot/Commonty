// Photos in the app's window (src/photos.rs): sign the window in as this
// device and go on to the Photos page, the password after the # of its
// address; or, the first time here, have the browser's passkey send it.
const $ = (id) => document.getElementById(id);

async function state() {
  const r = await fetch('/_dd/app/photos/state');
  if (!r.ok) throw new Error(await r.text());
  return r.json();
}

function go(s) {
  const f = $('signin');
  // the fragment rides the redirect to the Photos page; no server sees it
  f.action = `${s.action}#${s.hash}`;
  $('token').value = s.token;
  f.submit();
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
    const r = await fetch('/_dd/app/photos/start');
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
