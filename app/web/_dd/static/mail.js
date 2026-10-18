// Mail (docs/dev/mail.md): the member's locked mail, opened on this device
// with the key their app holds. The mailbox (mail-mailbox.js) speaks to
// the box and the app; this is what it looks like. A message's own HTML is
// drawn in a frame that runs nothing and loads nothing from the internet
// until asked.

import { me, inApp, servedByApp, appFetch } from './shell.js';
import { Mailbox } from './mail-mailbox.js';
import * as hpke from './mail-hpke.js';

const $ = (id) => document.getElementById(id);
const esc = (s) => String(s ?? '').replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' })[c]);
const viaApp = (path, body) => {
  const init = body === undefined ? {} : { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) };
  return inApp ? appFetch(path, init) : fetch(path, init);
};
async function appJson(path, body) {
  const r = await viaApp(path, body);
  if (!r.ok) {
    const e = new Error((await r.text().catch(() => '')) || `the app said ${r.status}`);
    e.status = r.status;
    throw e;
  }
  return r.json();
}

const IC = {
  inbox: '<path d="M3 13h5l1.5 3h5L16 13h5M5 5h14l2 8v6H3v-6z"/>',
  star: '<path d="m12 4 2.5 5.2 5.5.8-4 3.9 1 5.6-5-2.7-5 2.7 1-5.6-4-3.9 5.5-.8z"/>',
  sent: '<path d="M4 12 20 4l-6 16-3-7z"/>',
  drafts: '<path d="M4 20h4L19 9l-4-4L4 16zM14 6l4 4"/>',
  archive: '<path d="M3 5h18v4H3zM5 9v10h14V9M10 13h4"/>',
  spam: '<path d="M12 3 3 19h18zM12 10v4M12 17h.01"/>',
  trash: '<path d="M4 7h16M9 7V4h6v3M6 7l1 13h10l1-13"/>',
  lock: '<rect x="5" y="11" width="14" height="9" rx="1.5"/><path d="M8 11V8a4 4 0 0 1 8 0v3"/>',
  arrow: '<path d="M5 12h14M13 6l6 6-6 6"/>',
  reply: '<path d="M10 7 4 12l6 5M4 12h10a6 6 0 0 1 6 6"/>',
  replyall: '<path d="M8 7 3 12l5 5M13 7l-5 5 5 5M8 12h6a6 6 0 0 1 6 6"/>',
  fwd: '<path d="m14 7 6 5-6 5M20 12H10a6 6 0 0 0-6 6"/>',
  back: '<path d="M19 12H5M11 6l-6 6 6 6"/>',
  clip: '<path d="m20 11-8.5 8.5a5 5 0 0 1-7-7L13 4a3.5 3.5 0 0 1 5 5l-8.5 8.5a2 2 0 0 1-3-3L14 7"/>',
  unread: '<rect x="3" y="5" width="18" height="14" rx="1.5"/><path d="m3.5 6.5 8.5 7 8.5-7"/>',
  undo: '<path d="M9 14 4 9l5-5M4 9h10a6 6 0 0 1 0 12h-3"/>', redo: '<path d="m15 14 5-5-5-5M20 9H10a6 6 0 0 0 0 12h3"/>',
  code: '<path d="m8 8-4 4 4 4M16 8l4 4-4 4"/>',
  al: '<path d="M4 6h16M4 10h10M4 14h16M4 18h10"/>', ac: '<path d="M4 6h16M7 10h10M4 14h16M7 18h10"/>',
  ar: '<path d="M4 6h16M10 10h10M4 14h16M10 18h10"/>', aj: '<path d="M4 6h16M4 10h16M4 14h16M4 18h16"/>',
  ul: '<path d="M9 6h11M9 12h11M9 18h11"/><circle cx="4.5" cy="6" r="1"/><circle cx="4.5" cy="12" r="1"/><circle cx="4.5" cy="18" r="1"/>',
  ol: '<path d="M10 6h10M10 12h10M10 18h10M4 5l1.5-1V9M3.5 13.5a1.5 1.5 0 1 1 2.5 1L3.5 17H6"/>',
  ck: '<path d="M11 7h9M11 17h9"/><rect x="3" y="4" width="5" height="5" rx="1"/><rect x="3" y="14" width="5" height="5" rx="1"/><path d="m4 16.5 1 1 2-2"/>',
  out: '<path d="M11 6h9M11 12h9M11 18h9M7 9l-3 3 3 3"/>', in: '<path d="M11 6h9M11 12h9M11 18h9M4 9l3 3-3 3"/>',
  link: '<path d="M10 14a4 4 0 0 0 5.7 0l3-3a4 4 0 0 0-5.7-5.7l-1 1M14 10a4 4 0 0 0-5.7 0l-3 3a4 4 0 0 0 5.7 5.7l1-1"/>',
  img: '<rect x="3" y="5" width="18" height="14" rx="1.5"/><circle cx="9" cy="10" r="1.5"/><path d="m21 16-5-5-8 8"/>',
  tbl: '<rect x="3" y="5" width="18" height="14" rx="1"/><path d="M3 10h18M3 15h18M9 5v14M15 5v14"/>',
  hr: '<path d="M3 12h18"/>',
  quote: '<path d="M6 17h3l2-4V7H5v6h3M14 17h3l2-4V7h-6v6h3"/>',
  sig: '<path d="M3 17c3-1 4-9 6-9s0 8 2 8 2-4 4-4 1 3 3 3M3 21h18"/>',
  clr: '<path d="M6 5h12M12 5l-3 14M15 13l5 5M20 13l-5 5"/>',
};
const svg = (k, w = 16) => `<svg viewBox="0 0 24 24" width="${w}" height="${w}" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${IC[k]}</svg>`;

function idb(name) {
  const opened = new Promise((ok, bad) => {
    const r = indexedDB.open(name, 1);
    r.onupgradeneeded = () => r.result.createObjectStore('kv');
    r.onsuccess = () => ok(r.result);
    r.onerror = () => bad(r.error);
  });
  const tx = async (mode, f) => {
    const db = await opened;
    return new Promise((ok, bad) => {
      const t = db.transaction('kv', mode);
      const out = f(t.objectStore('kv'));
      t.oncomplete = () => ok(out && 'result' in out ? out.result : undefined);
      t.onerror = () => bad(t.error);
    });
  };
  return {
    get: (k) => tx('readonly', (s) => s.get(k)),
    put: (k, v) => tx('readwrite', (s) => s.put(v, k)),
    del: (k) => tx('readwrite', (s) => s.delete(k)),
    all: async (prefix) => {
      const db = await opened;
      return new Promise((ok, bad) => {
        const out = [];
        const t = db.transaction('kv', 'readonly');
        const c = t.objectStore('kv').openCursor(IDBKeyRange.bound(prefix, `${prefix}￿`));
        c.onsuccess = () => { const cur = c.result; if (cur) { out.push(cur.value); cur.continue(); } };
        t.oncomplete = () => ok(out);
        t.onerror = () => bad(t.error);
      });
    },
  };
}

const api = {
  async list(kind, after) {
    const r = await fetch(`/_dd/mail/list/${kind}?after=${encodeURIComponent(after || '')}`);
    if (!r.ok) throw new Error(`the box said ${r.status}`);
    return r.json();
  },
  async get(kind, id, range) {
    const r = await fetch(`/_dd/mail/o/${kind}/${id}`, range ? { headers: { range } } : {});
    if (!r.ok) throw new Error(`the box said ${r.status}`);
    return { bytes: new Uint8Array(await r.arrayBuffer()), from: r.headers.get('x-dd-from') };
  },
  async put(kind, id, bytes) {
    const r = await fetch(`/_dd/mail/o/${kind}/${id}`, { method: 'PUT', headers: { 'content-type': 'application/octet-stream' }, body: bytes });
    if (!r.ok) throw new Error(r.status === 507 ? 'your storage is full' : `the box said ${r.status}`);
  },
  async del(kind, id) { await fetch(`/_dd/mail/o/${kind}/${id}`, { method: 'DELETE' }); },
  async deliver(to, id, bytes) {
    const r = await fetch(`/_dd/mail/deliver/${encodeURIComponent(to)}/${id}`, { method: 'POST', headers: { 'content-type': 'application/octet-stream' }, body: bytes });
    if (!r.ok) throw new Error(await r.text() || `the box said ${r.status}`);
  },
  key: (name) => appJson('/_dd/app/mail/worker', { method: 'GET', path: `/api/mailkey/${encodeURIComponent(name)}` }),
  vouch: (v) => appJson('/_dd/app/mail/vouch', v),
  send: (p) => appJson('/_dd/app/mail/send', p),
};

let mb = null;
let user = '';
let domain = '';
let addrs = { address: '', aliases: [] };
let folder = 'inbox';
let only = null;
let pick = null;
let query = '';
let editing = null;
let pics = false;
const FOLDERS = [['inbox', 'Inbox'], ['starred', 'Starred'], ['sent', 'Sent'], ['drafts', 'Drafts'], ['archive', 'Archive'], ['spam', 'Spam'], ['trash', 'Trash']];

let toastT;
function toast(s) {
  const t = $('toast');
  t.textContent = s;
  t.hidden = false;
  clearTimeout(toastT);
  toastT = setTimeout(() => { t.hidden = true; }, 3200);
}
function when(m) {
  const t = new Date(m.s.date || m.s.at || Date.now());
  const now = new Date();
  if (t.toDateString() === now.toDateString()) return t.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  if (t.getFullYear() === now.getFullYear()) return t.toLocaleDateString([], { month: 'short', day: 'numeric' });
  return t.toISOString().slice(0, 10);
}
const who = (m) => {
  if (m.kind === 'sent' || m.kind === 'drafts') {
    const to = (m.s.to || []).map((a) => a.name || a.address).join(', ');
    return to ? `To: ${to}` : '(nobody yet)';
  }
  return m.s.from?.name || m.s.from?.address || '(nobody)';
};
const mine = (a) => { const x = String(a || '').toLowerCase(); return x === addrs.address || addrs.aliases.some((y) => y.address === x); };

function drawSide() {
  $('folders').innerHTML = FOLDERS.map(([k, n]) => {
    const c = k === 'starred' ? mb.folder('starred').length : k === 'inbox' || k === 'spam' ? mb.unread(k) : k === 'drafts' ? mb.folder('drafts').length : 0;
    return `<li><button type="button" data-f="${k}" aria-current="${folder === k && !query}">${svg(k === 'starred' ? 'star' : k)}${n}${c ? `<span class="mail-count">${c}</span>` : ''}</button></li>`;
  }).join('');
  const list = [{ address: addrs.address, label: 'Your address' }, ...addrs.aliases];
  $('addrs').innerHTML = `<li><button type="button" data-a="" aria-current="${!only}">All addresses</button></li>` + list.filter((x) => x.address).map((x) => `<li><button type="button" data-a="${esc(x.address)}" aria-current="${only === x.address}" title="${esc(x.label || x.address)}"><span class="mono">${esc(x.address.replace(`@${domain}`, '@…'))}</span><span class="mail-count">${esc(x.label || '')}</span></button></li>`).join('');
}

function chips(m) {
  const t = [];
  if (m.by === 'member') t.push(`<span class="chip e2e">${svg('lock', 11)}End to end</span>`);
  if (m.s.alias) t.push(`<span class="chip alias">${esc(m.s.alias)}</span>`);
  if (m.s.attachments?.length) t.push(`<span class="chip alias">${svg('clip', 11)}${m.s.attachments.length}</span>`);
  return t.join('');
}
let shown = [];
let more = false;
async function drawRows() {
  if (query) {
    const r = await mb.search(query, 100);
    shown = r.hits;
    more = r.more;
  } else {
    shown = mb.folder(folder, only);
    more = false;
  }
  const rows = shown.slice(0, 500);
  $('rows').innerHTML = rows.length
    ? rows.map((m) => `<div class="mail-row${m.read ? '' : ' unread'}" role="option" tabindex="0" data-id="${m.id}" aria-selected="${m.id === pick}">
        <span class="who">${esc(who(m))}</span><time>${esc(when(m))}</time>
        <span class="subj">${esc(m.s.subject || '(no subject)')}</span><span class="snip">${esc(m.s.snippet || '')}</span>
        <span class="tags">${chips(m)}</span></div>`).join('') + (more || shown.length > 500 ? '<div class="mail-more">Showing the newest. Add a word to narrow it.</div>' : '')
    : `<div class="mail-empty">${query ? 'Nothing matches.' : folder === 'inbox' ? 'No mail yet.' : 'Nothing here.'}</div>`;
}

function note(m) {
  if (m.kind === 'in' && m.by === 'member') {
    return `<div class="mail-note e2e">${svg('lock', 18)}<p><b>End to end.</b> Locked on ${esc(m.sender)}'s device to your key, and signed by it. Nobody else read it, not Cloudflare and not a box.</p></div>`;
  }
  if (m.kind === 'in' && m.s.imported) return `<div class="mail-note in">${svg('lock', 18)}<p><b>Brought in from your old mail.</b> Locked on this device before it went to your box.</p></div>`;
  if (m.kind === 'in') return `<div class="mail-note in">${svg('lock', 18)}<p><b>Locked when it arrived.</b> It came from outside Commonty, so Cloudflare read it once as it came in. Since then only your devices can open it.</p></div>`;
  if (m.kind === 'sent') {
    const bad = (m.s.results || []).filter((r) => !r.ok);
    return `<div class="mail-note in">${svg('arrow', 18)}<p><b>Sent from your device through your box.</b> ${(m.s.results || []).some((r) => r.e2e) ? 'To members, locked end to end. ' : ''}Mail to other domains was read by nobody on the way but their own server; your box saw only which server and when.${bad.length ? ` <b>Not delivered:</b> ${bad.map((r) => esc(`${r.to} (${r.why})`)).join(', ')}` : ''}</p></div>`;
  }
  return '';
}
const spamNote = (m) => (m.folder === 'spam' ? `<div class="mail-note warn">${svg('spam', 18)}<p><b>In Spam.</b> ${m.s.auth?.dmarc === 'fail' ? 'Its sender failed the checks its own domain publishes, so it is not who it says.' : 'Your filter put it here, from what you marked before.'}</p></div>` : '');

// a message's HTML, without anything that runs, submits or reaches out
function clean(html, inline, showPics) {
  const doc = new DOMParser().parseFromString(html, 'text/html');
  for (const e of doc.querySelectorAll('script,iframe,frame,frameset,object,embed,applet,form,input,button,textarea,select,base,meta,link,noscript,template')) e.remove();
  for (const e of doc.querySelectorAll('*')) {
    for (const a of [...e.attributes]) {
      const n = a.name.toLowerCase();
      if (n.startsWith('on') || n === 'srcdoc' || n === 'formaction' || n === 'ping') e.removeAttribute(a.name);
      else if ((n === 'href' || n === 'src' || n === 'xlink:href' || n === 'background' || n === 'poster') && /^\s*(javascript|vbscript|data:text)/i.test(a.value)) e.removeAttribute(a.name);
    }
  }
  let blocked = 0;
  for (const img of doc.querySelectorAll('img, [background]')) {
    const attr = img.hasAttribute('src') ? 'src' : 'background';
    const src = img.getAttribute(attr) || '';
    if (/^cid:/i.test(src)) {
      const part = inline[src.slice(4).replace(/^<|>$/g, '')];
      if (part) img.setAttribute(attr, URL.createObjectURL(new Blob([part.body], { type: part.type })));
      else img.removeAttribute(attr);
    } else if (/^https?:/i.test(src) && !showPics) {
      img.removeAttribute(attr);
      // nothing in its place: no broken picture, and no size to give it away
      if (img.tagName === 'IMG') img.setAttribute('hidden', '');
      blocked++;
    }
  }
  for (const a of doc.querySelectorAll('a[href]')) { a.setAttribute('target', '_blank'); a.setAttribute('rel', 'noopener noreferrer'); }
  // nor a picture from the internet by way of a style
  if (!showPics) {
    const away = /url\(\s*['"]?\s*(https?:|\/\/)/i;
    for (const e of doc.querySelectorAll('[style]')) if (away.test(e.getAttribute('style'))) { e.setAttribute('style', e.getAttribute('style').replace(/url\([^)]*\)/gi, 'none')); blocked++; }
    for (const st of doc.querySelectorAll('style')) if (away.test(st.textContent) || /@import/i.test(st.textContent)) { st.textContent = st.textContent.replace(/@import[^;]*;?/gi, '').replace(/url\([^)]*\)/gi, 'none'); blocked++; }
  }
  return { html: doc.body.innerHTML, head: [...doc.querySelectorAll('style')].map((s) => s.outerHTML).join(''), blocked };
}

async function drawRead() {
  if (editing) return drawEditor();
  const m = mb?.msgs.get(pick);
  if (!m) { $('read').innerHTML = '<div class="mail-empty">No message open.</div>'; return; }
  if (!m.read) { mb.change([m.id], { read: true }).then(() => { drawSide(); drawRows(); }); }
  const al = m.s.alias ? addrs.aliases.find((a) => a.address.startsWith(`${m.s.alias}@`)) : null;
  const toLine = m.kind === 'in'
    ? `to <span class="mono">${esc(m.s.rcpt || (m.s.to || []).map((a) => a.address).join(', '))}</span>${al?.label ? ` (${esc(al.label)})` : ''}${m.s.cc?.length ? `, cc <span class="mono">${esc(m.s.cc.map((a) => a.address).join(', '))}</span>` : ''}`
    : `to <span class="mono">${esc([...(m.s.to || []), ...(m.s.cc || [])].map((a) => a.address).join(', '))}</span>${m.s.bcc?.length ? `, bcc <span class="mono">${esc(m.s.bcc.join(', '))}</span>` : ''}`;
  const name = (m.s.from?.name || m.s.from?.address || '?').replace(/^"/, '');
  $('read').innerHTML = `
    <div class="mail-tools" role="toolbar" aria-label="Message">
      <button class="ib back" type="button" data-act="back" aria-label="Back to the list">${svg('back', 18)}</button>
      <span class="grow"></span>
      ${m.kind === 'drafts' ? `<button class="ib" type="button" data-act="edit">${svg('drafts', 18)}<span class="lbl">Edit</span></button>` : `
      <button class="ib" type="button" data-act="reply" data-guide="mail-reply">${svg('reply', 18)}<span class="lbl">Reply</span></button>
      <button class="ib" type="button" data-act="replyall">${svg('replyall', 18)}<span class="lbl">Reply all</span></button>
      <button class="ib" type="button" data-act="forward">${svg('fwd', 18)}<span class="lbl">Forward</span></button>`}
      <span class="sep"></span>
      <button class="ib${m.star ? ' on' : ''}" type="button" data-act="star" aria-pressed="${m.star}" aria-label="${m.star ? 'Unstar' : 'Star'}" title="${m.star ? 'Unstar' : 'Star'}">${svg('star', 18)}</button>
      <button class="ib" type="button" data-act="unread" aria-label="Mark unread" title="Mark unread">${svg('unread', 18)}</button>
      ${m.folder === 'inbox' ? `<button class="ib" type="button" data-act="archive" aria-label="Archive" title="Archive">${svg('archive', 18)}</button>` : ''}
      ${m.folder === 'spam' ? `<button class="ib" type="button" data-act="notspam">${svg('inbox', 18)}<span class="lbl">Not spam</span></button>` : m.kind === 'in' && m.folder !== 'trash' ? `<button class="ib" type="button" data-act="spam" aria-label="Spam" title="Spam" data-guide="mail-spam">${svg('spam', 18)}</button>` : ''}
      ${m.folder === 'trash' ? `<button class="ib" type="button" data-act="restore">${svg('inbox', 18)}<span class="lbl">Put back</span></button><button class="ib" type="button" data-act="gone">${svg('trash', 18)}<span class="lbl">Delete for good</span></button>` : `<button class="ib" type="button" data-act="trash" aria-label="Delete" title="Delete">${svg('trash', 18)}</button>`}
    </div>
    <div class="mail-notes">${note(m)}${spamNote(m)}</div>
    <div class="mail-paper">
      <h1>${esc(m.s.subject || '(no subject)')}</h1>
      <div class="mail-meta">
        <span class="av">${esc(name[0]?.toUpperCase() || '?')}</span>
        <span class="from"><b>${esc(m.s.from?.name || m.s.from?.address || '')}</b> <span class="mono">&lt;${esc(m.s.from?.address || '')}&gt;</span></span>
        <time>${esc(when(m))}</time>
        <span class="to" style="grid-column: 2 / -1">${toLine}</span>
      </div>
      <div class="mail-body" id="body"><p class="mail-wait"><i aria-hidden="true"></i>Opening…</p></div>
      <div class="mail-att" id="att"></div>
    </div>`;
  if (m.kind === 'drafts') { $('body').innerHTML = `<pre class="plain">${esc(m.s.snippet || '')}</pre>`; return; }
  try {
    const r = await mb.read(m.id);
    if (pick !== m.id || editing) return;
    drawBody(r);
  } catch (err) {
    $('body').innerHTML = `<p class="mail-bad">${esc(err.message || err)}</p>`;
  }
}

function drawBody(r) {
  const body = $('body');
  if (r.html) {
    const c = clean(r.html, r.inline, pics);
    body.innerHTML = `${c.blocked && !pics ? `<div class="mail-pics">Pictures from the internet are not shown: loading them tells the sender you opened it. <button type="button" class="quiet" data-act="pics">Show pictures</button></div>` : ''}<iframe title="The message" sandbox="allow-same-origin allow-popups allow-popups-to-escape-sandbox"></iframe>`;
    const f = body.querySelector('iframe');
    const policy = `default-src 'none'; img-src data: blob:${pics ? ' https: http:' : ''}; style-src 'unsafe-inline'; font-src data:`;
    f.srcdoc = `<!doctype html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="${policy}"><base target="_blank"><style>html{background:#fff}body{margin:0;padding:14px 16px;background:#fff;color:#1a1f1c;font:15px/1.55 \"Segoe UI\",Helvetica,Arial,\"DejaVu Sans\",sans-serif;overflow-wrap:anywhere}img{max-width:100%;height:auto}table{max-width:100%}</style>${c.head}</head><body>${c.html}</body></html>`;
    const fit = () => { try { f.style.height = `${f.contentDocument.documentElement.scrollHeight + 8}px`; } catch {} };
    f.onload = () => { fit(); setTimeout(fit, 300); };
  } else {
    body.innerHTML = `<pre class="plain">${esc(r.plain || '')}</pre>`;
  }
  $('att').innerHTML = '';
  for (const a of r.attachments) {
    const name = a.filename || (a.type === 'message/rfc822' ? 'message.eml' : 'attachment');
    const link = document.createElement('a');
    link.href = URL.createObjectURL(new Blob([a.body], { type: 'application/octet-stream' }));
    link.download = name;
    link.innerHTML = `${svg('clip', 14)}${esc(name)} · ${esc(size(a.body.length))}`;
    $('att').append(link);
  }
}
const size = (n) => (n < 1024 ? `${n} B` : n < 1 << 20 ? `${(n / 1024).toFixed(0)} KB` : `${(n / (1 << 20)).toFixed(1)} MB`);

const FMT = [
  ['undo', 'Undo', 'undo'], ['redo', 'Redo', 'redo'], '|',
  ['@block'], ['@font'], ['@size'], '|',
  ['bold', 'Bold', 'B'], ['italic', 'Italic', 'I'], ['underline', 'Underline', 'U'], ['strikeThrough', 'Strikethrough', 'S'],
  ['superscript', 'Superscript', 'x²'], ['subscript', 'Subscript', 'x₂'], ['@code', 'Code', 'code'], '|',
  ['@color'], ['@mark'], '|',
  ['justifyLeft', 'Align left', 'al'], ['justifyCenter', 'Center', 'ac'], ['justifyRight', 'Align right', 'ar'], ['justifyFull', 'Justify', 'aj'], '|',
  ['insertUnorderedList', 'Bulleted list', 'ul'], ['insertOrderedList', 'Numbered list', 'ol'], ['@check', 'Checklist', 'ck'], ['outdent', 'Decrease indent', 'out'], ['indent', 'Increase indent', 'in'], '|',
  ['@quote', 'Quote', 'quote'], ['@link', 'Link', 'link'], ['@image', 'Picture', 'img'], ['@table', 'Table', 'tbl'], ['insertHorizontalRule', 'Line', 'hr'], ['@sig', 'Signature', 'sig'], '|',
  ['removeFormat', 'Clear formatting', 'clr'],
];
function fmtBar() {
  return FMT.map((x) => {
    if (x === '|') return '<span class="sep"></span>';
    const [cmd, name, g] = x;
    if (cmd === '@block') return '<select data-fmt="block" aria-label="Paragraph style"><option value="p">Normal text</option><option value="h1">Heading 1</option><option value="h2">Heading 2</option><option value="h3">Heading 3</option><option value="blockquote">Quote</option><option value="pre">Code block</option></select>';
    if (cmd === '@font') return '<select data-fmt="font" aria-label="Font"><option value="Public Sans">Sans</option><option value="Georgia">Serif</option><option value="IBM Plex Mono">Monospace</option><option value="Arial">Arial</option><option value="Times New Roman">Times New Roman</option><option value="Verdana">Verdana</option><option value="Trebuchet MS">Trebuchet</option><option value="Garamond">Garamond</option><option value="Courier New">Courier New</option></select>';
    if (cmd === '@size') return '<select data-fmt="size" aria-label="Size"><option value="2">Small</option><option value="3" selected>Normal</option><option value="4">Large</option><option value="5">Larger</option><option value="7">Huge</option></select>';
    if (cmd === '@color') return '<label class="ib clr" title="Text colour"><b>A</b><i style="background:#B3401F"></i><input type="color" data-fmt="color" value="#B3401F" aria-label="Text colour"></label>';
    if (cmd === '@mark') return '<label class="ib clr" title="Highlight"><span>ab</span><i style="background:#F2D24B"></i><input type="color" data-fmt="mark" value="#F2D24B" aria-label="Highlight colour"></label>';
    const face = IC[g] ? svg(g) : `<span style="font-weight:${g === 'B' ? 700 : 500};${g === 'I' ? 'font-style:italic;' : ''}${g === 'U' ? 'text-decoration:underline;' : ''}${g === 'S' ? 'text-decoration:line-through;' : ''}font-size:14px">${g}</span>`;
    return `<button class="ib" type="button" data-cmd="${cmd}" aria-label="${name}" title="${name}">${face}</button>`;
  }).join('');
}

// HTML for the editor, which is this page's own document: rebuilt node by
// node from tags and attributes known to be inert, and handed over as nodes,
// never as a string the page would parse again
const SAFE_TAGS = new Set(['P', 'BR', 'DIV', 'SPAN', 'B', 'STRONG', 'I', 'EM', 'U', 'S', 'STRIKE', 'SUB', 'SUP', 'A', 'UL', 'OL', 'LI', 'BLOCKQUOTE', 'PRE', 'CODE', 'H1', 'H2', 'H3', 'H4', 'H5', 'H6', 'HR', 'TABLE', 'THEAD', 'TBODY', 'TR', 'TD', 'TH', 'IMG', 'FONT', 'SMALL', 'BIG']);
function safeNodes(html) {
  const src = new DOMParser().parseFromString(String(html), 'text/html').body;
  const out = document.createDocumentFragment();
  const copy = (from, into) => {
    for (const n of from.childNodes) {
      if (n.nodeType === Node.TEXT_NODE) { into.append(document.createTextNode(n.nodeValue)); continue; }
      if (n.nodeType !== Node.ELEMENT_NODE) continue;
      if (!SAFE_TAGS.has(n.tagName)) { copy(n, into); continue; }
      const e = document.createElement(n.tagName.toLowerCase());
      const href = n.getAttribute('href');
      if (n.tagName === 'A' && href && /^(https?:|mailto:)/i.test(href)) { e.setAttribute('href', href); e.setAttribute('rel', 'noopener noreferrer'); e.setAttribute('target', '_blank'); }
      const imgSrc = n.getAttribute('src');
      if (n.tagName === 'IMG') { if (imgSrc && /^(data:image\/|blob:)/i.test(imgSrc)) e.setAttribute('src', imgSrc); else continue; }
      for (const a of ['colspan', 'rowspan', 'alt']) if (n.hasAttribute(a)) e.setAttribute(a, n.getAttribute(a).slice(0, 200));
      if (n.tagName === 'FONT' && /^#[0-9a-f]{3,6}$/i.test(n.getAttribute('color') || '')) e.setAttribute('color', n.getAttribute('color'));
      copy(n, e);
      into.append(e);
    }
  };
  copy(src, out);
  return out;
}

function quoteOf(m, r, forward) {
  const head = forward
    ? `<p>---------- Forwarded message ----------<br>From: ${esc(m.s.from?.name || '')} &lt;${esc(m.s.from?.address || '')}&gt;<br>Date: ${esc(new Date(m.s.date || m.s.at).toUTCString())}<br>Subject: ${esc(m.s.subject || '')}<br>To: ${esc((m.s.to || []).map((a) => a.address).join(', '))}</p>`
    : `<p>On ${esc(new Date(m.s.date || m.s.at).toLocaleString())}, ${esc(m.s.from?.name || m.s.from?.address || '')} wrote:</p>`;
  const inner = r.html ? clean(r.html, r.inline, false).html : `<p>${esc(r.plain || '').replace(/\r?\n/g, '<br>')}</p>`;
  return `<p><br></p>${head}<blockquote>${inner}</blockquote>`;
}

async function compose(m, how) {
  let quote = '';
  let attachments = [];
  if (m && how !== 'edit') {
    try {
      const r = await mb.read(m.id);
      quote = quoteOf(m, r, how === 'forward');
      if (how === 'forward') attachments = r.attachments.map((a) => ({ name: a.filename || 'attachment', type: a.type, bytes: a.body }));
    } catch { /* quoted without its body */ }
  }
  const from = m && m.kind === 'in' ? (m.s.rcpt && mine(m.s.rcpt) ? m.s.rcpt : m.s.alias ? `${m.s.alias}@${domain}` : addrs.address) : addrs.address;
  const replyTo = m?.s.replyTo?.address || m?.s.from?.address || '';
  const everyone = m ? [...(m.s.to || []), ...(m.s.cc || [])].map((a) => a.address).filter((a) => !mine(a) && a !== replyTo) : [];
  const subj = (m?.s.subject || '').replace(/^((re|fwd?|aw):\s*)+/i, '');
  // a draft's own HTML is cleaned like any message's before it is edited
  editing = how === 'edit' ? { ...m.s.draft, html: clean(String(m.s.draft?.html || ''), {}, false).html, draftId: m.id, attachments: [] } : {
    from,
    to: how === 'forward' || !m ? '' : m.kind === 'sent' ? (m.s.to || []).map((a) => a.address).join(', ') : replyTo,
    cc: how === 'replyall' ? everyone.join(', ') : '',
    bcc: '',
    subject: !m ? '' : how === 'forward' ? `Fwd: ${subj}` : `Re: ${subj}`,
    html: quote,
    attachments,
    reply: m && how !== 'forward' ? { mid: m.s.mid, refs: m.s.refs } : null,
    note: m && how !== 'forward' && from !== addrs.address ? 'Sending from the alias it came to' : '',
  };
  $('app').classList.add('reading');
  drawEditor();
}

const split = (s) => String(s || '').split(/[,;\s]+/).map((x) => x.trim()).filter(Boolean);

function drawEditor() {
  const e = editing;
  const froms = [{ address: addrs.address, label: 'your address' }, ...addrs.aliases];
  $('read').innerHTML = `
    <form id="ed-f" autocomplete="off">
      <div class="mail-tools mail-edtools" role="toolbar" aria-label="New email">
        <button class="ib back" type="button" data-act="close" aria-label="Back">${svg('back', 18)}</button>
        <div class="mail-fmt" role="toolbar" aria-label="Formatting" data-guide="mail-format">${fmtBar()}</div>
        <div class="mail-edright">
          <button class="ib" type="button" data-act="attach">${svg('clip', 18)}<span class="lbl">Attach</span></button>
          <button class="ib" type="button" data-act="discard" aria-label="Discard" title="Discard">${svg('trash', 18)}</button>
          <button class="ib primary" type="submit" id="send">${svg('sent', 16)}Send</button>
        </div>
      </div>
      <div class="mail-notes"><div class="mail-route none" id="route"></div></div>
      <div class="mail-paper mail-ed">
        <div class="f"><label for="c-from">From</label><select id="c-from">${froms.filter((x) => x.address).map((x) => `<option value="${esc(x.address)}"${x.address === e.from ? ' selected' : ''}>${esc(x.address)}${x.label ? ` · ${esc(x.label)}` : ''}</option>`).join('')}</select></div>
        <div class="f to"><label for="c-to">To</label><input id="c-to" type="text" placeholder="name@example.com" value="${esc(e.to)}"><span class="cc"><button type="button" data-act="showcc"${e.cc ? ' hidden' : ''}>Cc</button><button type="button" data-act="showbcc"${e.bcc ? ' hidden' : ''}>Bcc</button></span></div>
        <div class="f" id="row-cc"${e.cc ? '' : ' hidden'}><label for="c-cc">Cc</label><input id="c-cc" type="text" value="${esc(e.cc)}"></div>
        <div class="f" id="row-bcc"${e.bcc ? '' : ' hidden'}><label for="c-bcc">Bcc</label><input id="c-bcc" type="text" placeholder="Hidden from everyone else" value="${esc(e.bcc)}"></div>
        <div class="f"><label for="c-subj">Subject</label><input id="c-subj" class="subj" type="text" value="${esc(e.subject)}"></div>
        <p class="ednote" id="ed-note">${esc(e.note || '')}</p>
        <div class="mail-files" id="files"></div>
        <div class="text" id="c-body" contenteditable="true" role="textbox" aria-multiline="true" aria-label="Message" data-ph="Write your email"></div>
        <input type="file" id="c-attach" multiple hidden>
      </div>
    </form>`;
  $('c-body').replaceChildren(safeNodes(e.html || ''));
  drawFiles();
  for (const id of ['c-to', 'c-cc', 'c-bcc']) $(id).addEventListener('input', route);
  route();
  $('ed-f').addEventListener('submit', send);
  $('c-attach').addEventListener('change', async (ev) => {
    for (const f of ev.target.files) editing.attachments.push({ name: f.name, type: f.type || 'application/octet-stream', bytes: new Uint8Array(await f.arrayBuffer()) });
    ev.target.value = '';
    drawFiles();
  });
  const t = $('c-body');
  try { document.execCommand('styleWithCSS', false, true); } catch {}
  const bar = $('read').querySelector('.mail-fmt');
  bar.addEventListener('mousedown', (ev) => { if (ev.target.closest('button')) ev.preventDefault(); });
  bar.addEventListener('click', (ev) => {
    const b = ev.target.closest('[data-cmd]');
    if (!b) return;
    t.focus();
    const c = b.dataset.cmd;
    const sel = () => getSelection().toString();
    if (c === '@code') return document.execCommand('insertHTML', false, `<code>${esc(sel() || 'code')}</code>`);
    if (c === '@check') return document.execCommand('insertHTML', false, '<p>☐&nbsp;</p>');
    if (c === '@quote') return document.execCommand('formatBlock', false, 'blockquote');
    if (c === '@link') {
      const url = window.prompt?.('Link to', 'https://') || '';
      if (/^(https?:|mailto:)/i.test(url)) document.execCommand('insertHTML', false, `<a href="${esc(url)}">${esc(sel() || url)}</a>`);
      return;
    }
    if (c === '@image') {
      const p = document.createElement('input');
      p.type = 'file';
      p.accept = 'image/*';
      p.onchange = async () => {
        const f = p.files[0];
        if (!f) return;
        const url = await new Promise((ok) => { const r = new FileReader(); r.onload = () => ok(r.result); r.readAsDataURL(f); });
        t.focus();
        document.execCommand('insertHTML', false, `<img src="${url}" alt="${esc(f.name)}" style="max-width:100%">`);
      };
      p.click();
      return;
    }
    if (c === '@table') return document.execCommand('insertHTML', false, '<table><tr><td>&nbsp;</td><td>&nbsp;</td><td>&nbsp;</td></tr><tr><td>&nbsp;</td><td>&nbsp;</td><td>&nbsp;</td></tr></table><p><br></p>');
    if (c === '@sig') return document.execCommand('insertHTML', false, `<p>—<br>${esc(user)}<br>${esc($('c-from').value)}</p>`);
    document.execCommand(c, false, null);
    state();
  });
  bar.addEventListener('change', (ev) => {
    const k = ev.target.dataset.fmt;
    t.focus();
    if (k === 'block') document.execCommand('formatBlock', false, ev.target.value);
    if (k === 'font') document.execCommand('fontName', false, ev.target.value);
    if (k === 'size') document.execCommand('fontSize', false, ev.target.value);
    if (k === 'color' || k === 'mark') {
      document.execCommand(k === 'color' ? 'foreColor' : 'hiliteColor', false, ev.target.value);
      ev.target.previousElementSibling.style.background = ev.target.value;
    }
  });
  const state = () => { for (const c of ['bold', 'italic', 'underline', 'strikeThrough', 'insertUnorderedList', 'insertOrderedList']) { const b = bar.querySelector(`[data-cmd="${c}"]`); try { if (b) b.setAttribute('aria-pressed', document.queryCommandState(c)); } catch {} } };
  t.addEventListener('keyup', state);
  t.addEventListener('mouseup', state);
  // kept as a draft a few seconds after each change
  let saveT;
  const changed = () => { clearTimeout(saveT); saveT = setTimeout(saveDraft, 3000); };
  $('read').querySelector('form').addEventListener('input', changed);
  if (editing.to) { t.focus(); const r = document.createRange(); r.setStart(t, 0); r.collapse(true); getSelection().removeAllRanges(); getSelection().addRange(r); } else $('c-to').focus();
}

function drawFiles() {
  $('files').innerHTML = editing.attachments.map((a, i) => `<span>${svg('clip', 14)}${esc(a.name)} · ${esc(size(a.bytes.length))}<button type="button" data-rm="${i}" aria-label="Remove ${esc(a.name)}">×</button></span>`).join('');
}

function current() {
  return {
    from: $('c-from').value,
    to: split($('c-to').value),
    cc: split($('c-cc').value),
    bcc: split($('c-bcc').value),
    subject: $('c-subj').value,
    html: $('c-body').innerHTML,
  };
}

function route() {
  const all = ['c-to', 'c-cc', 'c-bcc'].flatMap((id) => split($(id)?.value));
  const r = $('route');
  if (!all.length) { r.className = 'mail-route none'; r.innerHTML = `${svg('lock')}<span>Who can read it depends on who it is to.</span>`; return; }
  if (all.every((a) => a.toLowerCase().endsWith(`@${domain}`))) { r.className = 'mail-route e2e'; r.innerHTML = `${svg('lock')}<span><b>End to end.</b> To members with Mail on, your device locks it to each of their keys before it leaves. Nobody else can read it.</span>`; return; }
  r.className = 'mail-route out'; r.innerHTML = `${svg('arrow')}<span><b>Leaves Commonty.</b> Your device sends it encrypted to each recipient's own server, through your box, which sees only which server. Their server reads it, as any mail.</span>`;
}

async function saveDraft() {
  if (!editing) return;
  const d = current();
  if (!d.to.length && !d.subject && !$('c-body').textContent.trim()) return;
  try { editing.draftId = await mb.saveDraft({ ...d, draftId: editing.draftId }); drawSide(); } catch { /* the next change tries again */ }
}

async function send(ev) {
  ev.preventDefault();
  const d = current();
  const bad = [...d.to, ...d.cc, ...d.bcc].find((a) => !/^[^\s<>@,;"]+@[^\s<>@,;"]+\.[^\s<>@,;"]+$/.test(a));
  if (!d.to.length && !d.cc.length && !d.bcc.length) { $('ed-note').textContent = 'Add who it is to, like name@example.com.'; $('c-to').focus(); return; }
  if (bad) { $('ed-note').textContent = `${bad} is not an email address.`; return; }
  $('send').disabled = true;
  $('ed-note').textContent = 'Sending…';
  try {
    const r = await mb.send({ ...d, from: d.from, attachments: editing.attachments, reply: editing.reply, draftId: editing.draftId });
    const failed = r.results.filter((x) => !x.ok);
    editing = null;
    folder = 'sent';
    query = '';
    $('q').value = '';
    pick = mb.folder('sent')[0]?.id || null;
    toast(failed.length ? `Not delivered to ${failed.map((x) => x.to).join(', ')}` : 'Sent');
    drawAll();
  } catch (err) {
    $('ed-note').textContent = String(err.message || err);
    $('send').disabled = false;
  }
}

function drawAll() { drawSide(); drawRows(); drawRead(); }

async function act(name, el) {
  const m = mb.msgs.get(pick);
  if (name === 'back' || name === 'close') { if (name === 'close') await saveDraft(); editing = null; $('app').classList.remove('reading'); drawRead(); return; }
  if (name === 'discard') { const id = editing?.draftId; editing = null; if (id) await mb.discard(id); toast('Discarded'); $('app').classList.remove('reading'); drawAll(); return; }
  if (name === 'attach') { $('c-attach').click(); return; }
  if (name === 'showcc' || name === 'showbcc') { const k = name === 'showcc' ? 'cc' : 'bcc'; $(`row-${k}`).hidden = false; el.hidden = true; $(`c-${k}`).focus(); return; }
  if (name === 'pics') { pics = true; drawRead(); return; }
  if (!m) return;
  if (name === 'reply' || name === 'replyall' || name === 'forward') return compose(m, name);
  if (name === 'edit') return compose(m, 'edit');
  if (name === 'star') await mb.change([m.id], { star: !m.star });
  if (name === 'unread') { await mb.change([m.id], { read: false }); pick = null; $('app').classList.remove('reading'); }
  if (name === 'archive') { await mb.change([m.id], { folder: 'archive' }); toast('Archived'); }
  if (name === 'spam') { await mb.change([m.id], { folder: 'spam' }); toast('Moved to Spam. Your filter learned from it.'); }
  if (name === 'notspam') { await mb.change([m.id], { folder: 'inbox' }); toast('Moved to Inbox. Your filter learned from it.'); }
  if (name === 'trash') { await mb.change([m.id], { folder: 'trash' }); toast('Moved to Trash'); }
  if (name === 'restore') { await mb.change([m.id], { folder: m.kind === 'sent' ? 'sent' : 'inbox' }); toast('Put back'); }
  if (name === 'gone') { await mb.change([m.id], { gone: true }); toast('Deleted'); pick = null; }
  drawAll();
}

document.addEventListener('click', (e) => {
  const f = e.target.closest('[data-f]');
  if (f) { folder = f.dataset.f; query = ''; $('q').value = ''; pick = null; editing = null; $('app').classList.remove('reading'); drawAll(); return; }
  const a = e.target.closest('[data-a]');
  if (a) { only = a.dataset.a || null; pick = null; drawAll(); return; }
  const r = e.target.closest('.mail-row');
  if (r) { if (editing) saveDraft(); editing = null; pick = r.dataset.id; pics = false; $('app').classList.add('reading'); drawRows(); drawRead(); return; }
  const rm = e.target.closest('[data-rm]');
  if (rm && editing) { editing.attachments.splice(Number(rm.dataset.rm), 1); drawFiles(); return; }
  const x = e.target.closest('[data-act]');
  if (x) act(x.dataset.act, x);
});
document.addEventListener('keydown', (e) => { if (e.key === 'Enter' && e.target.classList?.contains('mail-row')) e.target.click(); });

function gate(which) { for (const id of ['g-wait', 'g-app', 'g-on', 'g-confirm']) $(id).hidden = id !== which; $('gate').hidden = !which; }

async function codeOf(name, fp, pk) {
  const d = new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(`${name}\0${fp}\0${pk}`)));
  return [...d.slice(0, 6)].map((b) => b.toString(16).padStart(2, '0')).join('').match(/..../g).join('-');
}

async function registered(name, pk, fp) {
  try {
    const k = await api.key(name);
    return k.pk === pk && k.devices.some((d) => d.fp === fp);
  } catch { return false; }
}

async function start() {
  const who = await me();
  user = who.user;
  domain = new URL(who.home).hostname.split('.').slice(-2).join('.');
  if (!inApp && !servedByApp) return gate('g-app');
  let k;
  try {
    k = await appJson('/_dd/app/mail/key');
  } catch (err) {
    if (err.status !== 404) { gate('g-on'); $('g-on-msg').textContent = String(err.message || err); $('g-on-msg').hidden = false; return; }
    gate('g-on');
    await new Promise((ok) => { $('turn-on').onclick = ok; });
    $('turn-on').disabled = true;
    try { k = await appJson('/_dd/app/mail/key', {}); } catch (e2) { $('g-on-msg').textContent = String(e2.message || e2); $('g-on-msg').hidden = false; $('turn-on').disabled = false; return start(); }
  }
  const pair = await hpke.derive(hpke.b64u(k.ikm));
  const pk = hpke.tob64u(pair.pk);
  const sk = await hpke.privateKey(pair.sk);
  pair.sk.fill(0);
  if (!(await registered(user, pk, k.device.fp))) {
    gate('g-confirm');
    $('g-code').textContent = await codeOf(user, k.device.fp, pk);
    if (k.pk && k.pk !== pk) throw new Error('your app and this page disagree about your mail key');
    const frag = new URLSearchParams({ name: user, pk, fp: k.device.fp, key: k.device.key, sig: k.sig });
    $('confirm').href = `https://mail.${domain}/inbox#${frag}`;
    while (!(await registered(user, pk, k.device.fp))) await new Promise((r) => setTimeout(r, 3000));
  }
  gate(null);
  $('app').hidden = false;
  try { addrs = await appJson('/_dd/app/mail/worker', { method: 'GET', path: '/api/addresses' }); } catch { addrs = { address: `${user}@${domain}`, aliases: [] }; }
  mb = new Mailbox({ api, db: idb(`dd-mail-${user}`), sk, pk: pair.pk, ikm: hpke.b64u(k.ikm), me: user, domain });
  mb.on(() => {});
  await mb.load();
  pick = mb.folder('inbox')[0]?.id || null;
  drawAll();
  const sync = async () => {
    try { if (await mb.sync()) { drawSide(); if (!query) drawRows(); } } catch (err) { console.warn(`mail: ${err.message || err}`); }
  };
  await sync();
  setInterval(sync, 20000);
  window.addEventListener('focus', sync);
}

$('q').addEventListener('input', (e) => { query = e.target.value.trim(); drawSide(); drawRows(); });
$('new').addEventListener('click', () => compose(null, 'new'));
$('new2').addEventListener('click', () => compose(null, 'new'));
$('import').addEventListener('click', () => $('mbox').click());
$('mbox').addEventListener('change', async (e) => {
  const f = e.target.files[0];
  if (!f) return;
  $('import-msg').textContent = `Reading ${f.name}…`;
  try {
    const n = await mb.import(new Uint8Array(await f.arrayBuffer()), (k) => { $('import-msg').textContent = `${k} brought in…`; });
    $('import-msg').textContent = `${n} messages brought in, locked on this device.`;
    drawAll();
  } catch (err) {
    $('import-msg').textContent = String(err.message || err);
  }
  e.target.value = '';
});

start().catch((err) => { gate('g-on'); $('g-on-msg').textContent = String(err.message || err); $('g-on-msg').hidden = false; });
