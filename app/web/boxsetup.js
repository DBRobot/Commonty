// A box's installer drive (app/src/boxsetup.rs): choose a USB drive, say
// which Wi-Fi the box takes, write it, start the box from it, done once it
// has joined. Every step asks the app's own routes under /_dd/app/boxsetup/,
// with the launch secret.

import { $, el } from '/_dd/static/kit.js';

const STEPS = ['choose', 'wifi', 'write', 'start', 'done'];
let at = 0;
let disks = [];
let chosen = null;
let polling = null;
let wifi = null;
let here = null;

const secret = document.querySelector('meta[name="dd-app"]')?.content || '';
const api = (path, init = {}) => fetch('/_dd/app/boxsetup' + path, { ...init, headers: { ...(init.headers || {}), 'x-dd-app': secret } });
async function said(r) {
  return (await r.text()) || `the app said ${r.status}`;
}

function size(b) {
  if (b == null) return '—';
  if (b >= 1e9) return `${(b / 1e9).toFixed(1)} GB`;
  if (b >= 1e6) return `${Math.round(b / 1e6)} MB`;
  return `${Math.round(b / 1e3)} kB`;
}

function left(s) {
  if (s == null) return '—';
  if (s < 60) return 'less than a minute left';
  const m = Math.round(s / 60);
  return m === 1 ? 'about a minute left' : `about ${m} minutes left`;
}

function state(id, kind, words) {
  $(id).dataset.state = kind;
  $(id).textContent = words;
}

function show(i) {
  at = i;
  const step = STEPS[i];
  for (const li of $('hi-steps').children) {
    const n = STEPS.indexOf(li.dataset.step);
    if (n === i) li.setAttribute('aria-current', 'step');
    else li.removeAttribute('aria-current');
    li.classList.toggle('done', n < i);
  }
  for (const s of document.querySelectorAll('[data-pane]')) s.hidden = s.dataset.pane !== step;
  clearInterval(polling);
  polling = null;
  // once written, back would only write it again
  $('hi-back').hidden = i === 0 || ['write', 'start', 'done'].includes(step);
  $('hi-next').hidden = ['start', 'done'].includes(step);
  $('hi-next').textContent = step === 'wifi' ? 'Write the drive' : 'Next';
  $('hi-back').disabled = false;
  if (step === 'choose') choose();
  if (step === 'wifi') wifiStep();
  if (step === 'write') write();
  if (step === 'start') waitForJoin();
}

// 1: removable drives only; the one picked is named in the warning
async function choose() {
  $('hi-next').disabled = !chosen;
  try {
    const r = await api('/disks');
    if (!r.ok) throw new Error(await said(r));
    disks = await r.json();
  } catch (e) {
    $('hi-disks').replaceChildren(el('li', { class: 'hi-empty', text: `The drives in this computer could not be listed: ${e.message || e}` }));
    return;
  }
  if (chosen && !disks.some((d) => d.id === chosen.id)) chosen = null;
  drawDisks();
}

function drawDisks() {
  if (!disks.length) {
    $('hi-disks').replaceChildren(el('li', { class: 'hi-empty', text: 'No USB drive is in this computer. Put one in, then press Look again.' }));
  } else {
    $('hi-disks').replaceChildren(...disks.map((d) => el('li', { class: 'hi-disk' },
      el('label', {},
        el('input', {
          type: 'radio', name: 'disk', value: d.id, checked: chosen?.id === d.id,
          onchange: () => { chosen = d; drawDisks(); },
        }),
        el('span', { class: 'what' },
          el('span', { text: d.model || 'Drive' }),
          el('span', { class: 'meta', text: `${d.name} · ${d.bus}` })),
        el('span', { class: 'size', text: size(d.size_bytes) })))));
  }
  $('hi-warn').hidden = !chosen;
  if (chosen) {
    const mounted = chosen.mounted?.length ? ` It is open as ${chosen.mounted.join(', ')} now.` : '';
    $('hi-warn').textContent = `Everything on ${chosen.model || 'this drive'} (${chosen.name}, ${size(chosen.size_bytes)}) will be erased.${mounted}`;
  }
  $('hi-next').disabled = !chosen;
}

// 2: this computer's network, another, or none
async function wifiStep() {
  if (!here) {
    try { here = await (await api('/wifi')).json(); } catch { here = {}; }
  }
  if (here.ssid) {
    $('hi-wifi-this').hidden = false;
    $('hi-wifi-this-name').textContent = `Use this computer's Wi-Fi (${here.ssid})`;
    $('hi-wifi-this-meta').textContent = here.known
      ? 'its password goes on the drive, and nowhere else'
      : 'your computer may ask you to allow reading its password';
  }
  if (!wifi) wifi = here.ssid ? 'this' : 'none';
  for (const r of document.querySelectorAll('input[name="wifi"]')) {
    r.checked = r.value === wifi;
    r.onchange = () => { wifi = r.value; drawWifi(); };
  }
  drawWifi();
}

function drawWifi() {
  const other = wifi === 'other';
  $('hi-wifi-form').hidden = !other;
  if (other && !$('hi-ssid').value && here?.ssid) $('hi-ssid').value = here.ssid;
  $('hi-next').disabled = other && !wifiTyped();
  $('hi-wifi-note').hidden = true;
}

function wifiTyped() {
  const ssid = $('hi-ssid').value;
  const psk = $('hi-psk').value;
  return ssid.length > 0 && ssid.length <= 32 && psk.length >= 8 && psk.length <= 63;
}
$('hi-ssid').oninput = drawWifi;
$('hi-psk').oninput = drawWifi;

function wifiChoice() {
  if (wifi === 'other') return { use: 'other', ssid: $('hi-ssid').value, psk: $('hi-psk').value };
  return { use: wifi };
}

// 3: the release, the download, the writer, the drive's files
const PHASES = {
  manifest: 'Getting the release',
  download: 'Downloading the installer',
  hash: 'Checking the image (1 of 3)',
  write: 'Writing (2 of 3)',
  verify: 'Reading it back (3 of 3)',
  key: 'Adding your pass',
  written: 'Written',
};

async function write() {
  $('hi-next').disabled = true;
  $('hi-write-error').hidden = true;
  const now = await (await api('/progress')).json();
  if (now.stage !== 'written' && !['manifest', 'download', 'write', 'key'].includes(now.stage)) {
    const r = await api('/write', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ device: chosen.id, wifi: wifiChoice(), flat: $('hi-flat').checked }),
    });
    if (!r.ok) return failed(await said(r));
  }
  // the password is on its way to the drive; the page keeps no copy
  $('hi-psk').value = '';
  polling = setInterval(progress, 500);
  progress();
}

async function progress() {
  let p;
  try { p = await (await api('/progress')).json(); } catch { return; }
  const words = p.stage === 'write' ? PHASES[p.phase] || 'Writing' : PHASES[p.stage] || 'Working';
  $('hi-phase').textContent = p.stage === 'download' && p.version ? `${words} (version ${p.version})` : words;
  const pct = p.total ? Math.min(100, Math.floor((p.done / p.total) * 100)) : 0;
  $('hi-fill').style.width = `${p.stage === 'written' ? 100 : pct}%`;
  $('hi-bar').setAttribute('aria-valuenow', String(pct));
  $('hi-done').textContent = p.total ? `${size(p.done)} of ${size(p.total)}` : '—';
  $('hi-eta').textContent = ['download', 'write'].includes(p.stage) ? left(p.eta) : '—';
  if (p.stage === 'written') {
    clearInterval(polling);
    state('hi-state', 'ok', 'Done');
    $('hi-next').disabled = false;
  } else if (p.stage === 'failed') {
    clearInterval(polling);
    failed(p.error);
  } else {
    state('hi-state', 'working', 'Working');
  }
}

function failed(why) {
  state('hi-state', 'broken', 'Not written');
  $('hi-write-error').textContent = why;
  $('hi-write-error').hidden = false;
  // a Wi-Fi password the computer would not give: back to type it in
  $('hi-back').hidden = false;
}

// 4: the box, once it has joined with the drive's pass, and its backup
const JOIN_WORDS = {
  waiting: ['Waiting for the box to join', 'working', 'Waiting'],
  adding: ['Joined. Adding its old disk\'s backup to your images', 'working', 'Adding'],
  added: ['Joined', 'ok', 'Done'],
  failed: ['Joined, but its backup is not in your images yet', 'attention', 'Needs a look'],
};

async function waitForJoin() {
  const look = async () => {
    let j;
    try {
      const r = await api('/joined');
      if (!r.ok) return;
      j = await r.json();
    } catch { return; }
    if (!j) return;
    const [words, kind, word] = JOIN_WORDS[j.state] || JOIN_WORDS.waiting;
    $('hi-join-words').textContent = words;
    state('hi-join-state', kind, word);
    $('hi-join-error').hidden = !j.error;
    $('hi-join-error').textContent = j.error || '';
    if (j.state === 'added') {
      clearInterval(polling);
      $('hi-name').textContent = j.box;
      $('hi-backup-name').textContent = `from ${j.box}`;
      $('hi-backup-said').hidden = false;
      show(STEPS.indexOf('done'));
    }
  };
  polling = setInterval(look, 10000);
  look();
}

$('hi-next').onclick = () => show(at + 1);
$('hi-back').onclick = () => show(STEPS[at] === 'write' ? STEPS.indexOf('wifi') : Math.max(0, at - 1));
$('hi-rescan').onclick = choose;

show(0);
