// Devices, a tab of Settings: where you are signed in, and what holds a key
// for your account. Sessions and when each key was last used come from the
// gate (signins.rs); the devices and passkeys from your signed entry; their
// names from a file only your library key opens, which the box keeps
// without being able to read it.

import init, { file_seal, file_open, entry_without, entry_signed, entry_with_device, qr_svg } from '/_dd/web/dd_web.js';
import { unlock } from './library.js';
import { requestOptions, assertion, post, b64u, u8b64 } from './webauthn.js';
import { me, inApp, appFetch } from './shell.js';

const $ = (id) => document.getElementById(id);
const ICON = {
  browser: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 9h18"/></svg>',
  phone: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><rect x="7" y="3" width="10" height="18" rx="2"/><path d="M11 18h2"/></svg>',
  laptop: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><rect x="4" y="5" width="16" height="11" rx="1.5"/><path d="M2 19h20"/></svg>',
  key: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><circle cx="8" cy="15" r="4"/><path d="m11 12 8-8M16 7l2 2M14 9l2 2"/></svg>',
  lock: '<svg class="lock" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" aria-hidden="true"><rect x="5" y="11" width="14" height="9" rx="1.5"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/></svg>',
  pencil: '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><path d="M4 20h4L19 9l-4-4L4 16z"/></svg>',
};

const secs = (t) => (t < 1e11 ? t : t / 1000);
const day = (t) => new Date(secs(t) * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
function ago(t) {
  const d = Math.max(0, Date.now() / 1000 - secs(t));
  if (d < 120) return 'now';
  if (d < 5400) return `${Math.round(d / 60)} min ago`;
  if (d < 86400) return `${Math.round(d / 3600)} h ago`;
  if (d < 172800) return 'yesterday';
  return day(t);
}
const mobile = (kind) => /Android|iOS/.test(kind || '');

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

let who = null;
let entry = null;
let signins = null;
let lib = null; // the library, once the passkey has been shown
let names = null; // { key: name }, once opened

async function load() {
  who = who || (await me());
  const [d, s] = await Promise.all([
    fetch('/_dd/directory/' + encodeURIComponent(who.user)).then((r) => r.json()),
    fetch('/_dd/signins').then((r) => (r.ok ? r.json() : { sessions: [], keys: {} })),
  ]);
  entry = d;
  signins = s;
}

// the passkey, once more: what ends a session elsewhere asks for it
async function confirm() {
  const start = await post('/_dd/login/start', { username: who.user });
  const { publicKey, ceremony } = await start.json();
  const cred = await navigator.credentials.get({ publicKey: requestOptions(publicKey) });
  await post('/_dd/login/finish', assertion(cred), { 'x-dd-ceremony': ceremony });
}

// The email address's unlock lives at the mail Worker, which no box can
// end: the email line on Profile is its frame, and is asked to end the
// others there. Nothing comes back but whether it did.
function endMailElsewhere() {
  const frame = document.getElementById('email');
  if (!frame?.src || !frame.contentWindow) return;
  frame.contentWindow.postMessage('end-others', new URL(frame.src).origin);
}

async function endSessions(body, button) {
  button.disabled = true;
  const send = () => fetch('/_dd/signins/end', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  try {
    let r = await send();
    if (r.status === 403) {
      await confirm();
      r = await send();
    }
    if (!r.ok) throw new Error(await r.text());
    if (body.others) endMailElsewhere();
    await load();
    render();
  } catch (e) {
    button.disabled = false;
    $('dv-msg').textContent = e.name === 'NotAllowedError' ? 'Your passkey was not used.' : String(e.message || e);
  }
}

function sessionRow(s) {
  const li = el('li', 'dv-item');
  const ico = el('span', 'dv-ico');
  ico.innerHTML = mobile(s.kind) ? ICON.phone : ICON.browser;
  const text = el('div');
  const name = el('div', 'dv-name', s.kind);
  if (s.current) name.append(el('span', 'dv-badge', 'This browser'));
  text.append(name, el('div', 'dv-meta', `active ${ago(s.last)} · signed in ${day(s.first)}`));
  const actions = el('div', 'dv-actions');
  if (!s.current) {
    const b = el('button', 'quiet', 'Sign out');
    b.onclick = () => endSessions({ id: s.id }, b);
    actions.append(b);
  }
  li.append(ico, text, actions);
  return li;
}

// a device or passkey of the entry, as a row
function keyRow(k, n) {
  const li = el('li', 'dv-item');
  const ico = el('span', 'dv-ico');
  ico.innerHTML = k.passkey ? ICON.key : mobile(k.kind) ? ICON.phone : ICON.laptop;
  const text = el('div');
  const name = el('div', 'dv-name');
  const fallback = k.passkey ? (n > 1 ? `Passkey ${n}` : 'Passkey') : `Device ${n}`;
  if (names === null) {
    name.classList.add('locked');
    name.innerHTML = ICON.lock;
    name.append(fallback);
  } else {
    name.append(names[k.key] || fallback);
    const pen = el('button', 'dv-pencil');
    pen.setAttribute('aria-label', 'Rename');
    pen.innerHTML = ICON.pencil;
    pen.onclick = () => rename(name, k, fallback);
    name.append(pen);
  }
  const bits = [k.kind, `added ${day(k.added)}`];
  if (k.last) bits.push(`used ${ago(k.last)}`);
  text.append(name, el('div', 'dv-meta', bits.filter(Boolean).join(' · ')));
  const actions = el('div', 'dv-actions');
  const rm = el('button', 'danger', 'Remove');
  rm.onclick = () => remove(k, names?.[k.key] || fallback);
  // removing is signed by the account's main key: a passkey one here, a
  // device one in the app on the device holding it
  if (!k.root && (rootHere || entry.entry.root.startsWith('webauthn:'))) actions.append(rm);
  li.append(ico, text, actions);
  return li;
}

function keys() {
  const e = entry.entry;
  const used = signins.keys || {};
  const rootPasskey = e.root.startsWith('webauthn:') ? e.root.split(':')[1] : null;
  return [
    ...(e.devices || []).map((d) => ({
      key: d.fingerprint,
      fingerprint: d.fingerprint,
      added: d.added,
      kind: used[d.fingerprint]?.kind,
      last: used[d.fingerprint]?.last,
    })),
    ...(e.passkeys || []).map((p) => ({
      key: `passkey:${p.id}`,
      passkey: p.id,
      added: p.added,
      kind: p.cred?.cred?.backup_state ? 'synced passkey' : 'passkey',
      last: used[`passkey:${p.id}`]?.last,
      root: p.id === rootPasskey,
    })),
  ];
}

function render() {
  const sessions = signins.sessions || [];
  $('dv-sessions').replaceChildren(...sessions.map(sessionRow));
  $('dv-none').hidden = sessions.length > 0;
  $('dv-others').hidden = sessions.filter((s) => !s.current).length === 0;
  let d = 0;
  let p = 0;
  $('dv-keys').replaceChildren(...keys().map((k) => keyRow(k, k.passkey ? ++p : ++d)));
  $('dv-show').hidden = names !== null;
  $('dv-hint').hidden = names !== null;
}

// The names: a file only the library key opens, kept by the box as bytes.
async function openNames() {
  $('dv-show').disabled = true;
  try {
    await init();
    if (!lib) {
      const u = await unlock(who.user);
      if (!u.ok) throw new Error(u.none ? 'names need your library, and this account has none yet' : 'this browser cannot open your library yet');
      lib = u.ok;
    }
    // none yet is a fresh start; a file that will not open is not, or the
    // next rename would write over every name in it
    const r = await fetch('/_dd/devices/names');
    if (r.status === 404) names = {};
    else if (!r.ok) throw new Error(`the box said ${r.status}`);
    else {
      try {
        names = JSON.parse(new TextDecoder().decode(file_open(lib.key, lib.id, new Uint8Array(await r.arrayBuffer()))));
      } catch {
        throw new Error('your device names could not be opened, so they were left as they are');
      }
    }
    render();
  } catch (e) {
    $('dv-msg').textContent = e.name === 'NotAllowedError' ? 'Your passkey was not used.' : String(e.message || e);
  } finally {
    $('dv-show').disabled = false;
  }
}

async function saveNames() {
  const sealed = file_seal(lib.key, lib.id, new TextEncoder().encode(JSON.stringify(names)));
  const r = await fetch('/_dd/devices/names', { method: 'PUT', body: sealed });
  if (!r.ok) throw new Error(`the box said ${r.status}`);
}

function rename(name, k, fallback) {
  const form = el('form', 'dv-edit');
  const input = el('input');
  input.value = names[k.key] || '';
  input.placeholder = fallback;
  input.maxLength = 60;
  input.setAttribute('aria-label', 'Name');
  const save = el('button', '', 'Save');
  const cancel = el('button', 'quiet', 'Cancel');
  cancel.type = 'button';
  cancel.onclick = render;
  form.append(input, save, cancel);
  form.onsubmit = async (e) => {
    e.preventDefault();
    save.disabled = true;
    const was = names[k.key];
    const v = input.value.trim();
    if (v) names[k.key] = v;
    else delete names[k.key];
    try {
      await saveNames();
      render();
    } catch (err) {
      if (was === undefined) delete names[k.key];
      else names[k.key] = was;
      save.disabled = false;
      $('dv-msg').textContent = String(err.message || err);
    }
  };
  name.replaceChildren(form);
  input.focus();
}

// Removing: the account's main key signs the new entry, here when it is a
// passkey; an account whose main key is a device's gets no Remove.
function remove(k, label) {
  $('rm-h').textContent = `Remove ${label}?`;
  $('rm-msg').hidden = true;
  $('rm-yes').disabled = false;
  $('rm-yes').textContent = rootHere ? 'Remove' : 'Remove with passkey';
  $('rm-yes').onclick = async () => {
    $('rm-yes').disabled = true;
    try {
      if (rootHere) {
        const r = await appFetch('/_dd/app/remove', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(k.passkey ? { passkey: k.passkey } : { fingerprint: k.fingerprint }) });
        if (!r.ok) throw new Error(await r.text());
        endMailElsewhere();
        $('remove').close();
        await load();
        render();
        return;
      }
      await init();
      const e = entry.entry;
      const plan = JSON.parse(entry_without(JSON.stringify(entry), k.passkey ? undefined : k.fingerprint, k.passkey || undefined, BigInt(Math.floor(Date.now() / 1000))));
      const cfg = await (await fetch('/_dd/config')).json();
      const rootId = e.root.split(':')[1];
      const a = await navigator.credentials.get({
        publicKey: {
          challenge: b64u(plan.challenge),
          rpId: cfg.rpId,
          allowCredentials: [{ type: 'public-key', id: b64u(rootId) }],
          userVerification: 'preferred',
        },
      });
      const signed = entry_signed(JSON.stringify(plan.entry), JSON.stringify({
        authenticatorData: u8b64(a.response.authenticatorData),
        clientDataJSON: u8b64(a.response.clientDataJSON),
        signature: u8b64(a.response.signature),
      }));
      const r = await fetch('/_dd/directory/' + encodeURIComponent(who.user), { method: 'PUT', headers: { 'content-type': 'application/json' }, body: signed });
      if (!r.ok) throw new Error(await r.text() || `the box said ${r.status}`);
      endMailElsewhere();
      $('remove').close();
      await load();
      render();
    } catch (err) {
      $('rm-msg').textContent = err.name === 'NotAllowedError' ? 'Your passkey was not used.' : String(err.message || err);
      $('rm-msg').hidden = false;
      $('rm-yes').disabled = false;
    }
  };
  $('rm-no').onclick = () => $('remove').close();
  $('remove').showModal();
}

// Adding a device by QR code (box/verify/src/adddevice.rs). The account's
// main key approves: a passkey one here, or a device one in the app on the
// device that holds it.
let rootHere = false;
let adding = null;

async function canAdd() {
  if (entry.entry.root.startsWith('webauthn:')) return true;
  if (!inApp) return false;
  const r = await appFetch('/_dd/app/root').catch(() => null);
  return !!(r && r.ok && (await r.json()).here && (rootHere = true));
}

function adPanel(which) {
  for (const p of ['ad-show', 'ad-ask', 'ad-done']) $(p).hidden = p !== which;
  $('ad-msg').hidden = true;
}

function adSay(text) {
  $('ad-msg').textContent = text;
  $('ad-msg').hidden = false;
}

async function addStart() {
  await init();
  const r = await fetch('/_dd/add/start', { method: 'POST' });
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  adding = await r.json();
  $('ad-qr').innerHTML = qr_svg(adding.url);
  $('ad-code').textContent = adding.code;
  $('ad-where').textContent = new URL(adding.url).host + '/add';
  adPanel('ad-show');
  $('add').showModal();
  addWatch(adding.code);
}

// the pairing sums, the same as identity::pairing_commit and _digits make
const enc = (t) => new TextEncoder().encode(t);
const sha = async (t) => new Uint8Array(await crypto.subtle.digest('SHA-256', enc(t)));
const hex = (b) => [...b].map((x) => x.toString(16).padStart(2, '0')).join('');
async function pairingDigits(pk, newDevice, approver) {
  const d = await sha(`commonty pairing v1\0${pk}\0${newDevice}\0${approver}`);
  return String(new DataView(d.buffer).getUint32(0) % 1000000).padStart(6, '0');
}

// The new device commits to a number of its own; this page then gives it
// one, and only after that does the new device reveal its own. The digits
// come from the key and both numbers, worked out here, so a box that put
// its own key in the new device's place could not make them match.
async function addWatch(code) {
  while (adding && adding.code === code && $('add').open) {
    if (Date.now() / 1000 > adding.expires) {
      adSay('That code ran out. Close this and make a new one.');
      return;
    }
    const s = await fetch('/_dd/add/status?code=' + encodeURIComponent(code)).then((r) => r.json()).catch(() => null);
    if (s?.state === 'gone') return adSay('That code ran out. Close this and make a new one.');
    if (s?.state === 'offered') {
      adding.offer = s.offer;
      if (!adding.mine) {
        adding.mine = hex(crypto.getRandomValues(new Uint8Array(16)));
        await fetch('/_dd/add/theirs', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ code, nonce: adding.mine }) });
      }
      if (s.revealed) {
        const pk = s.offer.public_key;
        if (hex(await sha(`commonty pairing commit v1\0${pk}\0${s.revealed}`)) !== s.offer.commit) {
          return adSay("The new device's answer didn't add up. Close this and make a new code.");
        }
        $('ad-what').textContent = `${s.offer.kind} wants to join`;
        $('ad-digits').textContent = await pairingDigits(pk, s.revealed, adding.mine);
        $('ad-yes').textContent = rootHere ? 'Approve' : 'Approve with passkey';
        adPanel('ad-ask');
        return;
      }
    }
    await new Promise((r) => setTimeout(r, 2000));
  }
}

async function addCancel() {
  if (adding) fetch('/_dd/add/cancel', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ code: adding.code }) }).catch(() => {});
  adding = null;
  $('add').close();
}

async function addApprove() {
  $('ad-yes').disabled = true;
  try {
    const pk = adding.offer.public_key;
    if (rootHere) {
      // the main key is on this device: the app signs it in
      const r = await appFetch('/_dd/app/admit', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ public_key: pk }) });
      if (!r.ok) throw new Error(await r.text());
    } else {
      // a passkey main key: the library is opened to seal its key to the
      // device, then the new entry is signed with the passkey
      if (!lib) {
        const u = await unlock(who.user);
        if (u.ok) lib = u.ok;
      }
      const plan = JSON.parse(entry_with_device(JSON.stringify(entry), pk, lib?.id, lib?.key, BigInt(Math.floor(Date.now() / 1000))));
      const cfg = await (await fetch('/_dd/config')).json();
      const a = await navigator.credentials.get({
        publicKey: {
          challenge: b64u(plan.challenge),
          rpId: cfg.rpId,
          allowCredentials: [{ type: 'public-key', id: b64u(entry.entry.root.split(':')[1]) }],
          userVerification: 'preferred',
        },
      });
      const signed = entry_signed(JSON.stringify(plan.entry), JSON.stringify({
        authenticatorData: u8b64(a.response.authenticatorData),
        clientDataJSON: u8b64(a.response.clientDataJSON),
        signature: u8b64(a.response.signature),
      }));
      const r = await fetch('/_dd/directory/' + encodeURIComponent(who.user), { method: 'PUT', headers: { 'content-type': 'application/json' }, body: signed });
      if (!r.ok) throw new Error(await r.text() || `the box said ${r.status}`);
    }
    fetch('/_dd/add/cancel', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ code: adding.code }) }).catch(() => {});
    adding = null;
    adPanel('ad-done');
    await load();
    render();
  } catch (e) {
    adSay(e.name === 'NotAllowedError' ? 'Your passkey was not used. Nothing changed.' : String(e.message || e));
  } finally {
    $('ad-yes').disabled = false;
  }
}

let started = false;

/// the Devices tab, the first time it is shown
export async function start() {
  if (started) return;
  started = true;
  $('dv-show').onclick = openNames;
  $('dv-others').onclick = () => endSessions({ others: true }, $('dv-others'));
  $('dv-add').onclick = () => addStart().catch((e) => { $('dv-msg').textContent = String(e.message || e); });
  $('ad-cancel').onclick = addCancel;
  $('ad-no').onclick = addCancel;
  $('ad-yes').onclick = addApprove;
  $('ad-close').onclick = () => $('add').close();
  $('add').addEventListener('close', () => { adding = null; });
  try {
    await load();
    render();
    $('dv-add').hidden = !(await canAdd());
    // the main key may be here after all: Remove where it can be done
    if (rootHere) render();
  } catch (e) {
    $('dv-msg').textContent = String(e.message || e);
  }
}
