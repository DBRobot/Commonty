// Who is in this home, who asked, and what Certificate Transparency showed
// (servers/smarthome/door/src/people.rs). Drawn from the last copy at once,
// then from the door's answer.
import { $, el, load } from './kit.js';

const say = (t) => { $('msg').textContent = t; };
const day = (ts) => (ts ? new Date(ts * 1000).toLocaleDateString() : '—');

async function post(path, body) {
  const r = await fetch(path, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body || {}),
  });
  if (r.status === 401) { location.reload(); throw new Error('signed out'); }
  if (!r.ok) throw new Error((await r.text()) || `the door said ${r.status}`);
  return r.status === 204 ? null : r.json();
}

function act(button, path, body, done) {
  button.disabled = true;
  post(path, body)
    .then((v) => { say(done(v)); refresh(); })
    .catch((e) => { say(e.message); button.disabled = false; });
}

function row(p, me) {
  const state = p.ready
    ? el('span', { class: 'state-word', 'data-state': 'ok', text: 'In' })
    : el('span', { class: 'state-word', 'data-state': 'working', text: 'Getting ready' });
  const remove = el('button', { class: 'quiet inline', type: 'button', text: p.name === me ? 'Leave' : 'Remove' });
  remove.addEventListener('click', () => act(remove, '/_door/remove', { name: p.name }, () => 'Removed.'));
  return el('li', {},
    el('span', { class: 'who', text: p.name }),
    el('span', { class: 'fp', text: p.fingerprint, title: 'their key' }),
    el('span', { class: 'tag', text: `since ${day(p.since)}` }),
    el('span', { class: 'grow' }),
    state,
    remove);
}

function asking(a) {
  const yes = el('button', { class: 'inline', type: 'button', text: 'Let in' });
  yes.addEventListener('click', () => act(yes, '/_door/let-in', { name: a.name },
    (v) => (v && v.ready ? `${a.name} is in.` : `${a.name} is in; their Home Assistant is getting ready.`)));
  const no = el('button', { class: 'quiet inline', type: 'button', text: 'Turn down' });
  no.addEventListener('click', () => act(no, '/_door/remove', { name: a.name }, () => 'Turned down.'));
  return el('li', {},
    el('span', { class: 'who', text: a.name }),
    el('span', { class: 'fp', text: a.fingerprint, title: 'their key: check it with them' }),
    el('span', { class: 'tag', text: `asked ${day(a.at)}` }),
    el('span', { class: 'grow' }),
    yes, no);
}

function draw(v) {
  $('me').textContent = v.me ? v.me.name : '';
  const inside = !!(v.me && v.me.in);
  $('inside').hidden = !inside;
  $('outside').hidden = inside;
  if (!inside) {
    const asked = v.me && v.me.asked;
    $('out-state').textContent = asked ? 'Asked' : 'Not in yet';
    $('out-state').dataset.state = asked ? 'working' : 'off';
    $('out-text').textContent = asked
      ? 'Asked. Anyone already in this home can let you in; this page shows it when they have.'
      : 'You are signed in, but not in this home yet. Ask, and anyone already in can let you in.';
    $('ask').hidden = !!asked;
    $('ct').hidden = true;
    return;
  }
  const people = v.people || [];
  const mine = people.find((p) => p.name === v.me.name);
  $('lead').textContent = mine && !mine.ready
    ? 'You are in. Your Home Assistant is getting ready; this takes a minute.'
    : 'Everyone in can open the home, and let others in.';
  $('people').replaceChildren(...people.map((p) => row(p, v.me.name)));
  const asked = v.asked || [];
  $('asked').replaceChildren(...asked.map(asking));
  $('asked-empty').hidden = asked.length > 0;
  const odd = (v.ct && v.ct.unexpected) || [];
  $('ct').hidden = odd.length === 0;
  $('ct-list').replaceChildren(...odd.map((u) =>
    el('li', {}, el('a', { href: `https://crt.sh/?id=${u.id}`, rel: 'noreferrer', text: `${u.serial} · ${u.issuer} · ${u.not_before}` }))));
}

const when = (ts) => new Date(ts * 1000).toLocaleString([], { dateStyle: 'medium', timeStyle: 'short' });

function backup(b) {
  const [state, word, text] = !b
    ? ['off', 'Not yet', 'Every night this home is backed up, locked so only the people in can open it. The first is an hour after the controller starts.']
    : b.ok
      ? ['ok', 'Backed up', `Last backup: ${when(b.at)}. Locked so only the people in can open it.`]
      : ['attention', 'Backup failed', `The last backup, ${when(b.at)}, did not get away: ${b.error || 'no reason given'}. It tries again tonight.`];
  $('backup-state').dataset.state = state;
  $('backup-state').textContent = word;
  $('backup-text').textContent = text;
}

function updates(v) {
  backup(v.backup);
  const held = v.updates === 'hold';
  $('upd-auto').setAttribute('aria-pressed', String(!held));
  $('upd-hold').setAttribute('aria-pressed', String(held));
  $('release').textContent = v.release || '—';
  $('upd-hint').textContent = held
    ? 'Held: this home stays on the release it runs until you let updates go again.'
    : 'New releases install on their own. Hold them, and this home stays on the release it runs until you let them go.';
}

for (const [id, hold] of [['upd-auto', false], ['upd-hold', true]]) {
  $(id).addEventListener('click', () => {
    if ($(id).getAttribute('aria-pressed') === 'true') return;
    post('/_door/updates', { hold })
      .then((v) => { updates(v); say(hold ? 'Updates held.' : 'Updates are automatic again.'); })
      .catch((e) => say(e.message));
  });
}

function refresh() {
  load('/_door/people', draw).catch(() => say('The door did not answer. The list may be out of date.'));
  load('/_door/status', updates).catch(() => {});
}

$('ask').addEventListener('click', () => act($('ask'), '/_door/ask', {}, () => 'Asked.'));
for (const id of ['signout', 'signout-out']) {
  $(id).addEventListener('click', () => post('/_door/signout').finally(() => location.reload()));
}
refresh();
