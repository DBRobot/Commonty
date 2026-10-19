// The Remote computers pages: an overview of your computers and configs, a page
// for each computer, a page to edit a config, and a short flow to make a
// computer. One answer draws all of them (/api/state: what the fleet has
// agreed of them, and where each runs), the kept copy first (kit.js load);
// the address after # says which view. A change is an item your passkey
// signs, put at the gate, which the fleet judges: the page shows it once
// it is agreed.
import { $, el, load } from '/_dd/static/kit.js';
import { b64u, u8b64 } from '/_dd/static/webauthn.js';
import { pageConfig, entryOf } from '/_dd/static/shell.js';

const STATE = '/api/state';
let now = null;
let poll = null;

const words = {
  running: ['ok', 'Running'],
  waiting: ['working', 'Waiting for a box'],
  stopped: ['off', 'Stopped'],
};
const stateOf = (i) => (!i.up ? 'stopped' : i.box ? 'running' : 'waiting');
const gib = (mib) => `${(mib / 1024).toFixed(mib % 1024 ? 1 : 0)} GiB`;
const spec = (x) => `${gib(x.memory)} · ${x.threads} ${x.threads === 1 ? 'thread' : 'threads'} · ${x.disk} GiB disk`;
const state = (i) => {
  const [tone, word] = words[stateOf(i)];
  return el('span', { class: 'state-word', 'data-state': tone, text: word });
};
// the config everyone has, by the name people know it by
const label = (n) => (n === 'default' ? 'Default config' : n);
const up = (i) => i.up;
// with a key its config lets in, from a device on the Commonty network
const sshOf = (i) => (i.address ? `ssh -p ${i.port} ${i.owner}@${i.address}` : null);
const now_s = () => Math.floor(Date.now() / 1000);
const enc = (t) => new TextEncoder().encode(t);
const std64 = (bytes) => {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
};

// `body`, signed as `kind` by your account's passkey, put at the gate. The
// passkey answers sha256 of what is signed: the kind, then the body's bytes
async function sign(kind, body) {
  const [cfg, e] = await Promise.all([pageConfig(), entryOf(now.user)]);
  const root = e?.entry?.root || '';
  if (!root.startsWith('webauthn:')) {
    throw new Error("Your account's main key is on one of your devices, not a passkey: make this change from that device.");
  }
  const bytes = enc(JSON.stringify(body));
  const head = enc(`commonty ${kind} v2\0`);
  const msg = new Uint8Array(head.length + bytes.length);
  msg.set(head);
  msg.set(bytes, head.length);
  const a = await navigator.credentials.get({
    publicKey: {
      challenge: new Uint8Array(await crypto.subtle.digest('SHA-256', msg)),
      rpId: cfg.rpId,
      allowCredentials: [{ type: 'public-key', id: b64u(root.split(':')[1]) }],
      userVerification: 'preferred',
    },
  });
  const answer = JSON.stringify({
    authenticatorData: u8b64(a.response.authenticatorData),
    clientDataJSON: u8b64(a.response.clientDataJSON),
    signature: u8b64(a.response.signature),
  });
  const r = await fetch(`/_dd/computers/${kind}`, {
    method: 'PUT',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ v: 2, kind, body: std64(bytes), sig: std64(enc(answer)) }),
  });
  if (!r.ok) throw new Error((await r.text()) || `the gate said ${r.status}`);
}

const version = () => Date.now();
const fromConfig = (c, id, name, anywhere) => ({
  owner: now.user, id, name, config: c.name, memory: c.memory, threads: c.threads, disk: c.disk,
  nix: c.nix || '', ports: c.ports || [], players: c.players || [], keys: c.keys || [],
  consent: c.consent, anywhere, version: version(),
});
const newId = () => [...crypto.getRandomValues(new Uint8Array(8))].map((b) => b.toString(16).padStart(2, '0')).join('');
const power = (i, on) => sign('computer-power', { id: i.id, up: on, at: now_s() });

function note(text) {
  $('note').textContent = text || '';
  $('note').hidden = !text;
}

/// an action that shows its own trouble, and draws again once the fleet
/// has had a moment to agree on it
async function act(fn, done) {
  try {
    await fn();
    note('Sent. It shows here once the fleet has agreed on it.');
    if (done) location.hash = done;
  } catch (e) {
    note(e.name === 'NotAllowedError' ? 'Your passkey was not used.' : e.message);
  }
  refresh();
  setTimeout(refresh, 3000);
}

function copyable(text) {
  const code = el('code', { class: 'mono', text });
  const b = el('button', { type: 'button', class: 'quiet small', text: 'Copy' });
  b.addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(text);
      b.textContent = 'Copied';
      setTimeout(() => { b.textContent = 'Copy'; }, 1500);
    } catch {
      getSelection().selectAllChildren(code);
    }
  });
  return el('div', { class: 'copy' }, code, b);
}

function head(title, crumbs, ...acts) {
  $('title').textContent = title;
  document.title = title === 'Remote computers' ? title : `${title} · Remote computers`;
  // no trail: one arrow back to where this page was opened from
  const back = crumbs.filter(([, href]) => href).pop();
  $('back').replaceChildren(back ? el('a', { href: back[1], class: 'back', 'aria-label': `Back to ${back[0]}`, text: '←' }) : '');
  $('head-acts').replaceChildren(...acts.filter(Boolean));
}

function meter(name, used, limit, fmt) {
  const pct = limit ? Math.min(100, Math.round((used / limit) * 100)) : 0;
  const fill = el('i', { 'data-full': pct >= 100 ? '' : null });
  // through the DOM, not a style attribute: the pages' CSP refuses inline styles
  fill.style.width = `${pct}%`;
  return el('div', { class: 'meter' },
    el('span', { class: 'k', text: name }),
    el('span', { class: 'bar', role: 'meter', 'aria-valuemin': 0, 'aria-valuemax': limit, 'aria-valuenow': used, 'aria-label': name }, fill),
    el('span', { class: 'v mono', text: `${fmt(used)} of ${fmt(limit)}` }));
}

// the configs: yours, and the Default one everyone has until they save their own
function allConfigs() {
  const mine = now.configs || [];
  if (mine.some((c) => c.name === 'default')) return mine;
  return [{ name: 'default', memory: 4096, threads: 2, disk: 32, nix: '', ports: [], players: [], keys: [], consent: 'any', builtIn: true }, ...mine];
}

function tabs(which) {
  const t = (id, name, n) => el('a', {
    href: id === 'computers' ? '#/' : '#/configs',
    class: 'tab',
    'aria-current': which === id ? 'page' : null,
  }, name, el('span', { class: 'count mono', text: String(n) }));
  return el('nav', { class: 'tabs-l', 'aria-label': 'Remote computers or configs' },
    t('computers', 'Remote computers', now.computers.length),
    t('configs', 'Configs', allConfigs().length));
}

const toggle = (i) => (up(i)
  ? el('button', { type: 'button', class: 'quiet', text: 'Stop', onclick: () => act(() => power(i, false)) })
  : el('button', { type: 'button', text: 'Start', onclick: () => act(() => power(i, true)) }));

// the overview: your computers, as cards

function overview() {
  head('Remote computers', [],
    now.member ? el('a', { class: 'btn', href: '#/new', text: 'New remote computer' }) : null);
  const s = now;
  const cards = s.computers.map((i) => el('article', { class: 'card-c' },
    el('div', { class: 'card-top' },
      el('h2', {}, el('a', { href: `#/c/${i.id}`, text: i.name })),
      state(i)),
    el('p', { class: 'mono dim', text: spec(i) }),
    sshOf(i) ? copyable(sshOf(i)) : el('p', { class: 'dim', text: up(i) ? i.waiting || 'Waiting for a box with room' : 'Stopped' }),
    el('div', { class: 'card-foot' },
      i.consent === 'any' ? el('span', { class: 'state-word', 'data-state': 'attention', text: "On anyone's box" }) : el('span', { class: 'dim', text: `From ${label(i.config)}` }),
      toggle(i))));
  $('view').replaceChildren(
    el('section', { class: 'usage panel', 'aria-label': 'What your remote computers take, together' },
      meter('Memory', s.used.memory, s.limits.memory, gib),
      meter('Threads', s.used.threads, s.limits.threads, String),
      meter('Disk', s.used.disk, s.limits.disk, (g) => `${g} GiB`)),
    tabs('computers'),
    cards.length
      ? el('div', { class: 'cards' }, cards)
      : el('div', { class: 'empty-p panel' },
        el('h2', { text: s.member ? 'No remote computers yet' : 'Remote computers are for members' }),
        s.member ? el('p', {}, 'A remote computer is yours to SSH into and install what you need. Make one from the Default config, or write your own: how big it is, what it has, and who can reach it.') : null,
        s.member ? el('a', { class: 'btn', href: '#/new', text: 'Make a remote computer' }) : null));
}

// one computer

function computer(id) {
  const i = now.computers.find((x) => x.id === id);
  if (!i) return missing('That remote computer');
  head(i.name, [['Remote computers', '#/'], [i.name]], state(i), toggle(i));

  const c = allConfigs().find((x) => x.name === i.config);
  const widens = c && c.consent === 'any' && i.consent !== 'any';
  const yesBox = widens ? warning() : null;
  const apply = el('button', {
    type: 'button', text: 'Apply its config',
    onclick: () => {
      if (widens && !yesBox.querySelector('input').checked) { note("Say yes below first: its config lets it run on someone else's box."); return; }
      act(() => sign('computer', fromConfig(c, i.id, i.name, c.consent === 'any')));
    },
    disabled: !c,
  });

  const danger = el('section', { class: 'panel sec danger' },
    el('h2', { text: 'Delete' }),
    el('div', { class: 'row' },
      confirming('Delete', `Delete ${i.name} and both its disks. This can't be undone.`,
        () => act(() => sign('computer-delete', { owner: now.user, id: i.id, at: now_s() }), '#/'))));

  $('view').replaceChildren(el('div', { class: 'split' },
    el('div', { class: 'col' },
      el('section', { class: 'panel sec' },
        el('h2', { text: 'Connect' }),
        sshOf(i)
          ? [el('p', { class: 'dim', text: 'From one of your devices on the Commonty network, with a key its config lets in:' }), copyable(sshOf(i))]
          : el('p', { class: 'dim', text: up(i) ? i.waiting || 'It is waiting for a box with room for it.' : 'Start it to connect.' }),
        i.keys.length
          ? el('p', { class: 'dim small-t', text: `${i.keys.length} SSH ${i.keys.length === 1 ? 'key' : 'keys'} let in.` })
          : el('p', { class: 'dim small-t', text: 'No SSH key is let in yet: add your public key to its config and apply it.' }),
        i.forwards.length && i.address
          ? el('table', { class: 'ports' },
            el('thead', {}, el('tr', {}, el('th', { text: 'On the computer' }), el('th', { text: 'Reach it at' }))),
            el('tbody', {}, i.forwards.map((f) => el('tr', {},
              el('td', { class: 'mono', text: String(f.guest) }),
              el('td', { class: 'mono', text: `${i.address}:${f.host}` })))))
          : null),
      el('section', { class: 'panel sec' },
        el('h2', { text: 'Software' }),
        el('p', { class: 'dim', text: 'It rebuilds itself from its Nix file when you apply it; what came of that, it says on the computer itself.' }),
        el('div', { class: 'row' },
          apply,
          c && !c.builtIn ? el('a', { href: `#/configs/${encodeURIComponent(c.name)}`, text: `Edit ${label(c.name)}` }) : null),
        yesBox,
        el('p', { class: 'dim small-t', text: 'Packages and keys change at once; size and ports at its next start.' })),
      el('section', { class: 'panel sec' },
        el('h2', { text: 'Who can reach it' }),
        el('ul', { class: 'who' },
          el('li', {}, el('strong', { text: 'You' }), el('span', { class: 'dim', text: ' · every device of yours' })),
          i.players.map((p) => el('li', {}, el('strong', { text: p }), el('span', { class: 'dim', text: ' · while you are friends' })))),
        el('p', { class: 'dim small-t', text: 'Nobody else on Commonty or the internet can reach it.' })),
      danger),
    el('aside', { class: 'panel sec facts', 'aria-label': 'About this computer' },
      el('h2', { text: 'Details' }),
      el('dl', {},
        el('dt', { text: 'Memory' }), el('dd', { class: 'mono', text: gib(i.memory) }),
        el('dt', { text: 'Threads' }), el('dd', { class: 'mono', text: String(i.threads) }),
        el('dt', { text: 'Disk' }), el('dd', { class: 'mono', text: `${i.disk} GiB system + ${i.disk} GiB /home` }),
        el('dt', { text: 'Made from' }), el('dd', {}, label(i.config)),
        el('dt', { text: 'Runs on' }), el('dd', { text: i.box || '—' }),
        el('dt', { text: 'Where it may run' }), el('dd', { text: i.consent === 'any' ? "Anyone's box: its owner can read it" : 'Your boxes only' })))));
}

function confirming(name, said, fn, disabled) {
  const wrap = el('div', { class: 'confirm' });
  const ask = el('button', { type: 'button', class: 'quiet', text: name, disabled });
  ask.addEventListener('click', () => {
    wrap.replaceChildren(
      el('p', { text: said }),
      el('div', { class: 'row' },
        el('button', { type: 'button', class: 'bad', text: name, onclick: fn }),
        el('button', { type: 'button', class: 'quiet', text: 'Cancel', onclick: () => wrap.replaceChildren(ask) })));
  });
  wrap.append(ask);
  return wrap;
}

function warning() {
  return el('div', { class: 'warn' },
    el('p', {}, el('strong', { text: "This runs on someone else's box." }),
      ' Whoever has that box in their house can read this remote computer’s disk and memory: your files, and anything you sign in to on it, such as Claude.'),
    el('label', { class: 'pick' }, el('input', { type: 'checkbox', id: 'yes-any' }), "I understand; run it on someone else's box"));
}

// configs

function configs() {
  head('Remote computers', [], now.member ? el('a', { class: 'btn quiet-l', href: '#/configs/new', text: 'New config' }) : null);
  $('view').replaceChildren(
    tabs('configs'),
    el('div', { class: 'cards' }, allConfigs().map((c) => {
      const made = now.computers.filter((i) => i.config === c.name).length;
      return el('article', { class: 'card-c' },
        el('div', { class: 'card-top' },
          el('h2', {}, el('a', { href: `#/configs/${encodeURIComponent(c.name)}`, text: label(c.name) })),
          c.consent === 'any' ? el('span', { class: 'state-word', 'data-state': 'attention', text: "Anyone's box" }) : el('span', { class: 'state-word', 'data-state': 'off', text: 'My boxes' })),
        el('p', { class: 'mono dim', text: spec(c) }),
        el('p', { class: 'dim', text: `${c.ports.length ? `Ports ${c.ports.join(', ')}` : 'SSH only'} · ${c.keys.length} SSH ${c.keys.length === 1 ? 'key' : 'keys'} · ${made} made` }),
        el('div', { class: 'card-foot' },
          el('a', { href: `#/configs/${encodeURIComponent(c.name)}`, text: c.builtIn ? 'Make your own from it' : 'Edit' }),
          el('a', { class: 'btn', href: `#/new/${encodeURIComponent(c.name)}`, text: 'Make one' })));
    })));
}

const DEFAULT_NIX = `{ pkgs, ... }:
{
  # what this computer has, besides the basics (git, curl, vim, htop)
  environment.systemPackages = with pkgs; [ ripgrep nodejs ];
}
`;

function field(id, name, input, hint) {
  return el('div', { class: 'field' }, el('label', { for: id, text: name }), input, hint ? el('p', { class: 'hint', text: hint }) : null);
}

function editor(name) {
  const fresh = name === 'new';
  const c = fresh
    ? { name: '', memory: 4096, threads: 2, disk: 32, consent: 'own', keys: [], ports: [], players: [], nix: DEFAULT_NIX }
    : allConfigs().find((x) => x.name === name);
  if (!c) return missing('That config');
  head(fresh ? 'New config' : label(c.name), [['Remote computers', '#/'], ['Configs', '#/configs'], [fresh ? 'New' : c.name]]);
  const L = now.limits;
  const num = (id, v, min, max, step) => el('input', { id, type: 'number', value: v, min, max, step, required: true });
  const nameSec = fresh
    ? el('section', { class: 'panel sec' },
      field('ed-name', 'Name', el('input', { id: 'ed-name', required: true, pattern: '[a-z0-9][a-z0-9-]{0,31}', autocomplete: 'off', placeholder: 'claude-box' }),
        'Lowercase letters, digits and dashes.'))
    : el('input', { id: 'ed-name', type: 'hidden', value: c.name });
  const size = el('section', { class: 'panel sec' },
    el('h2', { text: 'Size' }),
    el('div', { class: 'three' },
      field('ed-memory', 'Memory, MiB', num('ed-memory', c.memory, 512, L.memory, 256)),
      field('ed-threads', 'Threads', num('ed-threads', c.threads, 1, L.threads, 1)),
      field('ed-disk', 'Disk, GiB', num('ed-disk', c.disk, 8, L.disk, 1))),
    el('p', { class: 'hint', text: `All your remote computers together: up to ${gib(L.memory)}, ${L.threads} threads and ${L.disk} GiB. A disk only grows, never shrinks.` }));
  const access = el('section', { class: 'panel sec' },
    el('h2', { text: 'Access' }),
    field('ed-keys', 'SSH public keys', el('textarea', { id: 'ed-keys', class: 'keys', spellcheck: 'false', placeholder: 'ssh-ed25519 AAAA… you@laptop', text: (c.keys || []).join('\n') }),
      'One to a line, as in a .pub file. The private half stays on your device; only these are let in.'),
    el('div', { class: 'two' },
      field('ed-ports', 'Ports to open besides SSH', el('input', { id: 'ed-ports', inputmode: 'numeric', placeholder: '8080 3000', autocomplete: 'off', value: c.ports.join(' ') }),
        'Each gets a port on the box; the computer page says which.'),
      field('ed-players', 'Friends who may reach it', el('input', { id: 'ed-players', placeholder: 'ann bob', autocomplete: 'off', value: c.players.join(' ') }),
        'Only friends count, and only while you stay friends.')));
  const where = el('section', { class: 'panel sec' },
    el('h2', { text: 'Where it runs' }),
    el('fieldset', { class: 'where' },
      el('legend', { class: 'sr', text: 'Where it runs' }),
      el('label', { class: 'opt-card' }, el('input', { type: 'radio', name: 'consent', value: 'own', checked: c.consent !== 'any' }),
        el('span', {}, el('strong', { text: 'Only on my boxes' }), el('span', { class: 'dim', text: 'Nobody else holds it. If your box goes down, it waits for it.' }))),
      el('label', { class: 'opt-card' }, el('input', { type: 'radio', name: 'consent', value: 'any', checked: c.consent === 'any' }),
        el('span', {}, el('strong', { text: "On anyone's box" }), el('span', { class: 'dim', text: "It can run on, and move to, another member's box. That box's owner can read it." })))));
  const software = el('section', { class: 'panel sec software' },
    el('h2', { text: 'Software' }),
    field('ed-nix', 'A NixOS module', el('textarea', { id: 'ed-nix', spellcheck: 'false', text: c.nix || '' }),
      'Saved and applied, the computer rebuilds itself from this. /home is kept. Never put a password, token or key here: every box on the fleet keeps a copy of configs. Put secrets in /home on the computer.'));
  // settings down the left, the Nix file taking the rest
  const f = el('form', { class: 'editor', id: 'ed' },
    el('div', { class: 'ed-left' }, nameSec, size, access, where),
    software,
    el('div', { class: 'savebar' },
      el('a', { href: '#/configs', text: 'Cancel' }),
      el('button', { type: 'submit', text: fresh ? 'Save config' : 'Save' })));
  f.addEventListener('submit', (ev) => {
    ev.preventDefault();
    const split = (t) => t.split(/[\s,]+/).filter(Boolean);
    const body = {
      owner: now.user,
      name: $('ed-name').value,
      memory: Number($('ed-memory').value),
      threads: Number($('ed-threads').value),
      disk: Number($('ed-disk').value),
      nix: $('ed-nix').value,
      ports: split($('ed-ports').value).map(Number),
      players: split($('ed-players').value),
      keys: $('ed-keys').value.split('\n').map((k) => k.trim()).filter(Boolean),
      consent: f.querySelector('input[name=consent]:checked').value,
      version: version(),
    };
    act(() => sign('computer-config', body), '#/configs');
  });
  const del = fresh || c.builtIn ? null : el('section', { class: 'panel sec danger' },
    el('h2', { text: 'Delete this config' }),
    el('p', { class: 'dim', text: 'Remote computers made from it keep running; they just have nothing to apply.' }),
    confirming('Delete config', `Delete the config ${c.name}?`, () => act(() => sign('computer-config', {
      owner: now.user, name: c.name, memory: c.memory, threads: c.threads, disk: c.disk, nix: c.nix,
      ports: c.ports, players: c.players, keys: c.keys, consent: c.consent, version: version(), gone: true,
    }), '#/configs')));
  $('view').replaceChildren(f, del || '');
}

// making one

function maker(pick) {
  head('New remote computer', [['Remote computers', '#/'], ['New']]);
  const list0 = allConfigs();
  let chosen = list0.find((c) => c.name === pick) || list0[0];
  const warnSlot = el('div');
  const showWarn = () => {
    warnSlot.replaceChildren(chosen.consent === 'any' ? warning() : '');
  };
  const list = el('div', { class: 'pick-list' }, list0.map((c) => {
    const r = el('input', { type: 'radio', name: 'cfg', value: c.name, checked: c === chosen });
    r.addEventListener('change', () => { chosen = c; showWarn(); });
    return el('label', { class: 'opt-card' }, r,
      el('span', {}, el('strong', { text: label(c.name) }), el('span', { class: 'mono dim', text: spec(c) }),
        el('span', { class: 'dim', text: c.consent === 'any' ? "Anyone's box" : 'My boxes only' })));
  }));
  showWarn();
  const f = el('form', { class: 'editor' },
    el('section', { class: 'panel sec' }, el('h2', { text: 'From which config' }), list,
      el('p', { class: 'hint' }, 'Or ', el('a', { href: '#/configs/new', text: 'write a new config' }), '.')),
    el('section', { class: 'panel sec' }, el('h2', { text: 'Its name' }),
      field('mk-name', 'Name', el('input', { id: 'mk-name', required: true, pattern: '[a-z0-9][a-z0-9-]{0,31}', autocomplete: 'off', placeholder: 'dev' }), 'Lowercase letters, digits and dashes.'),
      field('mk-keys', 'SSH public keys', el('textarea', { id: 'mk-keys', class: 'keys', spellcheck: 'false', placeholder: 'ssh-ed25519 AAAA… you@laptop' }),
        "One to a line, as in a .pub file, besides its config's. The private half stays on your device."),
      warnSlot),
    el('div', { class: 'savebar' }, el('a', { href: '#/', text: 'Cancel' }), el('button', { type: 'submit', text: 'Make it' })));
  f.addEventListener('submit', (ev) => {
    ev.preventDefault();
    const yes = chosen.consent === 'any' && !!$('yes-any')?.checked;
    if (chosen.consent === 'any' && !yes) { note("Say yes to the warning first: it runs on someone else's box."); return; }
    const id = newId();
    const extra = $('mk-keys').value.split('\n').map((k) => k.trim()).filter(Boolean);
    const keys = [...new Set([...(chosen.keys || []), ...extra])];
    act(() => sign('computer', fromConfig({ ...chosen, keys }, id, $('mk-name').value, yes)), `#/c/${id}`);
  });
  $('view').replaceChildren(f);
}

function missing(what) {
  head('Not here', [['Remote computers', '#/'], ['Not here']]);
  $('view').replaceChildren(el('div', { class: 'empty-p panel' },
    el('h2', { text: `${what} isn't here yet` }),
    el('p', {}, 'Something just made shows once the fleet has agreed on it; something deleted is gone. ', el('a', { href: '#/', text: 'Back to your remote computers' }), '.')));
}

// the address says which

function route() {
  if (!now) return;
  const [, a, b] = (location.hash || '#/').slice(1).split('/');
  const name = b && decodeURIComponent(b);
  if (a === 'c') computer(b);
  else if (a === 'configs' && name) editor(name);
  else if (a === 'configs') configs();
  else if (a === 'new') maker(name);
  else overview();
  const busy = now.computers.some((i) => stateOf(i) === 'waiting');
  clearTimeout(poll);
  if (busy) poll = setTimeout(refresh, 5000);
}

function draw(s) {
  now = s;
  // a form being filled in is not drawn over by a refresh
  if (document.activeElement && document.activeElement.closest?.('form.editor')) return;
  route();
}

function refresh() {
  return load(STATE, draw).catch(() => {
    if (!now) note("The box didn't answer. Your computers keep running; try again in a moment.");
  });
}

addEventListener('hashchange', () => { note(''); route(); scrollTo(0, 0); });
refresh();
