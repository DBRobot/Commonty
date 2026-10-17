// The Computers page: your configs and the computers made from them. One
// answer draws all of it (/api/state), the kept copy first (kit.js load).
import { $, el, load } from '/_dd/static/kit.js';

let now = null;
const STATE = '/api/state';
const words = {
  running: ['ok', 'Running'],
  starting: ['working', 'Starting'],
  stopping: ['working', 'Stopping'],
  stopped: ['off', 'Stopped'],
  failed: ['broken', 'Failed'],
};

const gib = (mib) => `${(mib / 1024).toFixed(mib % 1024 ? 1 : 0)} GiB`;

function note(text) {
  const n = $('note');
  n.textContent = text || '';
  n.hidden = !text;
}

async function call(method, url, body) {
  const r = await fetch(url, {
    method,
    headers: body ? { 'content-type': 'application/json' } : {},
    body: body ? JSON.stringify(body) : undefined,
  });
  if (!r.ok) {
    let why = `the box said ${r.status}`;
    try { why = (await r.json()).error || why; } catch { /* not ours */ }
    throw new Error(why);
  }
  return r.json();
}

function refresh() {
  return load(STATE, draw).catch(() => {
    if (!now) note("The box didn't answer. Your computers keep running; try again in a moment.");
  });
}

function draw(s) {
  now = s;
  $('used').textContent = `${gib(s.used.memory)} of ${gib(s.limits.memory)} · ${s.used.threads} of ${s.limits.threads} threads`;
  $('new-config').hidden = !s.member;

  const list = $('instances');
  list.replaceChildren(
    ...(s.instances.length
      ? s.instances.map(instance)
      : [el('p', { class: 'empty', text: s.member
          ? 'No computers yet. Save a config below, then make a computer from it.'
          : 'Computers are for members.' })]),
  );
  const cfgs = $('configs');
  cfgs.replaceChildren(
    ...(s.configs.length
      ? s.configs.map(config)
      : [el('p', { class: 'empty', text: 'A config says how big a computer is, what it has installed, and who may reach it.' })]),
  );
  if (s.instances.some((i) => i.state === 'starting' || i.state === 'stopping')) {
    setTimeout(refresh, 3000);
  }
}

function instance(i) {
  const [tone, word] = words[i.state] || words.stopped;
  const ssh = `ssh -p ${i.address.split(':').pop()} ${i.owner}@${i.address.split(':')[0]}`;
  const applied = i.applied
    ? i.applied.ok
      ? el('span', { class: 'state-word', 'data-state': 'ok', text: 'Packages applied' })
      : el('a', { href: `/api/instances/${i.id}/log`, class: 'state-word', 'data-state': 'broken', text: "Packages didn't apply: the log" })
    : i.state === 'running'
      ? el('span', { class: 'state-word', 'data-state': 'working', text: 'Applying packages' })
      : null;
  const act = (verb, label, opts = {}) => el('button', {
    type: 'button',
    class: opts.quiet ? 'quiet' : null,
    text: label,
    onclick: () => run(i, verb, opts),
  });
  const up = i.state === 'running' || i.state === 'starting';
  return el('article', { class: 'row' },
    el('div', { class: 'what' },
      el('h3', { text: i.name }),
      el('p', { class: 'meta' },
        el('span', { class: 'state-word', 'data-state': tone, text: word }), ' ',
        applied, ' ',
        el('span', { class: 'mono', text: `${gib(i.memory)} · ${i.threads} threads · ${i.disk} GiB` }),
        i.consent === 'any' ? el('span', { class: 'state-word', 'data-state': 'attention', text: "On anyone's box" }) : null),
      el('p', { class: 'meta' },
        el('code', { class: 'mono', text: ssh }),
        i.forwards.length
          ? el('span', { class: 'mono', text: ` · ${i.forwards.map((f) => `${f.guest} on ${f.host}`).join(', ')}` })
          : null)),
    el('div', { class: 'actions' },
      up ? act('stop', 'Stop', { quiet: true }) : act('start', 'Start'),
      act('apply', 'Apply config', { quiet: true }),
      up ? null : act('reset', 'Reset system', { quiet: true, ask: 'Its system goes back to the first disk. /home is kept.' }),
      up ? null : act('delete', 'Delete', { quiet: true, ask: `Delete ${i.name} and both its disks? This can't be undone.` })));
}

async function run(i, verb, { ask } = {}) {
  if (ask && !window.confirm(ask)) return;
  try {
    let yes = false;
    if (verb === 'apply') {
      const c = now.configs.find((x) => x.name === i.config);
      if (c && c.consent === 'any' && i.consent !== 'any') {
        yes = window.confirm("This config runs on anyone's box. Whoever has that box can read this computer's disk and memory. Go on?");
        if (!yes) return;
      }
    }
    if (verb === 'delete') await call('DELETE', `/api/instances/${i.id}`);
    else await call('POST', `/api/instances/${i.id}/${verb}`, { yes });
    note('');
  } catch (e) {
    note(e.message);
  }
  refresh();
}

function config(c) {
  return el('article', { class: 'row' },
    el('div', { class: 'what' },
      el('h3', { text: c.name }),
      el('p', { class: 'meta mono', text: `${gib(c.memory)} · ${c.threads} threads · ${c.disk} GiB · ${c.consent === 'any' ? "anyone's box" : 'my boxes'}` })),
    el('div', { class: 'actions' },
      el('button', { type: 'button', text: 'Make a computer', onclick: () => maker(c) }),
      el('button', { type: 'button', class: 'quiet', text: 'Edit', onclick: () => editor(c) }),
      el('button', { type: 'button', class: 'quiet', text: 'Delete', onclick: async () => {
        if (!window.confirm(`Delete the config ${c.name}? Computers made from it keep running.`)) return;
        try { await call('DELETE', `/api/configs/${encodeURIComponent(c.name)}`); } catch (e) { note(e.message); }
        refresh();
      } })));
}

const DEFAULT_NIX = `{ pkgs, ... }:
{
  # what this computer has, besides the basics; saved, it rebuilds itself
  environment.systemPackages = with pkgs; [ ripgrep nodejs ];
}
`;

function editor(c) {
  const f = $('ed-form');
  const fresh = !c;
  c = c || { name: '', memory: 4096, threads: 2, disk: 32, consent: now.user === now.boxOwner ? 'own' : 'any', keys: [], ports: [], players: [], nix: DEFAULT_NIX };
  f.name.value = c.name;
  f.name.readOnly = !fresh;
  f.memory.value = c.memory;
  f.memory.max = now.limits.memory;
  f.threads.value = c.threads;
  f.threads.max = now.limits.threads;
  f.disk.value = c.disk;
  f.disk.max = now.limits.disk;
  f.consent.value = c.consent;
  f.keys.value = c.keys.join('\n');
  f.ports.value = c.ports.join(' ');
  f.players.value = c.players.join(' ');
  f.nix.value = c.nix;
  $('ed-error').hidden = true;
  $('editor').showModal();
}

$('ed-form').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const f = ev.target;
  const split = (s) => s.split(/[\s,]+/).filter(Boolean);
  const body = {
    name: f.name.value,
    memory: Number(f.memory.value),
    threads: Number(f.threads.value),
    disk: Number(f.disk.value),
    consent: f.consent.value,
    keys: f.keys.value.split('\n').map((k) => k.trim()).filter(Boolean),
    ports: split(f.ports.value).map(Number),
    players: split(f.players.value),
    nix: f.nix.value,
  };
  try {
    await call('PUT', `/api/configs/${encodeURIComponent(body.name)}`, body);
    $('editor').close();
    refresh();
  } catch (e) {
    $('ed-error').textContent = e.message;
    $('ed-error').hidden = false;
  }
});
$('ed-cancel').addEventListener('click', () => $('editor').close());
$('new-config').addEventListener('click', () => editor(null));

let making = null;
function maker(c) {
  making = c;
  const f = $('mk-form');
  f.name.value = '';
  $('mk-yes').checked = false;
  $('mk-warn').hidden = c.consent !== 'any';
  $('mk-error').hidden = true;
  $('mk-title').textContent = `New computer from ${c.name}`;
  $('maker').showModal();
}
$('mk-form').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const yes = $('mk-yes').checked;
  if (making.consent === 'any' && !yes) {
    $('mk-error').textContent = "Say yes to the warning first: it runs on someone else's box.";
    $('mk-error').hidden = false;
    return;
  }
  try {
    await call('POST', '/api/instances', { config: making.name, name: ev.target.name.value, yes });
    $('maker').close();
    refresh();
  } catch (e) {
    $('mk-error').textContent = e.message;
    $('mk-error').hidden = false;
  }
});
$('mk-cancel').addEventListener('click', () => $('maker').close());

refresh();
