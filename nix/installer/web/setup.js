// The box installer's steps (dd-box-setup's /api): firmware, network,
// disk, backup, install, done. One step shown at a time; Next moves on
// once the step is satisfied.
'use strict';

const $ = (id) => document.getElementById(id);
const STEPS = ['firmware', 'network', 'disk', 'backup', 'install', 'done'];
let at = 0;
let disk = null;
let backedUp = false;
const ready = {};

async function api(path, body) {
  const r = await fetch('/api/' + path, body === undefined
    ? { cache: 'no-store' }
    : { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) });
  const v = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(v.error || 'it did not answer');
  return v;
}

function show(i) {
  at = i;
  STEPS.forEach((s, n) => {
    document.querySelector(`[data-pane="${s}"]`).hidden = n !== i;
    const li = document.querySelector(`[data-step="${s}"]`);
    if (n === i) li.setAttribute('aria-current', 'step'); else li.removeAttribute('aria-current');
    li.classList.toggle('done', n < i);
  });
  $('back').hidden = i === 0 || i >= 4;
  $('next').hidden = i >= 4;
  $('next').disabled = !ready[STEPS[i]];
  const enter = { firmware, network, disks, backup, install }[STEPS[i]];
  if (enter) enter();
}
function can(step, ok) {
  ready[step] = ok;
  if (STEPS[at] === step) $('next').disabled = !ok;
}
function fail(id, e) {
  $(id).textContent = e.message || String(e);
  $(id).hidden = false;
}
function human(b) {
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (b >= 1000 && i < u.length - 1) { b /= 1000; i++; }
  return `${b.toFixed(i ? 1 : 0)} ${u[i]}`;
}
function clock(s) {
  if (s == null) return '—';
  const m = Math.floor(s / 60);
  return m >= 60 ? `${Math.floor(m / 60)} h ${m % 60} min left` : m ? `${m} min left` : `${s} s left`;
}

// what each check means, and where a person changes it when this can't
const CHECK = {
  uefi: ['Starts by UEFI', 'Set the boot mode to UEFI (not Legacy or CSM) in the firmware settings.'],
  tpm: ['Has a TPM 2.0', 'Turn on the TPM: on Intel it is often called PTT, on AMD fTPM, under Security.'],
  secureBoot: ['Secure Boot is off', 'Turn Secure Boot off, under Boot or Security.'],
  raid: ['Its disks can be seen', 'Set the disk controller (SATA mode) to AHCI, not RAID or RST, under Storage.'],
  virt: ['Can run virtual machines', 'Turn on virtualisation: Intel VT-x or AMD-V (SVM), under CPU or Advanced.'],
};
const WANT = { tpm: 'tpm', virt: 'virt', power: 'power' };

async function firmware() {
  $('fw-error').hidden = true;
  let v;
  try { v = await api('checks'); } catch (e) { return fail('fw-error', e); }
  const c = v.checks;
  const ok = { uefi: c.uefi, tpm: c.tpm, secureBoot: !c.secure_boot, raid: !c.raid, virt: c.virt };
  const fw = v.firmware;
  const changes = new Set(fw.change.map((x) => x.want));
  $('fw-machine').textContent = [v.maker, v.model].filter(Boolean).join(' ') || 'This machine';
  $('fw-checks').replaceChildren(...Object.keys(CHECK).map((k) => {
    const li = document.createElement('li');
    const name = document.createElement('span');
    name.textContent = CHECK[k][0];
    const word = document.createElement('span');
    word.className = 'state-word';
    const fixable = changes.has(WANT[k]);
    word.dataset.state = ok[k] ? 'ok' : fixable ? 'attention' : 'broken';
    word.textContent = ok[k] ? 'Yes' : fixable ? 'Changed here' : 'No';
    li.append(name, word);
    if (!ok[k]) {
      const how = document.createElement('p');
      how.className = 'how';
      how.textContent = fixable ? 'This installer can change it, then restart.' : CHECK[k][1];
      li.append(how);
    }
    return li;
  }));
  $('fw-tip').hidden = fw.already.includes('power') || changes.has('power');
  $('fw-set').hidden = fw.change.length === 0;
  // a box needs UEFI, its TPM, and to see its disks; the rest it can do without
  can('firmware', ok.uefi && ok.tpm && ok.secureBoot && ok.raid);
}
$('fw-again').onclick = firmware;
$('fw-set').onclick = async () => {
  try {
    const r = await api('firmware', {});
    if (r.restart) await api('restart', {});
    else firmware();
  } catch (e) { fail('fw-error', e); }
};

let chosen = null;
async function network() {
  $('net-error').hidden = true;
  let v;
  try { v = await api('network'); } catch (e) { return fail('net-error', e); }
  if (v.wired || v.wifi) {
    $('net-lead').textContent = v.wired ? 'Connected by cable.' : `Connected to ${v.wifi}.`;
    $('net-drive-row').hidden = true;
    $('net-list').hidden = true;
    $('net-form').hidden = true;
    return can('network', true);
  }
  can('network', false);
  if (v.drive) {
    $('net-lead').textContent = 'No cable. The drive carries the Wi-Fi from the computer that made it.';
    $('net-drive-name').textContent = v.drive;
    $('net-drive-row').hidden = false;
  } else {
    $('net-lead').textContent = 'No cable. Pick a Wi-Fi network.';
  }
  scan();
}
async function scan() {
  try {
    const v = await api('wifi');
    $('net-list').replaceChildren(...v.networks.map((n) => {
      const li = document.createElement('li');
      li.className = 'hi-disk';
      const b = document.createElement('label');
      const r = document.createElement('input');
      r.type = 'radio';
      r.name = 'net';
      r.onchange = () => pick(n);
      const what = document.createElement('span');
      what.className = 'what';
      what.textContent = n.ssid;
      const sig = document.createElement('span');
      sig.className = 'size';
      sig.textContent = `${n.signal}%`;
      b.append(r, what, sig);
      li.append(b);
      return li;
    }));
    $('net-list').hidden = false;
  } catch (e) { fail('net-error', e); }
}
function pick(n) {
  chosen = n;
  $('net-ssid').textContent = n.ssid;
  $('net-form').hidden = false;
  $('net-psk').value = '';
  if (n.open) $('net-psk').closest('form').requestSubmit();
  else $('net-psk').focus();
}
$('net-form').onsubmit = async (ev) => {
  ev.preventDefault();
  try {
    await api('wifi', { ssid: chosen.ssid, psk: $('net-psk').value });
    network();
  } catch (e) { fail('net-error', e); }
};
$('net-drive').onclick = async () => {
  try { await api('wifi', { drive: true }); network(); } catch (e) { fail('net-error', e); }
};

async function disks() {
  $('disk-error').hidden = true;
  let v;
  try { v = await api('disks'); } catch (e) { return fail('disk-error', e); }
  if (!v.disks.length) {
    const p = document.createElement('li');
    p.className = 'hi-empty';
    p.textContent = 'No disk to install to. A disk behind RAID (Intel RST) is hidden: set it to AHCI in the firmware settings.';
    return $('disk-list').replaceChildren(p);
  }
  $('disk-list').replaceChildren(...v.disks.map((d) => {
    const li = document.createElement('li');
    li.className = 'hi-disk';
    const l = document.createElement('label');
    const r = document.createElement('input');
    r.type = 'radio';
    r.name = 'disk';
    r.checked = disk && disk.name === d.name;
    r.onchange = () => { disk = d; backedUp = false; can('disk', true); };
    const what = document.createElement('span');
    what.className = 'what';
    const name = document.createElement('span');
    name.textContent = d.model || d.name;
    const meta = document.createElement('span');
    meta.className = 'meta';
    meta.textContent = d.used ? `${d.name} · ${human(d.used)} used` : d.name;
    what.append(name, meta);
    const size = document.createElement('span');
    size.className = 'size';
    size.textContent = human(d.size);
    l.append(r, what, size);
    li.append(l);
    return li;
  }));
  if (v.disks.length === 1 && !disk) { disk = v.disks[0]; $('disk-list').querySelector('input').checked = true; }
  can('disk', !!disk);
}

function backup() {
  $('bk-lead').textContent = disk.used
    ? `${human(disk.used)} is used on ${disk.model || disk.name}.`
    : `Nothing readable was found on ${disk.model || disk.name}.`;
  can('backup', backedUp);
}
$('bk-no').onclick = () => { backedUp = true; show(at + 1); };
$('bk-yes').onclick = async () => {
  $('bk-error').hidden = true;
  try { await api('backup', { disk: disk.name }); } catch (e) { return fail('bk-error', e); }
  $('bk-progress').hidden = false;
  $('bk-yes').disabled = $('bk-no').disabled = true;
  follow('bk', () => { backedUp = true; can('backup', true); $('bk-state').textContent = 'Backed up'; });
};

function install() {
  $('in-disk').textContent = disk.model ? `${disk.model} (${disk.name})` : disk.name;
}
$('in-go').onclick = async () => {
  $('in-error').hidden = true;
  try { await api('install', { disk: disk.name }); } catch (e) { return fail('in-error', e); }
  $('in-go').disabled = true;
  $('in-progress').hidden = false;
  follow('in', () => show(STEPS.indexOf('done')));
};
$('done-restart').onclick = () => api('restart', {}).catch(() => {});

// a running step, every second, until it ends
function follow(p, done) {
  const tick = async () => {
    let j;
    try { j = await api('job'); } catch { return setTimeout(tick, 1000); }
    const pct = Math.round((j.done || 0) * 100);
    $(`${p}-fill`).style.width = `${pct}%`;
    $(`${p}-bar`).setAttribute('aria-valuenow', pct);
    if ($(`${p}-done`)) $(`${p}-done`).textContent = `${pct}%`;
    if ($(`${p}-eta`)) $(`${p}-eta`).textContent = clock(j.eta);
    if (j.stages.length && p === 'in') $('in-phase').textContent = j.stages[j.stages.length - 1];
    if (j.state === 'running') return setTimeout(tick, 1000);
    $(`${p}-state`).dataset.state = j.state === 'done' ? 'ok' : 'broken';
    if (j.state === 'done') return done();
    $(`${p}-state`).textContent = 'Stopped';
    fail(`${p}-error`, new Error(j.said.slice(-2).join(' ') || 'It stopped.'));
    if (p === 'bk') { $('bk-yes').disabled = $('bk-no').disabled = false; }
    else $('in-go').disabled = false;
  };
  tick();
}

$('back').onclick = () => show(Math.max(0, at - 1));
$('next').onclick = () => show(Math.min(STEPS.length - 1, at + 1));
show(0);
