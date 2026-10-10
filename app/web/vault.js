// Passwords in the app's window (src/local.rs): the vault's code is the
// app's own copy, served on this computer. First the window signs in at the
// gate as this device - the vault's sign-in goes through the gate and wants
// its session - and the gate sends it back here; then on to the vault.
const secret = () => ({ 'x-dd-app': document.querySelector('meta[name="dd-app"]')?.content || '' });

async function go() {
  const r = await fetch('/_dd/app/vault/state', { headers: secret() });
  if (!r.ok) throw new Error(await r.text());
  const s = await r.json();
  if (new URLSearchParams(location.search).get('signed')) {
    location.replace(s.door);
    return;
  }
  const f = document.getElementById('signin');
  f.action = s.action;
  document.getElementById('token').value = s.token;
  f.submit();
}

go().catch((e) => { document.getElementById('msg').textContent = 'Could not open Passwords: ' + (e.message || e); });
