// A browser asking to open Photos, Passwords or the site's pages that hold
// a key (src/local.rs): it shows a code made from what only that browser
// holds; the member lets it in here when the codes match. Two browsers
// asking at once are both refused.
const secret = () => ({ 'x-dd-app': document.querySelector('meta[name="dd-app"]')?.content || '' });
const $ = (id) => document.getElementById(id);
const names = { photos: 'Photos', vault: 'Passwords', site: 'your files and settings' };
let shown = null;
let done = false;

const spaced = (c) => `${c.slice(0, 3)} ${c.slice(3)}`;

async function answer(code) {
  const r = await fetch('/_dd/app/pair/answer', {
    method: 'POST',
    headers: { ...secret(), 'content-type': 'application/json' },
    body: JSON.stringify({ service: shown.service, code }),
  });
  if (!r.ok) throw new Error(await r.text());
}

function show(service, s) {
  $('asking').hidden = s.state !== 'asking';
  if (s.state === 'asking') {
    shown = { service, code: s.code };
    $('title').textContent = `Allow this browser to open ${names[service]}?`;
    $('code').textContent = spaced(s.code);
    $('msg').textContent = '';
  } else if (s.state === 'crowded') {
    shown = null;
    $('title').textContent = 'Not opened';
    $('msg').textContent = `Two browsers asked to open ${names[service]} at once, so neither was let in. Open it again from the page in your browser.`;
  }
}

async function poll() {
  for (;;) {
    const r = await fetch('/_dd/app/pair/state', { headers: secret() });
    if (!r.ok) throw new Error(await r.text());
    const s = await r.json();
    const service = ['photos', 'vault', 'site'].find((k) => s[k] && s[k].state !== 'idle');
    if (!service) {
      if (!shown && !done) $('msg').textContent = 'Nothing is asking now. Open it again from its page in your browser.';
      return;
    }
    if (s[service].state === 'waiting' && shown?.allowed) return;
    show(service, s[service]);
    if (s[service].state === 'crowded') return;
    await new Promise((ok) => setTimeout(ok, 1000));
  }
}

$('allow').onclick = async () => {
  $('allow').disabled = true;
  try {
    await answer(shown.code);
    shown.allowed = true;
    $('asking').hidden = true;
    $('title').textContent = 'Allowed';
    $('msg').textContent = `Go back to your browser: ${names[shown.service]} opens there.`;
  } catch (e) {
    $('msg').textContent = String(e.message || e);
  } finally {
    $('allow').disabled = false;
  }
};

$('refuse').onclick = async () => {
  try {
    await answer(null);
  } catch {}
  shown = null;
  done = true;
  $('asking').hidden = true;
  $('title').textContent = 'Not opened';
  $('msg').textContent = 'That browser was not let in.';
};

poll().catch((e) => { $('msg').textContent = String(e.message || e); });
