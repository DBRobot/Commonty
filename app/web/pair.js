// The browser's way into what the app serves it (src/local.rs,
// src/handoff.rs). This page's address is no secret, so the proof is made
// here: a random verifier that never leaves this tab until the member has
// let it in, and only its hash goes to the app. The app's window and this
// page show the same code from that hash; once the member allows it there,
// the verifier buys this browser its own session.
const $ = (id) => document.getElementById(id);
const say = (t) => { $('msg').textContent = t; };
const hex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, '0')).join('');

// every service pairs here, on the site's page; which one is in the address
const params = new URLSearchParams(location.search);
const service = params.get('service') || 'site';
const post = (path, body) =>
  fetch(path, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ service, ...body }) });

// A browser let in before keeps this device's name for it here: the app's
// pages are there, and its cookie, so it goes straight on with nothing
// asked. Only this page's own origin can read it, and it is in no address
// the app opens. ?again: the app did not know this browser there any more.
const KEPT = 'dd-own';
const kept = () => { try { return localStorage.getItem(KEPT); } catch { return null; } };
const keep = (v) => { try { if (v) localStorage.setItem(KEPT, v); else localStorage.removeItem(KEPT); } catch { /* asked each time */ } };

async function known() {
  if (params.has('again')) { keep(null); return false; }
  const host = kept();
  if (!host) return false;
  const r = await post('/_dd/app/pair/known', { host }).catch(() => null);
  if (r?.ok && (await r.json()).known) {
    location.replace(`http://${host}/_dd/app/resume?service=${encodeURIComponent(service)}`);
    return true;
  }
  keep(null);
  return false;
}

async function go() {
  if (await known()) return;
  $('title').textContent = 'Link this browser';
  say('Getting a code…');
  const verifier = hex(crypto.getRandomValues(new Uint8Array(32)));
  const hash = hex(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(verifier)));
  const r = await post('/_dd/app/pair/ask', { hash });
  if (!r.ok) {
    const { why } = await r.json().catch(() => ({}));
    $('title').textContent = 'Not linked';
    say(why === 'two browsers asked at once'
      ? 'Two browsers asked at once, so neither was linked. Open the page again.'
      : 'Open the page again from the site.');
    return;
  }
  const { code } = await r.json();
  $('code').textContent = `${code.slice(0, 3)} ${code.slice(3)}`;
  $('code').hidden = false;
  say('The Commonty app shows the same code. Choose "Link this browser" there.');
  for (;;) {
    await new Promise((ok) => setTimeout(ok, 1500));
    const f = await post('/_dd/app/pair/finish', { verifier });
    if (f.status === 202) continue;
    if (f.ok) {
      const { go: to } = await f.json();
      keep(new URL(to).host);
      location.replace(to);
      return;
    }
    $('code').hidden = true;
    $('title').textContent = 'Not linked';
    say('This browser was not linked. Open the page again.');
    return;
  }
}

go().catch((e) => say(`Could not ask the app: ${e?.message || e}`));
