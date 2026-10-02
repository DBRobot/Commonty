// Network, a tab of Settings: the house's boxes and how each is connected,
// the Wi-Fi they fall back to, and ad blocking at home. A Wi-Fi change is
// signed by your passkey here and tried on every box, each of which keeps
// its old details if the new ones do not work (box/verify/src/home.rs,
// nix/modules/box/wifi-apply.sh).

import { me, pageConfig, entryOf } from './shell.js';

const $ = (id) => document.getElementById(id);
const BOX = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" aria-hidden="true"><rect x="3" y="4" width="18" height="7" rx="1.5"/><rect x="3" y="13" width="18" height="7" rx="1.5"/><circle cx="7" cy="7.5" r=".9" fill="currentColor"/><circle cx="7" cy="16.5" r=".9" fill="currentColor"/></svg>';
const LABEL = new TextEncoder().encode('commonty wifi v1\0');

const b64u = (a) => btoa(String.fromCharCode(...new Uint8Array(a))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
const unb64u = (s) => Uint8Array.from(atob(s.replace(/-/g, '+').replace(/_/g, '/')), (c) => c.charCodeAt(0));

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function bars(signal) {
  const b = el('span', 'nw-bars');
  for (const [i, h] of [4, 7, 10, 14].entries()) {
    const bar = el('i');
    bar.style.height = `${h}px`;
    if (signal < (i + 1) * 25 - 10) bar.className = 'off';
    b.append(bar);
  }
  return b;
}

let house = null;
let fleet = {}; // by box: its release and backup, from the boxes' metrics

function since(t) {
  const d = Math.max(0, Date.now() / 1000 - t);
  if (d < 5400) return `${Math.max(1, Math.round(d / 60))} min ago`;
  if (d < 172800) return `${Math.round(d / 3600)} h ago`;
  return `${Math.round(d / 86400)} days ago`;
}

// "is it all right": on the latest release, and backed up lately; a
// problem in plain words where there is one
function health(name) {
  const f = fleet[name];
  if (!f || !f.up) return [];
  const newest = Math.max(...Object.values(fleet).filter((x) => x.up && x.release != null).map((x) => x.release));
  const out = [];
  if (f.release != null && f.release < newest) out.push(['Behind on updates', true]);
  else if (f.result && f.result !== 'ok') out.push([`Last update ${f.result}`, true]);
  else out.push(['Up to date', false]);
  const b = f.backup?.last_success;
  if (!b) out.push(['Never backed up', true]);
  else if (Date.now() / 1000 - b > 2 * 86400) out.push([`Not backed up for ${Math.floor((Date.now() / 1000 - b) / 86400)} days`, true]);
  else out.push([`Backed up ${since(b)}`, false]);
  return out;
}

function row(b) {
  const li = el('li', 'nw-row');
  const ico = el('span', 'dv-ico');
  ico.innerHTML = BOX;
  const text = el('div');
  text.append(el('div', 'dv-name', b.box));
  const meta = el('div', 'dv-meta');
  const s = b.status || {};
  const wifi = s.wifi || {};
  const onWifi = wifi.state && wifi.state.startsWith('100');
  if (b.unreachable) {
    meta.textContent = 'Not answering';
  } else if (s.wired) {
    meta.append(onWifi ? 'Cable to the router · Wi-Fi standing by ' : 'Cable to the router');
    if (onWifi && wifi.signal != null) meta.append(bars(wifi.signal), ` ${wifi.signal}%`);
  } else if (onWifi) {
    meta.append('On Wi-Fi ');
    if (wifi.signal != null) meta.append(bars(wifi.signal), ` ${wifi.signal}%`);
  } else {
    meta.textContent = 'No connection to the router';
  }
  text.append(meta);
  const h = health(b.box);
  if (h.length) {
    const line = el('div', 'dv-meta nw-health');
    h.forEach(([words, bad], i) => {
      if (i) line.append(' · ');
      line.append(el('span', bad ? 'bad' : '', words));
    });
    text.append(line);
  }
  const state = el('span', 'nw-state', b.unreachable ? 'Offline' : 'Online');
  if (!b.unreachable) state.classList.add('ok');
  li.append(ico, text, state);
  return li;
}

async function load() {
  const [r, f] = await Promise.all([fetch('/_dd/house'), fetch('/_dd/fleet.json').catch(() => null)]);
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  house = await r.json();
  if (f && f.ok) fleet = Object.fromEntries((await f.json()).map((x) => [x.name, x]));
  $('nw-boxes').replaceChildren(...house.boxes.map(row));
  const ssid = house.boxes.map((b) => b.status?.wifi?.ssid).find(Boolean);
  $('nw-ssid').textContent = ssid || 'Not set';
  if (!$('nw-form').hidden) return;
  $('nw-new-ssid').value = ssid || '';
}

// The change, signed by your passkey over exactly what changes
async function save(e) {
  e.preventDefault();
  $('nw-save').disabled = true;
  $('nw-result').hidden = true;
  try {
    const who = await me();
    const nonce = b64u(crypto.getRandomValues(new Uint8Array(16)));
    const payload = JSON.stringify({
      user: who.user,
      ssid: $('nw-new-ssid').value.trim(),
      psk: $('nw-psk').value,
      at: Math.floor(Date.now() / 1000),
      nonce,
    });
    const body = new Uint8Array(LABEL.length + new TextEncoder().encode(payload).length);
    body.set(LABEL);
    body.set(new TextEncoder().encode(payload), LABEL.length);
    const challenge = new Uint8Array(await crypto.subtle.digest('SHA-256', body));
    const [cfg, entry] = await Promise.all([pageConfig(), entryOf(who.user)]);
    const a = await navigator.credentials.get({
      publicKey: {
        challenge,
        rpId: cfg.rpId,
        allowCredentials: (entry.entry.passkeys || []).map((p) => ({ type: 'public-key', id: unb64u(p.id) })),
        userVerification: 'preferred',
      },
    });
    const r = await fetch('/_dd/house/wifi', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({
        payload,
        id: b64u(a.rawId),
        assertion: {
          authenticatorData: b64u(a.response.authenticatorData),
          clientDataJSON: b64u(a.response.clientDataJSON),
          signature: b64u(a.response.signature),
        },
      }),
    });
    if (!r.ok) throw new Error((await r.text()) || `the box said ${r.status}`);
    $('nw-psk').value = '';
    await watch(nonce);
  } catch (err) {
    say(err.name === 'NotAllowedError' ? 'Your passkey was not used. Nothing changed.' : String(err.message || err), false);
  } finally {
    $('nw-save').disabled = false;
  }
}

function say(text, ok) {
  $('nw-result').textContent = text;
  $('nw-result').className = ok ? 'nw-result ok' : 'nw-result bad';
  $('nw-result').hidden = false;
}

// each box tries the details and says how it went; the ones on Wi-Fi only
// may drop off for a moment while they do
async function watch(nonce) {
  say('Trying the new details on each box…', true);
  const deadline = Date.now() + 120000;
  const wifiBoxes = () => house.boxes.filter((b) => !b.unreachable && b.status?.wifi);
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 3000));
    try { await load(); } catch { continue; }
    const boxes = wifiBoxes();
    const done = boxes.filter((b) => b.last?.nonce === nonce);
    if (done.length === boxes.length) {
      const failed = done.filter((b) => !b.last.ok).map((b) => b.box);
      if (!failed.length) {
        say('Saved. Every box joined with the new details.', true);
        $('nw-form').hidden = true;
        $('nw-view').hidden = false;
        $('nw-change').hidden = false;
      } else {
        say(`The change didn't work on ${failed.join(' and ')}, so ${failed.length > 1 ? 'they are' : 'it is'} back on the old Wi-Fi. Check the network name and password and try again.`, false);
      }
      return;
    }
  }
  say("Not every box answered in time. Those that didn't keep their old Wi-Fi; try again in a minute.", false);
}

// ad blocking, the same switch as the menu's, where this box has it
async function ads() {
  const sw = $('nw-ads');
  const show = (s) => {
    sw.disabled = false;
    sw.setAttribute('aria-checked', String(!!s.on));
    $('nw-ads-state').textContent = s.on ? 'On for every device in the house' : s.resumesIn ? 'Paused for now' : 'Off';
  };
  const r = await fetch('/_dd/adblock/state?brief').catch(() => null);
  if (!r || !r.ok) return;
  $('nw-ads-box').hidden = false;
  show(await r.json());
  sw.onclick = async () => {
    sw.disabled = true;
    const on = sw.getAttribute('aria-checked') !== 'true';
    const w = await fetch('/_dd/adblock/switch', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ on }) });
    if (w.ok) show(await w.json());
    else sw.disabled = false;
  };
}

let started = false;

/// the Network tab, the first time it is shown
export async function start() {
  if (started) return;
  started = true;
  $('nw-change').onclick = () => {
    $('nw-form').hidden = false;
    $('nw-view').hidden = true;
    $('nw-change').hidden = true;
    $('nw-psk').focus();
  };
  $('nw-cancel').onclick = () => {
    $('nw-form').hidden = true;
    $('nw-view').hidden = false;
    $('nw-change').hidden = false;
    $('nw-result').hidden = true;
  };
  $('nw-form').addEventListener('submit', save);
  ads();
  try {
    await load();
  } catch (e) {
    say(String(e.message || e), false);
  }
}
