// The browser's way into what the app serves it (src/local.rs,
// src/handoff.rs). This page's address is no secret, so the proof is made
// here: a random verifier that never leaves this tab until the member has
// let it in, and only its hash goes to the app. The app's window and this
// page show the same code from that hash; once the member allows it there,
// the verifier buys this browser its own session.
const $ = (id) => document.getElementById(id);
const say = (t) => { $('msg').textContent = t; };
const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');

const post = (path, body) =>
  fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });

async function go() {
  const verifier = hex(crypto.getRandomValues(new Uint8Array(32)));
  const hash = hex(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier)));
  const r = await post('/_dd/app/pair/ask', { hash });
  if (!r.ok) {
    const { why } = await r.json().catch(() => ({}));
    $('title').textContent = 'Not opened';
    say(why === 'two browsers asked at once'
      ? 'Two browsers asked at once, so neither was let in. Go back to the page and open it again.'
      : 'Go back to the page you came from and open it from there.');
    return;
  }
  const { code } = await r.json();
  $('code').textContent = `${code.slice(0, 3)} ${code.slice(3)}`;
  $('code').hidden = false;
  say('In the Commonty app, check it shows this code and choose "Allow this browser".');
  for (;;) {
    await new Promise((ok) => setTimeout(ok, 1500));
    const f = await post('/_dd/app/pair/finish', { verifier });
    if (f.status === 202) continue;
    if (f.ok) {
      const { go: to } = await f.json();
      location.replace(to);
      return;
    }
    $('code').hidden = true;
    $('title').textContent = 'Not opened';
    say('This browser was not let in. Go back to the page and open it again.');
    return;
  }
}

go().catch((e) => say(`Could not ask the app: ${e?.message || e}`));
