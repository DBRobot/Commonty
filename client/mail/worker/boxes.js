// Your boxes at boot: which unlocked at home, which is waiting to be let in
// from somewhere new, and the switch for one that was stolen. Letting one in
// or marking it wants your passkey just now (unlock.js).
import { signIn, post } from './passkey.js';

const $ = (id) => document.getElementById(id);
const name = new URLSearchParams(location.search).get('name') || '';

function ago(t) {
  const d = Math.max(0, Date.now() / 1000 - t);
  if (d < 5400) return `${Math.max(1, Math.round(d / 60))} min ago`;
  if (d < 172800) return `${Math.round(d / 3600)} h ago`;
  return `${Math.round(d / 86400)} days ago`;
}

function el(tag, cls, text) {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}

function say(text) {
  $('list-msg').textContent = text;
  $('list-msg').hidden = !text;
}

// a change wants the passkey just now; asked again when it is older
async function act(path, body, button) {
  button.disabled = true;
  say('');
  try {
    try {
      await post(path, body);
    } catch (e) {
      if (!/confirm it is you/.test(String(e.message))) throw e;
      await signIn(name);
      await post(path, body);
    }
    await load();
  } catch (e) {
    button.disabled = false;
    say(e.name === 'NotAllowedError' ? 'Your passkey was not used. Nothing changed.' : String(e.message || e));
  }
}

function row(b) {
  const li = el('li', 'bx-item');
  const text = el('div');
  text.append(el('div', 'bx-name', b.box));
  let state;
  if (b.stolen) state = 'Marked stolen. It will not unlock anywhere until you undo this.';
  else if (b.waiting) state = `Started somewhere new ${ago(b.waiting)} and is waiting for you.`;
  else if (!b.kept) state = 'Its disks are not encrypted yet.';
  else state = b.lastHome ? `Unlocks at home. Last seen home ${ago(b.lastHome)}.` : 'Unlocks at home.';
  text.append(el('div', b.waiting || b.stolen ? 'bx-meta bad' : 'bx-meta', state));
  const actions = el('div', 'bx-actions');
  if (b.waiting && !b.stolen) {
    const go = el('button', '', 'Let it in');
    go.onclick = () => act('/api/boxes/approve', { box: b.box }, go);
    actions.append(go);
  }
  const s = el('button', b.stolen ? 'quiet' : 'quiet danger', b.stolen ? 'It is not stolen' : 'Mark stolen');
  s.onclick = () => act('/api/boxes/stolen', { box: b.box, stolen: !b.stolen }, s);
  actions.append(s);
  li.append(text, actions);
  return li;
}

async function load() {
  const r = await fetch('/api/boxes');
  if (r.status === 401) {
    $('ask').hidden = false;
    $('list').hidden = true;
    return;
  }
  const boxes = await r.json();
  $('boxes').replaceChildren(...boxes.map(row));
  $('ask').hidden = true;
  $('list').hidden = false;
}

$('pass').onclick = async () => {
  $('pass').disabled = true;
  $('ask-msg').hidden = true;
  try {
    await signIn(name);
    await load();
  } catch (e) {
    $('pass').disabled = false;
    $('ask-msg').textContent = e.name === 'NotAllowedError' ? 'Your passkey was not used.' : String(e.message || e);
    $('ask-msg').hidden = false;
  }
};

load();
