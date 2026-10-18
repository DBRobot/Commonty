// The Remote computers pages: an overview of your computers and configs, a page
// for each computer, a page to edit a config, and a short flow to make a
// computer. One answer draws all of them (/api/state), the kept copy first
// (kit.js load); the address after # says which view.
import { $, el, load } from '/_dd/static/kit.js';

const STATE = '/api/state';
let now = null;
let poll = null;

const words = {
  running: ['ok', 'Running'],
  starting: ['working', 'Starting'],
  stopping: ['working', 'Stopping'],
  stopped: ['off', 'Stopped'],
  failed: ['broken', 'Failed'],
};
const gib = (mib) => `${(mib / 1024).toFixed(mib % 1024 ? 1 : 0)} GiB`;
const spec = (x) => `${gib(x.memory)} · ${x.threads} ${x.threads === 1 ? 'thread' : 'threads'} · ${x.disk} GiB disk`;
const state = (s) => {
  const [tone, word] = words[s] || words.stopped;
  return el('span', { class: 'state-word', 'data-state': tone, text: word });
};
const up = (i) => i.state === 'running' || i.state === 'starting';
// with its own key, saved where the download and dd put it
const sshOf = (i) => {
  const [host, port] = i.address.split(':');
  return `ssh -i ~/.ssh/commonty-${i.name} -p ${port} ${i.owner}@${host}`;
};

function note(text) {
  $('note').textContent = text || '';
  $('note').hidden = !text;
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

/// an action that shows its own trouble, and draws again after
async function act(fn, done) {
  try {
    await fn();
    note('');
    if (done) location.hash = done;
  } catch (e) {
    note(e.message);
  }
  refresh();
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
  const up = crumbs.filter(([, href]) => href).pop();
  $('back').replaceChildren(up ? el('a', { href: up[1], class: 'back', 'aria-label': `Back to ${up[0]}`, text: '←' }) : '');
  $('head-acts').replaceChildren(...acts);
}

function meter(label, used, limit, fmt) {
  const pct = limit ? Math.min(100, Math.round((used / limit) * 100)) : 0;
  const fill = el('i', { 'data-full': pct >= 100 ? '' : null });
  // through the DOM, not a style attribute: the pages' CSP refuses inline styles
  fill.style.width = `${pct}%`;
  return el('div', { class: 'meter' },
    el('span', { class: 'k', text: label }),
    el('span', { class: 'bar', role: 'meter', 'aria-valuemin': 0, 'aria-valuemax': limit, 'aria-valuenow': used, 'aria-label': label }, fill),
    el('span', { class: 'v mono', text: `${fmt(used)} of ${fmt(limit)}` }));
}

function tabs(which) {
  const t = (id, label, n) => el('a', {
    href: id === 'computers' ? '#/' : '#/configs',
    class: 'tab',
    'aria-current': which === id ? 'page' : null,
  }, label, el('span', { class: 'count mono', text: String(n) }));
  return el('nav', { class: 'tabs-l', 'aria-label': 'Remote computers or configs' },
    t('computers', 'Remote computers', now.instances.length),
    t('configs', 'Configs', now.configs.length));
}

// ---- the overview: your computers, as cards

function overview() {
  head('Remote computers', [],
    now.member ? el('a', { class: 'btn', href: '#/new', text: 'New remote computer' }) : null);
  const s = now;
  const cards = s.instances.map((i) => el('article', { class: 'card-c' },
    el('div', { class: 'card-top' },
      el('h2', {}, el('a', { href: `#/c/${i.id}`, text: i.name })),
      state(i.state)),
    el('p', { class: 'mono dim', text: spec(i) }),
    copyable(sshOf(i)),
    el('div', { class: 'card-foot' },
      i.consent === 'any' ? el('span', { class: 'state-word', 'data-state': 'attention', text: "On anyone's box" }) : el('span', { class: 'dim', text: `From ${i.config}` }),
      up(i)
        ? el('button', { type: 'button', class: 'quiet', text: 'Stop', onclick: () => act(() => call('POST', `/api/instances/${i.id}/stop`, {})) })
        : el('button', { type: 'button', text: 'Start', onclick: () => act(() => call('POST', `/api/instances/${i.id}/start`, {})) }))));
  $('view').replaceChildren(
    el('section', { class: 'usage panel', 'aria-label': 'What your remote computers take, together' },
      meter('Memory', s.used.memory, s.limits.memory, gib),
      meter('Threads', s.used.threads, s.limits.threads, String)),
    tabs('computers'),
    cards.length
      ? el('div', { class: 'cards' }, cards)
      : el('div', { class: 'empty-p panel' },
        el('h2', { text: s.member ? 'No remote computers yet' : 'Remote computers are for members' }),
        s.member ? el('p', {}, 'A remote computer is yours to SSH into and install what you need. Make one from the generic config, or write your own: how big it is, what it has, and who can reach it.') : null,
        s.member ? el('a', { class: 'btn', href: '#/new', text: 'Make a remote computer' }) : null));
}

// ---- one computer

function computer(id) {
  const i = now.instances.find((x) => x.id === id);
  if (!i) return missing('That remote computer');
  const run = (verb, label, quiet) => el('button', {
    type: 'button', class: quiet ? 'quiet' : null, text: label,
    onclick: () => act(() => call('POST', `/api/instances/${i.id}/${verb}`, {})),
  });
  head(i.name, [['Remote computers', '#/'], [i.name]], state(i.state), up(i) ? run('stop', 'Stop', true) : run('start', 'Start'));

  const applied = i.applied
    ? i.applied.ok
      ? el('p', {}, el('span', { class: 'state-word', 'data-state': 'ok', text: 'Applied' }), ' ', el('span', { class: 'dim', text: `Its Nix file built in ${i.applied.took}s.` }))
      : el('p', {}, el('span', { class: 'state-word', 'data-state': 'broken', text: "Didn't apply" }), ' ', el('span', { class: 'dim', text: 'The computer kept its last working system. The log says why.' }))
    : el('p', {}, el('span', { class: 'state-word', 'data-state': up(i) ? 'working' : 'off', text: up(i) ? 'Applying' : 'Not applied yet' }));
  const log = el('pre', { class: 'log', tabindex: 0, 'aria-label': 'What its last apply said', text: '—' });
  if (i.applied) fetch(`/api/instances/${i.id}/log`).then((r) => (r.ok ? r.text() : '')).then((t) => { log.textContent = t || 'nothing yet'; }).catch(() => {});

  const c = now.configs.find((x) => x.name === i.config);
  const widens = c && c.consent === 'any' && i.consent !== 'any';
  const apply = el('button', {
    type: 'button', text: 'Apply its config',
    onclick: () => {
      if (widens && !yesBox.querySelector('input').checked) { note("Say yes below first: its config lets it run on someone else's box."); return; }
      act(() => call('POST', `/api/instances/${i.id}/apply`, { yes: widens }));
    },
  });
  const yesBox = widens ? warning() : null;

  const danger = el('section', { class: 'panel sec danger' },
    el('h2', { text: 'Start over' }),
    up(i) ? el('p', { class: 'dim', text: 'Stop it first to reset or delete it.' }) : null,
    el('div', { class: 'row' },
      confirming('Reset system', 'Its system goes back to the first disk. /home is kept.', () => act(() => call('POST', `/api/instances/${i.id}/reset`, {})), up(i)),
      confirming('Delete', `Delete ${i.name} and both its disks. This can't be undone.`, () => act(() => call('DELETE', `/api/instances/${i.id}`), '#/'), up(i))));

  $('view').replaceChildren(el('div', { class: 'split' },
    el('div', { class: 'col' },
      el('section', { class: 'panel sec' },
        el('h2', { text: 'Connect' }),
        el('p', { class: 'dim', text: 'With the Commonty network on. From a terminal with dd, nothing to set up:' }),
        copyable(`dd vm ssh ${i.name}`),
        el('p', { class: 'dim', text: 'Or with plain ssh, once its key is saved as ~/.ssh/commonty-' + i.name + ':' }),
        copyable(sshOf(i)),
        el('div', { class: 'row' },
          el('a', { class: 'btn quiet-l', href: `/api/instances/${i.id}/key`, download: `commonty-${i.name}`, text: 'Download its key' }),
          confirming('New key', 'A new key is made and the old one stops working at once. Saved copies of the old one become useless.', () => act(() => call('POST', `/api/instances/${i.id}/rekey`, {})))),
        el('p', { class: 'dim small-t', text: 'Its own key, made for this remote computer alone. After downloading: chmod 600 ~/.ssh/commonty-' + i.name }),
        i.forwards.length
          ? el('table', { class: 'ports' },
            el('thead', {}, el('tr', {}, el('th', { text: 'On the computer' }), el('th', { text: 'Reach it at' }))),
            el('tbody', {}, i.forwards.map((f) => el('tr', {},
              el('td', { class: 'mono', text: String(f.guest) }),
              el('td', { class: 'mono', text: `${i.address.split(':')[0]}:${f.host}` })))))
          : null),
      el('section', { class: 'panel sec' },
        el('h2', { text: 'Software' }),
        applied,
        log,
        el('div', { class: 'row' },
          apply,
          c ? el('a', { href: `#/configs/${encodeURIComponent(c.name)}`, text: `Edit ${c.name}` }) : el('span', { class: 'dim', text: 'Its config is gone.' })),
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
        el('dt', { text: 'Made from' }), el('dd', {}, c ? el('a', { href: `#/configs/${encodeURIComponent(c.name)}`, text: i.config }) : i.config),
        el('dt', { text: 'Runs on' }), el('dd', { text: i.consent === 'any' ? "Anyone's box: its owner can read it" : 'Your boxes only' }),
        el('dt', { text: 'Made' }), el('dd', { class: 'mono', text: new Date(i.created).toLocaleDateString() })))));
}

function confirming(label, said, fn, disabled) {
  const wrap = el('div', { class: 'confirm' });
  const ask = el('button', { type: 'button', class: 'quiet', text: label, disabled });
  ask.addEventListener('click', () => {
    wrap.replaceChildren(
      el('p', { text: said }),
      el('div', { class: 'row' },
        el('button', { type: 'button', class: 'bad', text: label, onclick: fn }),
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

// ---- configs

function configs() {
  head('Remote computers', [], now.member ? el('a', { class: 'btn quiet-l', href: '#/configs/new', text: 'New config' }) : null);
  $('view').replaceChildren(
    tabs('configs'),
    now.configs.length
      ? el('div', { class: 'cards' }, now.configs.map((c) => {
        const made = now.instances.filter((i) => i.config === c.name).length;
        return el('article', { class: 'card-c' },
          el('div', { class: 'card-top' },
            el('h2', {}, el('a', { href: `#/configs/${encodeURIComponent(c.name)}`, text: c.name })),
            c.consent === 'any' ? el('span', { class: 'state-word', 'data-state': 'attention', text: "Anyone's box" }) : el('span', { class: 'state-word', 'data-state': 'off', text: 'My boxes' })),
          el('p', { class: 'mono dim', text: spec(c) }),
          el('p', { class: 'dim', text: `${c.ports.length ? `Ports ${c.ports.join(', ')}` : 'SSH only'} · ${made} made` }),
          el('div', { class: 'card-foot' },
            el('a', { href: `#/configs/${encodeURIComponent(c.name)}`, text: 'Edit' }),
            el('a', { class: 'btn', href: `#/new/${encodeURIComponent(c.name)}`, text: 'Make one' })));
      }))
      : el('div', { class: 'empty-p panel' },
        el('h2', { text: 'No configs yet' }),
        el('p', { text: 'A config says how big a remote computer is, what it has installed, and who can reach it.' }),
        now.member ? el('a', { class: 'btn', href: '#/configs/new', text: 'Write a config' }) : null));
}

const DEFAULT_NIX = `{ pkgs, ... }:
{
  # what this computer has, besides the basics (git, curl, vim, htop)
  environment.systemPackages = with pkgs; [ ripgrep nodejs ];
}
`;

function field(id, label, input, hint) {
  return el('div', { class: 'field' }, el('label', { for: id, text: label }), input, hint ? el('p', { class: 'hint', text: hint }) : null);
}

function editor(name) {
  const fresh = name === 'new';
  const c = fresh
    ? { name: '', memory: 4096, threads: 2, disk: 32, consent: now.user === now.boxOwner ? 'own' : 'any', keys: [], ports: [], players: [], nix: DEFAULT_NIX }
    : now.configs.find((x) => x.name === name);
  if (!c) return missing('That config');
  head(fresh ? 'New config' : c.name, [['Remote computers', '#/'], ['Configs', '#/configs'], [fresh ? 'New' : c.name]]);
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
    el('p', { class: 'hint', text: `All your remote computers together: up to ${gib(L.memory)} and ${L.threads} threads. A disk only grows, never shrinks.` }));
  const access = el('section', { class: 'panel sec' },
    el('h2', { text: 'Access' }),
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
      'Saved and applied, the computer rebuilds itself from this. /home is kept.'));
  // settings down the left, the Nix file taking the rest
  const f = el('form', { class: 'editor', id: 'ed' },
    el('div', { class: 'ed-left' }, nameSec, size, access, where),
    software,
    el('div', { class: 'savebar' },
      el('a', { href: '#/configs', text: 'Cancel' }),
      el('button', { type: 'submit', text: fresh ? 'Save config' : 'Save' })));
  f.addEventListener('submit', (ev) => {
    ev.preventDefault();
    const split = (s) => s.split(/[\s,]+/).filter(Boolean);
    const body = {
      name: $('ed-name').value,
      memory: Number($('ed-memory').value),
      threads: Number($('ed-threads').value),
      disk: Number($('ed-disk').value),
      consent: f.querySelector('input[name=consent]:checked').value,
      // each computer makes its own key; any a config had before stay
      keys: c.keys,
      ports: split($('ed-ports').value).map(Number),
      players: split($('ed-players').value),
      nix: $('ed-nix').value,
    };
    act(() => call('PUT', `/api/configs/${encodeURIComponent(body.name)}`, body), '#/configs');
  });
  const del = fresh ? null : el('section', { class: 'panel sec danger' },
    el('h2', { text: 'Delete this config' }),
    el('p', { class: 'dim', text: 'Remote computers made from it keep running; they just have nothing to apply.' }),
    confirming('Delete config', `Delete the config ${c.name}?`, () => act(() => call('DELETE', `/api/configs/${encodeURIComponent(c.name)}`), '#/configs')));
  $('view').replaceChildren(f, del || '');
}

// ---- making one

function maker(pick) {
  head('New remote computer', [['Remote computers', '#/'], ['New']]);
  let chosen = now.configs.find((c) => c.name === pick) || now.configs[0];
  const warnSlot = el('div');
  const showWarn = () => {
    warnSlot.replaceChildren(chosen.consent === 'any' ? warning() : '');
  };
  const list = el('div', { class: 'pick-list' }, now.configs.map((c) => {
    const r = el('input', { type: 'radio', name: 'cfg', value: c.name, checked: c === chosen });
    r.addEventListener('change', () => { chosen = c; showWarn(); });
    return el('label', { class: 'opt-card' }, r,
      el('span', {}, el('strong', { text: c.name }), el('span', { class: 'mono dim', text: spec(c) }),
        el('span', { class: 'dim', text: c.consent === 'any' ? "Anyone's box" : 'My boxes only' })));
  }));
  showWarn();
  const f = el('form', { class: 'editor' },
    el('section', { class: 'panel sec' }, el('h2', { text: 'From which config' }), list,
      el('p', { class: 'hint' }, 'Or ', el('a', { href: '#/configs/new', text: 'write a new config' }), '.')),
    el('section', { class: 'panel sec' }, el('h2', { text: 'Its name' }),
      field('mk-name', 'Name', el('input', { id: 'mk-name', required: true, pattern: '[a-z0-9][a-z0-9-]{0,31}', autocomplete: 'off', placeholder: 'dev' }), 'Lowercase letters, digits and dashes.'),
      warnSlot),
    el('div', { class: 'savebar' }, el('a', { href: '#/', text: 'Cancel' }), el('button', { type: 'submit', text: 'Make it' })));
  f.addEventListener('submit', (ev) => {
    ev.preventDefault();
    const yes = chosen.consent === 'any' && !!$('yes-any')?.checked;
    if (chosen.consent === 'any' && !yes) { note("Say yes to the warning first: it runs on someone else's box."); return; }
    act(async () => {
      const made = await call('POST', '/api/instances', { config: chosen.name, name: $('mk-name').value, yes });
      location.hash = `#/c/${made.id}`;
    });
  });
  $('view').replaceChildren(f);
}

function missing(what) {
  head('Not here', [['Remote computers', '#/'], ['Not here']]);
  $('view').replaceChildren(el('div', { class: 'empty-p panel' },
    el('h2', { text: `${what} isn't here` }),
    el('p', {}, 'It may have been deleted. ', el('a', { href: '#/', text: 'Back to your remote computers' }), '.')));
}

// ---- the address says which

function route() {
  if (!now) return;
  const [, a, b] = (location.hash || '#/').slice(1).split('/');
  const name = b && decodeURIComponent(b);
  if (a === 'c') computer(b);
  else if (a === 'configs' && name) editor(name);
  else if (a === 'configs') configs();
  else if (a === 'new') maker(name);
  else overview();
  const busy = now.instances.some((i) => i.state === 'starting' || i.state === 'stopping');
  clearTimeout(poll);
  if (busy) poll = setTimeout(refresh, 3000);
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
