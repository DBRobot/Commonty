// Contacts: everyone the member keeps a number or address for, in every
// address book of their libraries. A person is a .vcf file read and sealed
// in this tab (vdir.js, ical.js); anyone at all, with or without an
// account here. A save names the version it was made from.

import { $, el } from './kit.js';
import * as I from './ical.js';
import { collections, items, save, remove, make, first, fileName, forAccount } from './vdir.js';
import { unlock, save as download } from './library.js';
import { me } from './shell.js';

let books = [];
let libs = [];
const files = new Map();          // address book id -> [{ name, path, etag, text }]
let book = 'all';
let picked = null;                // a person's id
let q = '';
let people = [];

const bookId = (b) => `${b.lib.id}/${b.path}`;
const say = (t) => { $('msg').textContent = t; };
const KINDS = { tel: ['cell', 'home', 'work', 'other'], email: ['home', 'work', 'other'] };
const kindName = (k) => ({ cell: 'Mobile', home: 'Home', work: 'Work', other: 'Other', fax: 'Fax' }[k] || (k ? k[0].toUpperCase() + k.slice(1) : ''));

function rebuild() {
  people = [];
  for (const b of books) {
    for (const f of files.get(bookId(b)) || []) {
      let cs;
      try { cs = I.cards(f.text); } catch { continue; }
      for (const c of cs) people.push({ ...c, book: b, file: f, id: `${bookId(b)}/${f.name}/${c.uid}` });
    }
  }
  people.sort((a, b) => (a.fn || '').localeCompare(b.fn || '', undefined, { sensitivity: 'base' }));
}

const initials = (n) => (n || '?').split(/\s+/).filter((w) => /^\p{L}/u.test(w)).slice(0, 2).map((w) => w[0]).join('').toUpperCase() || '?';
const memberOf = (p) => I.text(I.prop(p.comp, 'X-COMMONTY'));

function visible() {
  const needle = q.toLowerCase();
  return people.filter((p) => (book === 'all' || bookId(p.book) === book)
    && (!needle || [p.fn, p.org, p.title, p.note, ...p.tels.map((t) => t.value), ...p.emails.map((e) => e.value)].join(' ').toLowerCase().includes(needle)));
}

function drawBooks() {
  const row = (id, name, n, sub) => el('button', {
    type: 'button', 'aria-current': String(book === id), onclick: () => { book = id; drawBooks(); drawList(); },
  }, el('span', { class: 'name', text: name, style: 'flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis' }), sub ? el('span', { class: 'sub', text: sub, style: 'font:12px var(--mono);margin-right:8px' }) : '', el('span', { class: 'sub', text: String(n), style: 'font:12px var(--mono)' }));
  $('books').replaceChildren(
    row('all', 'Everyone', people.length),
    ...books.map((b) => row(bookId(b), b.name, people.filter((p) => p.book === b).length, b.mine ? '' : (b.writable ? b.owner : `${b.owner}, read only`))));
}

function drawList() {
  const list = visible();
  $('count').textContent = `${list.length} of ${people.length}`;
  const box = $('plist');
  box.replaceChildren();
  if (!people.length) box.append(el('p', { class: 'empty-note', text: 'No contacts yet. Add one with New contact, or bring them in with Import .vcf (Google Contacts and most phones export one).' }));
  else if (!list.length) box.append(el('p', { class: 'empty-note', text: 'No one matches. Try part of a name, a number or an email.' }));
  let letter = '';
  for (const p of list) {
    const L = (p.fn || '#')[0].toUpperCase();
    if (L !== letter) { letter = L; box.append(el('div', { class: 'letter', text: L })); }
    box.append(el('button', {
      type: 'button', class: 'prow', 'aria-current': String(picked === p.id),
      onclick: () => { picked = p.id; $('people').classList.add('picked'); drawList(); },
    }, el('span', { class: 'init', text: initials(p.fn) }),
    el('span', { class: 'who' }, el('b', { text: p.fn || 'No name' }), el('span', { text: p.org || p.tels[0]?.value || p.emails[0]?.value || '' })),
    memberOf(p) ? el('span', { class: 'state-word', 'data-state': 'working', text: 'On Commonty' }) : ''));
  }
  drawCard();
}

function drawCard() {
  const p = people.find((x) => x.id === picked) || null;
  const box = $('pcard');
  if (!p) { box.replaceChildren(el('p', { class: 'fine', text: people.length ? 'Pick someone to see their details.' : '' })); return; }
  const dl = el('dl', { class: 'kv' });
  const add = (k, v, mono) => { if (v && (!Array.isArray(v) || v.length)) dl.append(el('dt', { text: k }), el('dd', { class: mono ? 'mono' : '' }, v)); };
  for (const t of p.tels) add(kindName(t.kind) || 'Phone', el('a', { href: `tel:${t.value.replace(/[^\d+]/g, '')}`, text: t.value }), true);
  for (const e of p.emails) add(kindName(e.kind) ? `${kindName(e.kind)} email` : 'Email', el('a', { href: `mailto:${e.value}`, text: e.value }), true);
  for (const a of p.adrs) add(kindName(a.kind) ? `${kindName(a.kind)} address` : 'Address', [a.street, [a.city, a.region, a.code].filter(Boolean).join(' '), a.country].filter(Boolean).join('\n'));
  const b = I.birthday(p.bday);
  if (b) add('Birthday', new Intl.DateTimeFormat(undefined, { day: 'numeric', month: 'long', ...(b.y ? { year: 'numeric' } : {}), timeZone: 'UTC' }).format(Date.UTC(b.y || 2000, b.m - 1, b.d)));
  if (p.url) add('Website', p.url);
  add('Notes', p.note);
  if (!p.book.mine) add('From', `${p.book.name} · ${p.book.owner}`);
  const member = memberOf(p);
  box.replaceChildren(
    el('button', { type: 'button', class: 'quiet small back', onclick: () => { $('people').classList.remove('picked'); } }, '‹ Everyone'),
    el('div', { class: 'who' }, el('span', { class: 'init big', text: initials(p.fn) }),
      el('div', {}, el('h2', { text: p.fn || 'No name' }), (p.title || p.org) ? el('p', { text: [p.title, p.org].filter(Boolean).join(', ') }) : '')),
    member ? el('div', {}, el('span', { class: 'state-word', 'data-state': 'working', text: `On Commonty as ${member}` })) : '',
    dl,
    el('div', { class: 'acts' },
      p.book.writable ? el('button', { type: 'button', onclick: () => openEdit(p) }, 'Edit') : '',
      el('button', { type: 'button', class: 'quiet', onclick: (e) => copyCard(p, e.currentTarget) }, 'Copy as vCard'),
      p.book.writable ? el('button', { type: 'button', class: 'danger-quiet', onclick: () => askDelete(p) }, 'Delete') : ''),
  );
}

async function copyCard(p, btn) {
  try { await navigator.clipboard.writeText(I.serialize(p.comp)); btn.textContent = 'Copied'; }
  catch { btn.textContent = 'Could not copy'; }
  setTimeout(() => { btn.textContent = 'Copy as vCard'; }, 1800);
}

function manyRows(box, kinds, list, type) {
  box.replaceChildren();
  const addRow = (x = { kind: kinds[0], value: '' }) => {
    const sel = el('select', { 'aria-label': 'Kind' }, kinds.map((k) => el('option', { value: k, text: kindName(k) })));
    if (!kinds.includes(x.kind) && x.kind) sel.append(el('option', { value: x.kind, text: kindName(x.kind) }));
    sel.value = x.kind || kinds[0];
    const input = el('input', { type, value: x.value, 'aria-label': type === 'tel' ? 'Phone number' : 'Email address', autocomplete: 'off' });
    const row = el('div', { class: 'many-row' }, sel, input, el('button', { type: 'button', 'aria-label': 'Remove', onclick: () => row.remove() }, '×'));
    box.insertBefore(row, box.lastChild);
  };
  box.append(el('button', { type: 'button', class: 'add-row', onclick: () => addRow() }, type === 'tel' ? 'Add a phone' : 'Add an email'));
  for (const x of list.length ? list : [{ kind: kinds[0], value: '' }]) addRow(x);
}
const rowsOf = (box) => [...box.querySelectorAll('.many-row')].map((r) => ({ kind: r.querySelector('select').value, value: r.querySelector('input').value }));

let editing = null;
function openEdit(p) {
  editing = p || null;
  $('edit-h').textContent = p ? 'Edit contact' : 'New contact';
  $('edit-err').textContent = '';
  const writable = books.filter((b) => b.writable);
  $('p-book').replaceChildren(...writable.map((b) => el('option', { value: bookId(b), text: b.mine ? b.name : `${b.name} (${b.owner})` })));
  $('p-name').value = p?.fn || '';
  $('p-org').value = p?.org || '';
  $('p-title').value = p?.title || '';
  manyRows($('p-tels'), KINDS.tel, p?.tels || [], 'tel');
  manyRows($('p-emails'), KINDS.email, p?.emails || [], 'email');
  const a = p?.adrs[0] || {};
  $('p-street').value = a.street || ''; $('p-city').value = a.city || ''; $('p-region').value = a.region || ''; $('p-code').value = a.code || ''; $('p-country').value = a.country || '';
  const b = I.birthday(p?.bday);
  $('p-bday').value = b ? `${String(b.y || 1604).padStart(4, '0')}-${String(b.m).padStart(2, '0')}-${String(b.d).padStart(2, '0')}` : '';
  $('p-member').value = p ? memberOf(p) : '';
  $('p-note').value = p?.note || '';
  $('p-book').value = p ? bookId(p.book) : (book !== 'all' && writable.some((x) => bookId(x) === book) ? book : (writable[0] ? bookId(writable[0]) : ''));
  $('edit').showModal();
  $('p-name').focus();
}
$('edit-cancel').addEventListener('click', () => $('edit').close());

$('edit-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const fn = $('p-name').value.trim();
  if (!fn) { $('edit-err').textContent = 'A contact needs a name.'; return; }
  const bd = $('p-bday').value;
  // a birthday with no year is kept as one (--MMDD); 1604 is how phones say "no year"
  const bday = bd ? (bd.startsWith('1604-') ? `--${bd.slice(5).replace('-', '')}` : bd.replace(/-/g, '')) : '';
  const fields = {
    fn, org: $('p-org').value.trim(), title: $('p-title').value.trim(),
    tels: rowsOf($('p-tels')), emails: rowsOf($('p-emails')),
    adrs: [{ kind: editing?.adrs[0]?.kind || '', street: $('p-street').value.trim(), city: $('p-city').value.trim(), region: $('p-region').value.trim(), code: $('p-code').value.trim(), country: $('p-country').value.trim() }, ...(editing?.adrs.slice(1) || [])],
    bday, note: $('p-note').value.trim(),
  };
  const target = books.find((b) => bookId(b) === $('p-book').value);
  if (!target) { $('edit-err').textContent = 'Pick an address book.'; return; }
  $('edit-ok').disabled = true;
  try {
    const done = await savePerson(editing, fields, $('p-member').value.trim(), target);
    $('edit').close();
    if (done) say('Saved');
  } catch (err) {
    $('edit-err').textContent = `Not saved: ${err.message}`;
  } finally {
    $('edit-ok').disabled = false;
  }
});

async function store(b, name, text, etag) {
  const r = await save(b, name, text, etag);
  if (r.changed !== undefined) {
    files.set(bookId(b), await items(b, '.vcf'));
    rebuild(); drawBooks(); drawList();
    say('This contact changed on another device first, so your change was not saved. This is how it is now; make your change again.');
    return null;
  }
  files.set(bookId(b), [...(files.get(bookId(b)) || []).filter((f) => f.name !== name), r.saved]);
  return r.saved;
}

async function savePerson(p, fields, member, target) {
  let comp, name, etag;
  if (p) {
    // the file afresh, and this card in it; the rest of the file stays
    const all = I.parse(p.file.text).filter((c) => c.name === 'VCARD');
    comp = all.find((c) => I.text(I.prop(c, 'UID')) === p.uid) || all[0];
    I.writeCard(comp, fields);
    I.setProp(comp, 'X-COMMONTY', member ? I.escapeText(member) : '');
    const text = all.map(I.serialize).join('');
    if (bookId(target) !== bookId(p.book)) {
      const moved = await store(target, p.file.name, text, 'new');
      if (!moved) return false;
      const ok = await remove(p.book, p.file.name, p.file.etag);
      if (ok) files.set(bookId(p.book), (files.get(bookId(p.book)) || []).filter((f) => f.name !== p.file.name));
      picked = `${bookId(target)}/${p.file.name}/${p.uid}`;
    } else {
      if (!await store(p.book, p.file.name, text, p.file.etag)) return false;
      picked = p.id;
    }
  } else {
    const uid = I.newUid();
    comp = I.newCard(uid);
    I.writeCard(comp, fields);
    if (member) I.setProp(comp, 'X-COMMONTY', I.escapeText(member));
    name = fileName(uid, '.vcf');
    etag = 'new';
    if (!await store(target, name, I.serialize(comp), etag)) return false;
    picked = `${bookId(target)}/${name}/${uid}`;
  }
  rebuild(); drawBooks(); drawList();
  return true;
}

function ask(title, text, buttons) {
  return new Promise((ok) => {
    const dlg = $('ask');
    $('ask-h').textContent = title;
    $('ask-text').textContent = text;
    $('ask-body').replaceChildren();
    $('ask-actions').replaceChildren(...buttons.map(([label, value, cls]) =>
      el('button', { type: 'button', class: cls || '', onclick: () => { dlg.close(); ok(value); } }, label)));
    dlg.onclose = () => ok(null);
    dlg.showModal();
  });
}

async function askDelete(p) {
  const yes = await ask('Delete this contact?', `${p.fn || 'This contact'} goes to the trash, where it stays for 30 days.`, [['Cancel', false, 'quiet'], ['Delete', true, 'danger']]);
  if (!yes) return;
  try {
    const all = I.parse(p.file.text).filter((c) => c.name === 'VCARD');
    if (all.length > 1) {
      // a file holding several: this one out, the rest kept
      const rest = all.filter((c) => I.text(I.prop(c, 'UID')) !== p.uid);
      if (!await store(p.book, p.file.name, rest.map(I.serialize).join(''), p.file.etag)) return;
    } else {
      const ok = await remove(p.book, p.file.name, p.file.etag);
      if (!ok) { files.set(bookId(p.book), await items(p.book, '.vcf')); rebuild(); drawList(); say('This contact changed on another device first, so it was not deleted.'); return; }
      files.set(bookId(p.book), (files.get(bookId(p.book)) || []).filter((f) => f.name !== p.file.name));
    }
  } catch (err) { say(`Not deleted: ${err.message}`); return; }
  picked = null;
  $('people').classList.remove('picked');
  rebuild(); drawBooks(); drawList();
  say('Deleted');
}

$('new-book').addEventListener('click', () => {
  const own = libs.find((l) => !l.reader && !l.writer);
  if (!own) return;
  const dlg = $('ask');
  $('ask-h').textContent = 'New address book';
  $('ask-text').textContent = '';
  const name = el('input', { id: 'nb-name', autocomplete: 'off' });
  $('ask-body').replaceChildren(el('label', { for: 'nb-name' }, 'Name', name));
  $('ask-actions').replaceChildren(
    el('button', { type: 'button', class: 'quiet', onclick: () => dlg.close() }, 'Cancel'),
    el('button', { type: 'button', onclick: async () => {
      if (!name.value.trim()) { name.focus(); return; }
      dlg.close();
      try {
        const b = await make(own, 'Contacts', name.value.trim(), '');
        books.push(b); files.set(bookId(b), []);
        book = bookId(b);
        drawBooks(); drawList(); say('Made');
      } catch (err) { say(`Not made: ${err.message}`); }
    } }, 'Make'));
  dlg.onclose = null;
  dlg.showModal();
  name.focus();
});

$('import').addEventListener('click', () => $('import-file').click());
$('import-file').addEventListener('change', async (e) => {
  const chosen = [...e.target.files];
  e.target.value = '';
  const target = books.find((b) => bookId(b) === book && b.writable) || books.find((b) => b.writable && b.mine) || books.find((b) => b.writable);
  if (!chosen.length || !target) return;
  const cards = [];
  for (const f of chosen) cards.push(...I.parse(await f.text()).filter((c) => c.name === 'VCARD'));
  let added = 0, there = 0, failed = 0;
  const work = [...cards];
  const one = async () => {
    while (work.length) {
      const c = work.shift();
      let uid = I.text(I.prop(c, 'UID'));
      if (!uid) { uid = I.newUid(); c.props.push({ name: 'UID', params: {}, value: uid }); }
      try {
        const r = await save(target, fileName(uid, '.vcf'), I.serialize(c), 'new');
        if (r.changed !== undefined) there++; else added++;
      } catch { failed++; }
      say(`Importing: ${added + there + failed} of ${cards.length}…`);
    }
  };
  await Promise.all(Array.from({ length: 6 }, one));
  files.set(bookId(target), await items(target, '.vcf'));
  rebuild(); drawBooks(); drawList();
  say(`Imported ${added} into ${target.name}` + (there ? `; ${there} were there already` : '') + (failed ? `; ${failed} could not be saved` : '') + '.');
});

$('export').addEventListener('click', () => {
  const list = visible();
  if (!list.length) { say('Nothing to export here.'); return; }
  const text = list.map((p) => I.serialize(p.comp)).join('');
  const name = book === 'all' ? 'contacts.vcf' : `${(books.find((b) => bookId(b) === book)?.name || 'contacts').replace(/[^\w -]/g, '')}.vcf`;
  download(new Blob([text], { type: 'text/vcard' }), name);
});

$('new-person').addEventListener('click', () => openEdit(null));
$('search').addEventListener('input', (e) => { q = e.target.value.trim(); drawList(); });

async function start() {
  $('body').hidden = false;
  let lib;
  try {
    const [m, u] = await Promise.all([me().catch(() => null), unlock()]);
    forAccount(m?.user);
    lib = u;
  } catch (e) { $('body').hidden = true; say(`Your contacts did not open: ${e.message}`); return; }
  if (lib.app) { $('body').hidden = true; $('link').hidden = false; return; }
  if (lib.none) { $('body').hidden = true; say('You have no library yet. The app makes one when you sign in on it.'); return; }
  const fill = async (b, draw) => {
    const id = bookId(b);
    try { files.set(id, await items(b, '.vcf', (kept) => { if (!files.has(id)) { files.set(id, kept); draw(); } })); }
    catch (e) { say(`${b.name} did not open: ${e.message}`); }
  };
  const redraw = () => { rebuild(); drawBooks(); drawList(); };
  let got;
  try {
    got = await collections('Contacts', (kept) => { if (!books.length) { books = kept; redraw(); Promise.all(books.map((b) => fill(b, redraw))).then(redraw); } });
  } catch (e) { say(`Your contacts did not open: ${e.message}`); return; }
  libs = got.libs;
  books = got.list;
  if (!books.some((b) => b.mine) && !lib.ok.demo && !lib.ok.reader) {
    try { const b = await first(libs, 'Contacts', books, 'Contacts'); if (b) books.unshift(b); } catch { /* made next time */ }
  }
  $('actions').hidden = !books.some((b) => b.writable);
  $('new-book').hidden = !libs.some((l) => !l.reader && !l.writer) || !!lib.ok.demo;
  redraw();
  await Promise.all(books.map((b) => fill(b, redraw)));
  redraw();
}

start();
