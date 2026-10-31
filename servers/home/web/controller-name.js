// Naming a home controller with your passkey (controller-name.html): the
// app asks the gate for a name for its controller's key, with the member's
// device token, then opens this page at #<label> when the account's root is
// a passkey. The passkey signs the name for the key the app asked, read
// from the gate, never one a link hands this page
// (identity::controller_name_message), the answer is left at the gate for
// the app to collect, and the app hands it to the controller
// (docs/dev/home-controller.md, "The root's word, either kind of root").

import { b64u, u8b64 } from './webauthn.js';
import { readEntry } from './kit.js';

const $ = (id) => document.getElementById(id);
const show = (id) => {
  for (const s of document.querySelectorAll('main > section')) s.hidden = s.id !== id;
};
const broken = (why) => {
  if (why) $('broken-why').textContent = why;
  show('broken');
};

const label = location.hash.slice(1);

async function start() {
  if (!/^[a-z2-7]{16}$/.test(label)) return broken();
  const [r, cfg] = await Promise.all([fetch('/_dd/me'), fetch('/_dd/config').then((c) => c.json())]);
  if (r.status === 401) {
    location.href = `/_dd/login?rd=${encodeURIComponent(location.pathname + location.hash)}`;
    return;
  }
  // the passkey signs for the front door's address alone
  if (location.hostname !== `home.${cfg.rpId}`) {
    location.replace(`https://home.${cfg.rpId}${location.pathname}${location.hash}`);
    return;
  }
  const me = r.ok ? await r.json() : null;
  if (!me || me.role !== 'member') return broken('Only a member of Commonty can name a home controller.');
  const name = `${label}.home.${cfg.rpId}`;
  $('name').textContent = name;
  const e = await fetch(`/_dd/directory/${encodeURIComponent(me.user)}`);
  if (!e.ok) throw new Error(`the box said ${e.status}`);
  const { entry } = readEntry(await e.text());
  if (!entry.root.startsWith('webauthn:')) {
    return broken('Your account is held by a device, not a passkey: the Commonty app names the controller by itself.');
  }
  const root = entry.root.slice('webauthn:'.length).split(':')[0];
  // what your own app asked, and nothing else
  const ask = await fetch(`/_dd/controller-name/${label}/ask`, { cache: 'no-store' });
  if (ask.status === 404) return broken('Nothing is waiting to be named here. Start again from the Commonty app.');
  if (!ask.ok) throw new Error(`the box said ${ask.status}`);
  const { controller } = await ask.json();
  $('sign').disabled = false;
  $('sign').addEventListener('click', () => sign(cfg.rpId, name, root, controller));
}

async function sign(rpId, name, root, controller) {
  $('sign').disabled = true;
  $('said').hidden = true;
  try {
    const message = new TextEncoder().encode(`commonty controller name v1\0${name}\0${controller}`);
    const challenge = new Uint8Array(await crypto.subtle.digest('SHA-256', message));
    const a = await navigator.credentials.get({
      publicKey: {
        challenge,
        rpId,
        allowCredentials: [{ type: 'public-key', id: b64u(root) }],
        userVerification: 'preferred',
        timeout: 60000,
      },
    });
    // as an entry's own passkey signature: base64 of the assertion's JSON
    const sig = btoa(JSON.stringify({
      authenticatorData: u8b64(a.response.authenticatorData),
      clientDataJSON: u8b64(a.response.clientDataJSON),
      signature: u8b64(a.response.signature),
    }));
    const r = await fetch(`/_dd/controller-name/${label}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ sig }),
    });
    if (!r.ok) throw new Error((await r.text()) || `the box said ${r.status}`);
    show('done');
  } catch (err) {
    $('said').textContent = err.name === 'NotAllowedError'
      ? 'Your passkey was not used. Nothing was named.'
      : `Not named: ${err.message || err}. Try again.`;
    $('said').hidden = false;
    $('sign').disabled = false;
  }
}

start().catch((err) => broken(`Something went wrong: ${err.message || err}. Try again from the Commonty app.`));
