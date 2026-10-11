// The aliases on Settings' Email tab: a member's own addresses for the
// sites they give one to, made, labelled, turned off and deleted here. A
// frame from the mail Worker, like the email line, so no box sees the list;
// it tells Settings how tall it is (to any page: only the fleet's own may
// frame it, by its content policy).
import { signIn, confirm, me, post } from './passkey.js';

const $ = (id) => document.getElementById(id);
const name = new URLSearchParams(location.search).get('name') || '';
const COPY = '<svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" aria-hidden="true"><rect x="8" y="8" width="12" height="12" rx="1.5"/><path d="M16 8V5.5A1.5 1.5 0 0 0 14.5 4h-9A1.5 1.5 0 0 0 4 5.5v9A1.5 1.5 0 0 0 5.5 16H8"/></svg>';
let aliases = [];
let max = 100;
// the address on offer in New alias: the nth of today's, which the Worker
// makes again from its own key
let offer = null;

const el = (tag, attrs = {}, ...kids) => {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === 'text') e.textContent = v;
    else if (k === 'html') e.innerHTML = v;
    else e.setAttribute(k, v);
  }
  e.append(...kids);
  return e;
};

// Settings sizes the frame to what is drawn; an open dialog needs room
const dlg = $('dlg');
function tell() {
  const h = Math.max(document.documentElement.scrollHeight, dlg.open ? 380 : 0);
  if (parent !== window) parent.postMessage({ aliasesHeight: h }, '*');
}
new ResizeObserver(tell).observe(document.body);

const day = (t) => new Date(t * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
const made = (t) => (Date.now() / 1000 - t < 120 ? 'Made just now' : `Made ${day(t)}`);

function count() {
  const c = $('count');
  if (aliases.length >= max) {
    c.replaceChildren(el('span', { class: 'state-word', 'data-state': 'attention', text: `${aliases.length} of ${max}` }));
  } else {
    c.textContent = `${aliases.length} of ${max}`;
  }
  const full = aliases.length >= max;
  $('new').disabled = full;
  $('new').title = full ? 'Delete one you no longer use to make another' : '';
}

function row(a) {
  const local = a.address.split('@')[0];
  const label = el('span', { class: 'al-label' }, a.label || 'No label');
  if (!a.on) label.append(' ', el('span', { class: 'state-word', 'data-state': 'off', text: 'Off: mail bounces' }));
  const copy = el('button', { class: 'al-copy', type: 'button', 'aria-label': 'Copy address', title: 'Copy', html: COPY });
  // a narrow screen breaks the address before its @, not inside a word
  const at = a.address.indexOf('@');
  const addr = el('span', { class: 'al-addr-text' }, a.address.slice(0, at), el('wbr'), a.address.slice(at));
  const sw = el('button', { class: 'switch', type: 'button', role: 'switch', 'aria-checked': String(a.on), 'aria-label': `${a.label || a.address} receives mail` });
  const more = el('button', { class: 'quiet al-more', type: 'button', 'aria-label': 'More', 'aria-haspopup': 'menu', 'aria-expanded': 'false', text: '⋯' });
  const li = el('li', { class: a.on ? '' : 'off', 'data-local': local },
    el('div', { class: 'al-what' }, label, el('span', { class: 'al-line' }, addr, copy),
      el('span', { class: 'al-meta', text: a.on || !a.off ? made(a.made) : `${made(a.made)} · off since ${day(a.off)}` })),
    el('div', { class: 'al-act' }, sw, el('span', { class: 'al-menu' }, more)));
  copy.onclick = () => copyOf(a.address, copy);
  sw.onclick = () => change(a, { on: !a.on });
  more.onclick = (e) => { e.stopPropagation(); menu(a, more); };
  return li;
}

function draw() {
  count();
  const q = $('find').value.trim().toLowerCase();
  $('find').hidden = aliases.length < 6;
  const shown = aliases.filter((a) => !q || `${a.label} ${a.address}`.toLowerCase().includes(q));
  $('list').replaceChildren(...shown.map(row));
  if (!aliases.length) {
    $('state').replaceChildren(el('div', { class: 'al-empty' }, el('b', { text: 'No aliases yet.' }),
      'Press New alias for the next site that asks for your email. Each one keeps the name you give it, so you can tell later which site has which address.'));
  } else if (aliases.length >= max) {
    $('state').replaceChildren(el('p', { class: 'al-limit', text: `You have ${max} aliases, the most there can be. Delete one you no longer use to make another.` }));
  } else if (q && !shown.length) {
    $('state').replaceChildren(el('p', { class: 'al-empty', text: `No alias matches “${q}”.` }));
  } else {
    $('state').replaceChildren();
  }
}

function broken(text, again) {
  const b = el('button', { class: 'quiet', type: 'button', text: again ? 'Try again' : 'Show my aliases' });
  $('state').replaceChildren(el('div', { class: again ? 'al-oops' : 'al-empty' }, text, b));
  $('list').replaceChildren();
  $('new').disabled = true;
  b.onclick = async () => {
    b.disabled = true;
    try {
      if (!again) await signIn(name);
      await load();
    } catch {
      b.disabled = false;
    }
  };
}

async function load() {
  const m = await me().catch(() => null);
  if (!m || m.name !== name) {
    $('count').textContent = `— of ${max}`;
    return broken('Your aliases are kept behind your passkey. Confirm it is you to see them here.', false);
  }
  const r = await fetch('/api/aliases').catch(() => null);
  if (!r || !r.ok) return broken('Your aliases didn’t load: Cloudflare didn’t answer. Mail to them still arrives as usual.', true);
  const v = await r.json();
  aliases = v.aliases;
  max = v.max;
  draw();
}

async function copyOf(text, button) {
  try {
    await navigator.clipboard.writeText(text);
    button.classList.add('done');
    button.title = 'Copied';
    setTimeout(() => { button.classList.remove('done'); button.title = 'Copy'; }, 1500);
  } catch {
    const s = getSelection();
    const r = document.createRange();
    r.selectNodeContents(button.previousSibling);
    s.removeAllRanges();
    s.addRange(r);
  }
}

async function change(a, what) {
  const local = a.address.split('@')[0];
  try {
    const v = await post(`/api/aliases/${local}`, what);
    Object.assign(a, v.alias);
  } catch (err) {
    say(err.message);
  }
  draw();
}

function say(text) {
  $('state').replaceChildren(el('p', { class: 'al-oops', text }));
}

let open = null;
function close() {
  if (!open) return;
  open.pop.remove();
  open.button.setAttribute('aria-expanded', 'false');
  open = null;
}
document.addEventListener('click', (e) => { if (open && !open.pop.contains(e.target)) close(); });
document.addEventListener('keydown', (e) => { if (e.key === 'Escape') close(); });

function menu(a, button) {
  const was = open?.button === button;
  close();
  if (was) return;
  const rename = el('button', { type: 'button', role: 'menuitem', text: 'Rename' });
  const del = el('button', { type: 'button', role: 'menuitem', class: 'danger', text: 'Delete…' });
  const pop = el('div', { class: 'popmenu al-pop', role: 'menu' }, rename, del);
  button.parentNode.append(pop);
  button.setAttribute('aria-expanded', 'true');
  open = { pop, button };
  rename.focus();
  rename.onclick = () => {
    close();
    const li = $('list').querySelector(`[data-local="${a.address.split('@')[0]}"]`);
    const label = li.querySelector('.al-label');
    const input = el('input', { class: 'al-rename', type: 'text', maxlength: '60', 'aria-label': 'Label' });
    input.value = a.label;
    label.replaceChildren(input);
    input.focus();
    let done = false;
    const finish = (keep) => {
      if (done) return;
      done = true;
      if (keep && input.value.trim() !== a.label) change(a, { label: input.value.trim() });
      else draw();
    };
    input.onkeydown = (k) => {
      if (k.key === 'Enter') finish(true);
      if (k.key === 'Escape') finish(false);
    };
    input.onblur = () => finish(true);
  };
  del.onclick = (e) => {
    e.stopPropagation();
    const yes = el('button', { type: 'button', class: 'al-yes', text: 'Delete' });
    const no = el('button', { type: 'button', class: 'quiet', text: 'Keep it' });
    pop.replaceChildren(el('div', { class: 'al-confirm' },
      'Delete this alias? Its address is gone for good and its mail bounces. No one is ever given it again.',
      el('div', {}, yes, no)));
    no.onclick = close;
    yes.onclick = async () => {
      yes.disabled = true;
      try {
        // deleting is for good: the passkey, for this one
        await post(`/api/aliases/${a.address.split('@')[0]}/delete`, await confirm(name));
        aliases = aliases.filter((x) => x !== a);
        close();
        draw();
      } catch (err) {
        close();
        say(err.message);
      }
    };
  };
}

async function another() {
  const was = { offer, text: $('next').textContent };
  $('next').textContent = '…';
  $('make-ok').disabled = true;
  $('make-bad').textContent = '';
  try {
    const v = await post('/api/aliases/next', { n: offer ? offer.n + 1 : 0 });
    offer = { n: v.n, day: v.day };
    $('next').textContent = v.address;
    $('make-ok').disabled = false;
  } catch (err) {
    // the last of today's: the one shown stays on offer
    offer = was.offer;
    $('next').textContent = was.offer ? was.text : '—';
    $('make-ok').disabled = !was.offer;
    $('another').disabled = true;
    $('make-bad').textContent = err.message;
  }
}

$('new').onclick = () => {
  $('make').hidden = false;
  $('done').hidden = true;
  $('label').value = '';
  $('make-bad').textContent = '';
  offer = null;
  $('another').disabled = false;
  dlg.showModal();
  tell();
  $('label').focus();
  another();
};
$('another').onclick = another;
$('cancel').onclick = () => dlg.close();
$('done-ok').onclick = () => dlg.close();
dlg.addEventListener('close', tell);
$('make').addEventListener('submit', async (e) => {
  e.preventDefault();
  $('make-ok').disabled = true;
  $('make-bad').textContent = '';
  try {
    const v = await post('/api/aliases', { ...offer, label: $('label').value });
    aliases.unshift(v.alias);
    draw();
    $('done-addr').textContent = v.alias.address;
    $('done-label').textContent = v.alias.label || 'the site';
    $('done-copy').textContent = 'Copy address';
    $('make').hidden = true;
    $('done').hidden = false;
    $('done-ok').focus();
  } catch (err) {
    $('make-bad').textContent = err.message;
    $('make-ok').disabled = false;
  }
});
$('done-copy').onclick = async () => {
  try {
    await navigator.clipboard.writeText($('done-addr').textContent);
    $('done-copy').textContent = 'Copied';
  } catch {
    const r = document.createRange();
    r.selectNodeContents($('done-addr'));
    getSelection().removeAllRanges();
    getSelection().addRange(r);
  }
};
$('find').addEventListener('input', draw);

load();
