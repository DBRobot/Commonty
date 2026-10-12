// The home controller's installer (app/src/installer.rs): choose a card,
// write it, start the Pi, pair it, done. Every step asks the app's own
// routes under /_dd/app/installer/, with the launch secret.

import { $, el } from '/_dd/static/kit.js';

const STEPS = ['choose', 'write', 'start', 'pair', 'done'];
let at = 0;
let disks = [];
let chosen = null;
let polling = null;
let paired = null;
let started = 0;
// 'write': the image and a pairing key; 'key': a card written another way,
// which gets only a pairing key
let mode = 'write';
let backups = null;
let restoreFrom = null;
let pairPoll = null;

// the app's routes answer a page that carries this launch's secret
const secret = document.querySelector('meta[name="dd-app"]')?.content || '';
const api = (path, init = {}) => fetch('/_dd/app/installer' + path, { ...init, headers: { ...(init.headers || {}), 'x-dd-app': secret } });
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
  $('hi-back').hidden = i === 0 || step === 'done';
  $('hi-next').hidden = step === 'done';
  $('hi-next').textContent = step === 'pair' ? 'Pair' : 'Next';
  $('hi-back').disabled = false;
  if (step === 'choose') choose();
  if (step === 'write') write();
  if (step === 'start') find();
  if (step === 'pair') $('hi-next').disabled = false;
}

// 1: removable disks only; the one picked is named in the warning
async function choose() {
  $('hi-next').disabled = !chosen;
  try {
    const r = await api('/disks');
    if (!r.ok) throw new Error(await said(r));
    disks = await r.json();
  } catch (e) {
    $('hi-disks').replaceChildren(el('li', { class: 'hi-empty', text: `The cards in this computer could not be listed: ${e.message || e}` }));
    return;
  }
  if (chosen && !disks.some((d) => d.id === chosen.id)) chosen = null;
  drawDisks();
}

function drawDisks() {
  if (!disks.length) {
    $('hi-disks').replaceChildren(el('li', { class: 'hi-empty', text: 'No card or USB disk is in this computer. Put the card in, then press Look again.' }));
  } else {
    $('hi-disks').replaceChildren(...disks.map((d) => el('li', { class: 'hi-disk' },
      el('label', {},
        el('input', {
          type: 'radio', name: 'disk', value: d.id, checked: chosen?.id === d.id,
          onchange: () => { chosen = d; drawDisks(); },
        }),
        el('span', { class: 'what' },
          el('span', { text: d.model || 'Card' }),
          el('span', { class: 'meta', text: `${d.name} · ${d.bus}` })),
        el('span', { class: 'size', text: size(d.size_bytes) })))));
  }
  $('hi-warn').hidden = !chosen;
  if (chosen) {
    const mounted = chosen.mounted?.length ? ` It is open as ${chosen.mounted.join(', ')} now.` : '';
    $('hi-warn').textContent = mode === 'key'
      ? `Only a pairing key is added to ${chosen.model || 'this card'} (${chosen.name}, ${size(chosen.size_bytes)}); nothing on it is erased.`
      : `Everything on ${chosen.model || 'this card'} (${chosen.name}, ${size(chosen.size_bytes)}) will be erased.${mounted}`;
  }
  $('hi-next').disabled = !chosen;
  if (chosen) restoreChoices();
}

// a backup to start the new card from, when the member has controllers
async function restoreChoices() {
  if (!backups) {
    try {
      const r = await api('/restore');
      backups = r.ok ? await r.json() : [];
    } catch { backups = []; }
  }
  $('hi-restore').hidden = !backups.length;
  const pick = (label) => el('input', {
    type: 'radio', name: 'restore', checked: restoreFrom === label,
    onchange: () => { restoreFrom = label; },
  });
  $('hi-backups').replaceChildren(
    el('li', { class: 'hi-disk' }, el('label', {}, pick(null), el('span', { class: 'what' }, el('span', { text: 'Start fresh' }), el('span', { class: 'meta', text: 'nothing is put back' })))),
    ...backups.map((b) => {
      const input = pick(b.label);
      input.disabled = !b.backup;
      return el('li', { class: 'hi-disk' }, el('label', {}, input,
        el('span', { class: 'what' },
          el('span', { text: `Restore ${b.name}` }),
          el('span', { class: 'meta', text: b.backup ? 'its last backup' : 'no backup of it yet' }))));
    }));
}

// 2: the release, the download, the writer, the key
const PHASES = {
  manifest: 'Getting the release',
  download: 'Downloading the card',
  hash: 'Checking the image (1 of 3)',
  write: 'Writing (2 of 3)',
  verify: 'Reading it back (3 of 3)',
  key: 'Adding the pairing key',
  written: 'Written',
};

async function write() {
  $('hi-next').disabled = true;
  $('hi-back').disabled = true;
  $('hi-write-error').hidden = true;
  $('hi-key-again').hidden = true;
  if (mode === 'key') return keyOnly();
  $('hi-write-lead').textContent = "The card is checked against this release's signature before anything is written.";
  const now = await (await api('/progress')).json();
  if (now.stage !== 'written' && !['manifest', 'download', 'write', 'key'].includes(now.stage)) {
    const r = await api('/write', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ device: chosen.id }) });
    if (!r.ok) return failed(await said(r), false);
  }
  polling = setInterval(progress, 500);
  progress();
}

// a card written another way: its pairing key alone, no image, no admin rights
async function keyOnly() {
  $('hi-write-lead').textContent = 'Nothing on the card is erased: it gets the key this app pairs with, and nothing else.';
  $('hi-phase').textContent = PHASES.key;
  $('hi-fill').style.width = '0%';
  $('hi-done').textContent = '—';
  $('hi-eta').textContent = '—';
  state('hi-state', 'working', 'Working');
  const r = await api('/key-only', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ device: chosen.id }) });
  if (!r.ok) return failed(await said(r), true);
  $('hi-phase').textContent = 'Pairing key added';
  $('hi-fill').style.width = '100%';
  state('hi-state', 'ok', 'Done');
  $('hi-next').disabled = false;
  $('hi-back').disabled = false;
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
    $('hi-back').disabled = false;
  } else if (p.stage === 'failed') {
    clearInterval(polling);
    failed(p.error, p.phase === 'key');
  } else {
    state('hi-state', 'working', 'Working');
  }
}

function failed(why, keyOnly) {
  state('hi-state', 'broken', 'Not written');
  $('hi-write-error').textContent = why;
  $('hi-write-error').hidden = false;
  $('hi-back').disabled = false;
  // the card is written and only the key is missing: just the key again
  $('hi-key-again').hidden = !keyOnly;
}

function state(id, kind, words) {
  $(id).dataset.state = kind;
  $(id).textContent = words;
}

// 3: the Pi on the house network, until its door answers
async function find() {
  $('hi-next').disabled = true;
  started = started || Date.now();
  const look = async () => {
    let f;
    try { f = await (await api('/find')).json(); } catch { return; }
    if (f.found && f.status?.pairing) {
      clearInterval(polling);
      state('hi-find-state', 'ok', 'Found');
      $('hi-find-words').textContent = 'Your home controller is on and waiting to be paired';
      $('hi-next').disabled = false;
    } else if (f.found && f.status?.needs_key) {
      state('hi-find-state', 'attention', 'Needs its key');
      $('hi-find-words').textContent = 'This controller needs its pairing key: add it from this app. Go back, choose its card, and pick "My card was written another way".';
    } else if (f.found && f.status?.finished) {
      clearInterval(polling);
      state('hi-find-state', 'attention', 'Already paired');
      $('hi-find-words').textContent = 'A home controller is here, but it is paired already';
    } else {
      $('hi-find-help').hidden = Date.now() - started < 3 * 60 * 1000;
    }
  };
  polling = setInterval(look, 3000);
  look();
}

// 4: pairing, all of it in the app; the page follows it as it goes
const PAIR_WORDS = {
  pair: 'Pairing with your home controller',
  passkey: 'Waiting for your passkey in the browser',
  download: 'Downloading the backup',
  upload: 'Putting the backup on your home controller',
  finish: 'Signing its name and letting it onto your network',
};

async function pairProgress() {
  let p;
  try { p = await (await api('/pair/progress')).json(); } catch { return; }
  if (!PAIR_WORDS[p.stage]) return;
  $('hi-pair-words').textContent = PAIR_WORDS[p.stage];
  $('hi-passkey').hidden = p.stage !== 'passkey';
  const moving = p.stage === 'download' || p.stage === 'upload';
  $('hi-pair-moving').hidden = !moving;
  if (!moving) return;
  const pct = p.total ? Math.min(100, Math.floor((p.done / p.total) * 100)) : 0;
  $('hi-pair-fill').style.width = `${pct}%`;
  $('hi-pair-bar').setAttribute('aria-valuenow', String(pct));
  $('hi-pair-done').textContent = p.total ? `${size(p.done)} of ${size(p.total)}` : size(p.done);
  $('hi-pair-eta').textContent = left(p.eta);
}

function pairFailed(e, words) {
  state('hi-pair-state', 'broken', 'Not paired');
  $('hi-pair-words').textContent = words;
  $('hi-pair-error').textContent = String(e.message || e);
  $('hi-pair-error').hidden = false;
}

async function pair() {
  $('hi-next').hidden = false;
  $('hi-next').disabled = true;
  $('hi-back').disabled = true;
  $('hi-pair-error').hidden = true;
  state('hi-pair-state', 'working', 'Pairing');
  $('hi-pair-words').textContent = PAIR_WORDS.pair;
  pairPoll = setInterval(pairProgress, 500);
  try {
    const c = await api('/restore', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ label: restoreFrom }) });
    if (!c.ok) throw new Error(await said(c));
    const r = await api('/pair', { method: 'POST' });
    if (!r.ok) throw new Error(await said(r));
    paired = await r.json();
    $('hi-name').textContent = paired.name;
    show(STEPS.indexOf('done'));
  } catch (e) {
    pairFailed(e, 'Pairing did not finish');
    $('hi-next').disabled = false;
    $('hi-back').disabled = false;
  } finally {
    clearInterval(pairPoll);
    $('hi-passkey').hidden = true;
    $('hi-pair-moving').hidden = true;
  }
}

$('hi-next').onclick = () => {
  if (STEPS[at] === 'pair') pair();
  else show(at + 1);
};
$('hi-back').onclick = () => show(Math.max(0, at - 1));
$('hi-rescan').onclick = choose;
$('hi-have').onclick = () => {
  mode = mode === 'key' ? 'write' : 'key';
  $('hi-have').textContent = mode === 'key' ? 'Write a new card instead' : 'My card was written another way';
  $('hi-choose-lead').textContent = mode === 'key'
    ? 'Put the card that is in your Raspberry Pi into this computer, and pick it here. Only a pairing key is added to it.'
    : "The card goes into the Raspberry Pi. Put it in this computer's card reader, or a USB reader, and pick it here.";
  drawDisks();
};
$('hi-key-again').onclick = async () => {
  if (mode === 'key') return write();
  $('hi-key-again').disabled = true;
  const r = await api('/key', { method: 'POST' });
  $('hi-key-again').disabled = false;
  if (r.ok) {
    $('hi-write-error').hidden = true;
    $('hi-key-again').hidden = true;
    state('hi-state', 'ok', 'Done');
    $('hi-next').disabled = false;
  } else {
    $('hi-write-error').textContent = await said(r);
  }
};
$('hi-open').onclick = async () => {
  $('hi-open').disabled = true;
  const r = await api('/done', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ name: paired.name }) });
  if (!r.ok) {
    $('hi-open-error').textContent = await said(r);
    $('hi-open-error').hidden = false;
    $('hi-open').disabled = false;
  }
};

// Before the first step: a main key on another device cannot sign the
// controller's name from here, so nothing is written that could not then be
// paired; a passkey signs it in the browser when pairing comes to it
(async () => {
  show(0);
  try {
    const s = await (await api('/start')).json();
    if (s.root === 'passkey') {
      $('hi-root').textContent = "Your account's main key is a passkey. When pairing, your browser will ask for it to sign your home controller's name.";
      $('hi-root').hidden = false;
    } else if (s.root === 'elsewhere') {
      $('hi-root').textContent = "Your account's main key is on another device, so this app cannot sign your home controller's name. Set it up from the device that holds the main key; nothing will be written here.";
      $('hi-root').hidden = false;
      $('hi-disks').hidden = true;
      $('hi-rescan').hidden = true;
      $('hi-have').hidden = true;
      $('hi-next').disabled = true;
      $('hi-next').onclick = null;
    }
  } catch { /* the steps say what goes wrong as they go */ }
})();
