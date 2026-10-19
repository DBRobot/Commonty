// The PDF editor in Files. Opened from a PDF's preview (Edit, Sign); the
// file is already open in this tab, and what is saved is sealed here before
// it leaves, like any upload. The document itself is pdf-doc.js (MuPDF),
// signatures pdf-sign.js, reading scans pdf-ocr.js; this is the workbench:
// the pages, the tools, and saving back into the library.

import { el } from './kit.js';
import { put, move, trash, fetchPlain, save as toDevice, human } from './library.js';
import { pickFile, pickFolder, saveInto } from './pick.js';
import { Pdf } from './pdf-doc.js';
import * as sig from './pdf-sign.js';

const SIGNING = 'Signing';                       // the library's folder for these, beside Files
const SIGS_FILE = `${SIGNING}/signatures.json`;

const COLORS = [
  ['Ink', [0.11, 0.16, 0.27]], ['Black', [0, 0, 0]], ['Red', [0.7, 0.25, 0.12]], ['Green', [0.11, 0.36, 0.26]], ['Blue', [0.11, 0.31, 0.69]],
];
const MARKERS = [['Yellow', [1, 0.84, 0.04]], ['Green', [0.49, 0.8, 0.64]], ['Pink', [0.95, 0.63, 0.71]], ['Blue', [0.61, 0.76, 0.92]]];
const hex = (c) => `#${c.map((v) => Math.round(v * 255).toString(16).padStart(2, '0')).join('')}`;
const PATTERNS = {
  email: [/[\w.+-]+@[\w-]+(\.[\w-]+)+/g, 'Email addresses'],
  phone: [/(\+?\d{1,2}[\s.-]?)?\(?\d{3}\)?[\s.-]?\d{3}[\s.-]?\d{4}\b/g, 'Phone numbers'],
  account: [/\b(?:\d[ -]?){8,19}\b/g, 'Card and account numbers'],
  ssn: [/\b\d{3}-\d{2}-\d{4}\b/g, 'Social security numbers'],
};

const ICONS = {
  select: '<path d="M5 3l10 6-4.5 1.2L8 15z"/>',
  text: '<path d="M4 5V3h12v2M10 3v14M7 17h6"/>',
  edittext: '<path d="M3 15h6M4 12l4-9 4 9M5.3 9h5.4M13 17l4-4-2-2-4 4v2z"/>',
  highlight: '<path d="M3 17h14M6 13l7-7 3 3-7 7H6z"/>',
  draw: '<path d="M3 15c3-6 5 2 8-3s3-6 6-5"/>',
  shape: '<rect x="3" y="5" width="9" height="9"/><circle cx="14" cy="12" r="3.5"/>',
  note: '<path d="M4 4h12v9H9l-4 3v-3H4z"/>',
  image: '<rect x="3" y="4" width="14" height="12"/><path d="M3 13l4-4 4 4 2-2 4 4"/>',
  form: '<rect x="3" y="4" width="14" height="4"/><rect x="3" y="11" width="14" height="4"/><path d="M5 6h4M5 13h6"/>',
  addfield: '<rect x="3" y="6" width="11" height="6"/><path d="M16 4v6M13 7h6"/>',
  sign: '<path d="M3 15c2-1 3-7 5-7s0 6 2 6 2-3 3-3 1 2 4 2M3 18h14"/>',
  redact: '<rect x="3" y="7" width="14" height="6" fill="currentColor"/>',
  find: '<circle cx="9" cy="9" r="5"/><path d="M13 13l4 4"/>',
  pages: '<rect x="3" y="3" width="9" height="12"/><path d="M8 17h9V6"/>',
};
const TOOLS = [
  ['select', 'Select'], ['text', 'Text'], ['edittext', 'Edit text'], ['highlight', 'Highlight'], ['draw', 'Draw'],
  ['shape', 'Shape'], ['note', 'Comment'], ['image', 'Image'], null,
  ['form', 'Fill form'], ['addfield', 'Add field'], ['sign', 'Sign'], ['redact', 'Redact'], null,
  ['find', 'Find'], ['pages', 'Pages'],
];
const KEYS = { v: 'select', t: 'text', e: 'edittext', h: 'highlight', d: 'draw', s: 'shape', c: 'note', f: 'form', r: 'redact', p: 'pages' };

let cssLoaded = false;
function css() {
  if (cssLoaded) return;
  cssLoaded = true;
  document.head.append(el('link', { rel: 'stylesheet', href: '/_dd/static/pdf.css' }));
}

const svg = (d) => {
  const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  s.setAttribute('viewBox', '0 0 20 20');
  s.setAttribute('aria-hidden', 'true');
  s.innerHTML = d;
  return s;
};
// bytes to base64 a slice at a time: spreading a big array overflows the stack
const b64 = (u8) => {
  let s = '';
  for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000));
  return btoa(s);
};
const pngBytes = (canvas) => new Promise((ok) => canvas.toBlob(async (b) => ok(new Uint8Array(await b.arrayBuffer())), 'image/png'));

/// Open the editor over the page. `item` is a PDF in the library (path,
/// name); or, for one that is not there yet (a mail's attachment), `bytes`
/// and `name`, and Save asks where in Files it goes, starting at `folder`.
/// `lib` is the library, `who` the member's name, mode 'edit' or 'sign'.
/// `saved(path)` once a save has landed. `actions`: more ways out, each
/// { label, small, run(bytes, name) }, listed under Save (Sign and reply).
export async function openEditor({ lib, item, bytes, name, folder, who, mode = 'edit', saved, actions = [] }) {
  css();
  const ed = new Editor(lib, item || { path: null, name: name || 'document.pdf' }, who, saved);
  ed.given = bytes || null;
  ed.folder = folder || 'Files';
  ed.actions = actions;
  await ed.start(mode);
  return ed;
}

class Editor {
  constructor(lib, item, who, saved) {
    this.lib = lib;
    this.item = item;
    this.who = who;
    this.saved = saved;
    this.tool = 'select';
    this.zoom = 1;
    this.pages = [];             // { box, canvas, layer, svg, drawn, w, h }
    this.selected = null;        // { page, n, rect, type }
    this.picked = new Set();     // pages ticked in the rail
    this.current = 0;
    this.color = COLORS[0][1];
    this.marker = MARKERS[0][1];
    this.width = 2;
    this.size = 12;
    this.markup = 'Highlight';
    this.shape = 'Square';
    this.fieldKind = 'text';
    this.redactMode = 'area';
    this.signature = null;       // the one being placed
    this.digital = !lib.reader && !lib.demo;
    this.pendingSeal = false;    // a key signature to seal at the next save
    this.signs = [];             // saved signatures
    this.identity = null;
    this.hits = [];              // find: [{ page, quads }]
    this.hitAt = -1;
    this.paraCache = new Map();
    this.checks = null;          // the file's own signatures, checked
    this.stamp = 0;              // journal position at the last save
    this.readOnly = !!lib.reader;
  }

  async start(mode) {
    this.root = el('section', { class: 'pdfed', role: 'dialog', 'aria-modal': 'true', 'aria-label': `Edit ${this.item.name}` });
    this.build();
    document.body.append(this.root);
    document.documentElement.style.overflow = 'hidden';
    this.keyHandler = (e) => this.key(e);
    addEventListener('keydown', this.keyHandler, true);
    this.leaveHandler = (e) => { if (this.dirty()) { e.preventDefault(); e.returnValue = ''; } };
    addEventListener('beforeunload', this.leaveHandler);
    this.back.focus();
    try {
      const bytes = this.given || await fetchPlain(this.lib, this.item.path);
      this.pdf = await this.openBytes(bytes);
      if (!this.pdf) return this.close(true);
    } catch (e) {
      this.pagesEl.replaceChildren(el('p', { class: 'pe-note', 'data-state': 'broken', role: 'alert', text: `This PDF could not be opened: ${e.message}. Download it to open it elsewhere.` }));
      return;
    }
    this.layout();
    this.loadSigns();
    this.checkSignatures();
    this.setTool(mode === 'sign' ? 'sign' : 'select');
    this.state();
  }

  async openBytes(bytes, password) {
    try {
      return await Pdf.open(bytes, password);
    } catch (e) {
      if (!e.password) throw e;
      const pw = await this.ask({ title: 'This PDF has a password', text: e.wrong ? 'That password did not open it. Try again.' : 'Type its password to open it. It is used in this tab only.', value: '', ok: 'Open', type: 'password' });
      if (pw === null) return null;
      return this.openBytes(bytes, pw);
    }
  }

  build() {
    const where = this.item.path ? this.item.path.split('/').slice(1, -1) : null;
    this.back = el('button', { class: 'pe-btn quiet', type: 'button', text: this.item.path ? '← Files' : '← Back', 'aria-label': this.item.path ? 'Back to Files' : 'Close the editor', onclick: () => this.close() });
    this.stateEl = el('span', { class: 'state-word', 'data-state': 'ok', role: 'status', text: 'Opening…' });
    this.undoBtn = el('button', { class: 'pe-btn quiet', type: 'button', text: '↶ Undo', disabled: true, onclick: () => this.undo() });
    this.redoBtn = el('button', { class: 'pe-btn quiet wide', type: 'button', text: '↷ Redo', disabled: true, onclick: () => this.redo() });
    this.saveBtn = el('button', { class: 'pe-btn go', type: 'button', text: this.readOnly ? 'Download' : this.item.path ? 'Save' : 'Save to Files', disabled: !this.readOnly, onclick: () => (this.readOnly ? this.saveAs('download') : this.saveAs('replace')) });
    this.menu = el('div', { class: 'pe-menu', role: 'menu', hidden: true });
    const opt = (label, small, fn, hide) => (hide ? null : el('button', { type: 'button', role: 'menuitem', onclick: () => { this.menu.hidden = true; fn(); } }, label, el('small', { text: small })));
    const loose = !this.item.path;
    this.menu.append(
      opt(loose ? 'Save to Files' : 'Save', loose ? 'Into a folder you pick; it is locked like your other files' : 'Replaces it; the old one stays in the Trash for 30 days', () => this.saveAs('replace'), this.readOnly),
      opt('Save as a copy', 'A new file beside it; this one stays as it was', () => this.saveAs('copy'), this.readOnly),
      opt('Save flattened', 'Marks and fields burnt into the pages, for good', () => this.saveAs('flatten'), this.readOnly),
      opt('Save smaller', 'Unused parts dropped and the rest packed tighter', () => this.saveAs('smaller'), this.readOnly),
      ...this.actions.map((a) => opt(a.label, a.small || '', () => this.saveAs('action', a))),
      el('hr'),
      opt('Download', 'To this device, unlocked', () => this.saveAs('download')),
    );
    this.more = el('button', { class: 'pe-btn go more', type: 'button', text: '▾', 'aria-label': 'More ways to save', 'aria-haspopup': 'menu', 'aria-expanded': 'false', onclick: (e) => { e.stopPropagation(); this.toggleMenu(); } });
    this.whereEl = el('div', { class: 'where', text: where ? ['My files', ...where].join(' / ') : 'Not in Files yet' });
    const head = el('div', { class: 'pe-head' },
      this.back,
      el('div', { class: 'pe-title' },
        el('div', {}, el('h1', { text: this.item.name }), this.whereEl)),
      this.stateEl,
      el('div', { class: 'pe-acts' },
        this.undoBtn, this.redoBtn,
        el('div', { class: 'pe-save' }, this.saveBtn, this.more, this.menu)));

    this.toolBtns = new Map();
    const tools = el('div', { class: 'pe-tools', role: 'toolbar', 'aria-label': 'Tools' });
    for (const t of TOOLS) {
      if (!t) { tools.append(el('span', { class: 'sep', 'aria-hidden': 'true' })); continue; }
      const b = el('button', { class: 'pe-tool', type: 'button', 'aria-pressed': 'false', onclick: () => this.setTool(t[0]) }, svg(ICONS[t[0]]), t[1]);
      const k = Object.entries(KEYS).find(([, v]) => v === t[0]);
      if (k) b.title = `${t[1]} (${k[0].toUpperCase()})`;
      this.toolBtns.set(t[0], b);
      tools.append(b);
    }
    tools.addEventListener('keydown', (e) => {
      if (e.key !== 'ArrowRight' && e.key !== 'ArrowLeft') return;
      const all = [...this.toolBtns.values()];
      const i = all.indexOf(document.activeElement);
      if (i < 0) return;
      e.preventDefault();
      all[(i + (e.key === 'ArrowRight' ? 1 : -1) + all.length) % all.length].focus();
    });

    this.thumbsEl = el('div', { class: 'pe-thumbs', role: 'listbox', 'aria-label': 'Pages', 'aria-multiselectable': 'true' });
    this.rail = el('nav', { class: 'pe-rail', 'aria-label': 'Pages' }, el('h2', { text: 'Pages' }), this.thumbsEl);
    this.pagesEl = el('main', { class: 'pe-pages', tabindex: '-1', 'aria-label': 'The pages' },
      el('div', { class: 'pe-loading', 'aria-hidden': 'true' }, el('div'), el('div')));
    this.insp = el('aside', { class: 'pe-insp', 'aria-label': 'Tool settings' });
    const bench = el('div', { class: 'pe-bench' }, this.rail, this.pagesEl, this.insp);

    this.pageOf = el('span', { class: 'num', text: '—' });
    this.zoomEl = el('span', { class: 'num', text: '100%' });
    this.lock = el('span', { class: 'pe-lock', role: 'status' });
    this.lockText();
    const status = el('div', { class: 'pe-status' },
      this.pageOf,
      el('span', { class: 'zoom' },
        el('button', { class: 'pe-btn quiet small', type: 'button', text: '−', 'aria-label': 'Zoom out', onclick: () => this.setZoom(this.zoom / 1.15) }),
        this.zoomEl,
        el('button', { class: 'pe-btn quiet small', type: 'button', text: '+', 'aria-label': 'Zoom in', onclick: () => this.setZoom(this.zoom * 1.15) }),
        el('button', { class: 'pe-btn quiet small', type: 'button', text: 'Fit', 'aria-label': 'Fit the page to the width', onclick: () => this.fit() })),
      el('button', { class: 'pe-btn small pe-insp-toggle', type: 'button', text: 'Tool settings', onclick: () => this.insp.classList.toggle('open') }),
      el('span', { class: 'grow' }),
      this.lock);
    this.root.append(head, el('div', { class: 'pe-tools-wrap' }, tools), bench, status);
    this.root.addEventListener('click', (e) => { if (!e.target.closest('.pe-save')) this.menu.hidden = true; });
  }

  lockText(msg) {
    clearTimeout(this.lockT);
    this.lock.replaceChildren();
    if (msg) {
      this.lock.textContent = msg;
      this.lockT = setTimeout(() => this.lockText(), 5000);
      return;
    }
    const s = svg('<rect x="3" y="7" width="10" height="7" rx="1"/><path d="M5 7V5a3 3 0 016 0v2"/>');
    s.setAttribute('viewBox', '0 0 16 16');
    this.lock.append(s, 'Edited in this tab. The box only ever holds the locked file.');
  }
  say(msg) { this.lockText(msg); }

  toggleMenu() {
    this.menu.hidden = !this.menu.hidden;
    this.more.setAttribute('aria-expanded', String(!this.menu.hidden));
    if (!this.menu.hidden) this.menu.querySelector('button').focus();
  }

  layout() {
    const n = this.pdf.count;
    const avail = Math.max(200, this.pagesEl.clientWidth - 40);
    if (!this.zoomSet) {
      const w = this.pdf.size(0)[0];
      this.zoom = Math.min(1.6, Math.max(0.4, avail / w));
      this.zoomSet = true;
    }
    this.io?.disconnect();
    this.io = new IntersectionObserver((es) => {
      for (const e of es) {
        const p = this.pages.find((x) => x.box === e.target);
        if (!p) continue;
        p.visible = e.isIntersecting;
        if (e.isIntersecting && !p.drawn) this.draw(p.i);
      }
      this.whereNow();
    }, { root: this.pagesEl, rootMargin: '600px 0px' });
    this.pages = [];
    this.pagesEl.replaceChildren();
    for (let i = 0; i < n; i++) {
      const [w, h] = this.pdf.size(i);
      const canvas = el('canvas', { 'aria-hidden': 'true' });
      const s = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
      const layer = el('div', { class: 'pe-layer' }, s);
      const box = el('div', { class: 'pe-page', role: 'img', 'aria-label': `Page ${i + 1} of ${n}`, 'data-tool': this.tool }, el('div', { class: 'ph' }), canvas, layer);
      box.style.width = `${w * this.zoom}px`;
      box.style.height = `${h * this.zoom}px`;
      const p = { i, box, canvas, layer, svg: s, w, h, drawn: false };
      this.pages.push(p);
      this.pagesEl.append(box);
      this.io.observe(box);
      layer.addEventListener('pointerdown', (e) => this.down(p, e));
      layer.addEventListener('pointermove', (e) => this.hover(p, e));
      layer.addEventListener('dblclick', (e) => this.dbl(p, e));
    }
    this.pagesEl.onscroll = () => this.whereNow();
    this.thumbs();
    this.zoomEl.textContent = `${Math.round(this.zoom * 100)}%`;
  }

  draw(i) {
    const p = this.pages[i];
    if (!p) return;
    const scale = this.zoom * (devicePixelRatio || 1);
    const r = this.pdf.render(i, scale);
    p.canvas.width = r.width;
    p.canvas.height = r.height;
    p.canvas.getContext('2d').putImageData(new ImageData(r.pixels, r.width, r.height), 0, 0);
    p.drawn = true;
    p.box.querySelector('.ph')?.remove();
    this.overlay(p);
  }

  // a page changed: drawn again, its thumbnail too, its text read again
  redraw(i) {
    this.paraCache.delete(i);
    const p = this.pages[i];
    if (!p) return;
    const [w, h] = this.pdf.size(i);
    if (Math.abs(w - p.w) > 0.5 || Math.abs(h - p.h) > 0.5) { this.layout(); return; }
    this.draw(i);
    this.thumb(i);
    this.state();
  }

  // the whole document may have changed (undo, pages): everything again
  refresh() {
    this.paraCache.clear();
    this.selected = null;
    const sizes = this.pages.map((p) => `${p.w}x${p.h}`).join();
    const now = [...Array(this.pdf.count).keys()].map((i) => this.pdf.size(i).join('x')).join();
    if (this.pdf.count !== this.pages.length || sizes !== now) this.layout();
    else {
      for (const p of this.pages) { p.drawn = false; if (p.visible) this.draw(p.i); }
      this.thumbs();
    }
    this.state();
    this.inspector();
  }

  setZoom(z) {
    const keep = this.current;
    this.zoom = Math.min(4, Math.max(0.25, z));
    this.zoomSet = true;
    for (const p of this.pages) {
      p.box.style.width = `${p.w * this.zoom}px`;
      p.box.style.height = `${p.h * this.zoom}px`;
      p.drawn = false;
    }
    this.zoomEl.textContent = `${Math.round(this.zoom * 100)}%`;
    this.pages[keep]?.box.scrollIntoView({ block: 'start' });
    for (const p of this.pages) if (p.visible) this.draw(p.i);
  }

  fit() {
    const w = Math.max(...this.pages.map((p) => p.w));
    this.setZoom((this.pagesEl.clientWidth - 40) / w);
  }

  whereNow() {
    const top = this.pagesEl.getBoundingClientRect().top + this.pagesEl.clientHeight / 3;
    let n = 0;
    this.pages.forEach((p, i) => { if (p.box.getBoundingClientRect().top < top) n = i; });
    if (n !== this.current || this.pageOf.textContent === '—') {
      this.current = n;
      this.pageOf.textContent = `Page ${n + 1} of ${this.pages.length}`;
      for (const t of this.thumbsEl.children) t.setAttribute('aria-current', String(Number(t.dataset.i) === n));
    }
  }

  goTo(i) {
    this.pages[i]?.box.scrollIntoView({ behavior: matchMedia('(prefers-reduced-motion: reduce)').matches ? 'auto' : 'smooth', block: 'start' });
  }

  thumbs() {
    this.thumbsEl.replaceChildren();
    const io = new IntersectionObserver((es) => {
      for (const e of es) if (e.isIntersecting) { this.thumb(Number(e.target.dataset.i)); io.unobserve(e.target); }
    }, { root: this.rail });
    for (let i = 0; i < this.pdf.count; i++) {
      const t = el('div', { class: 'pe-thumb', role: 'option', tabindex: i === this.current ? '0' : '-1', draggable: 'true', 'data-i': String(i), 'aria-label': `Page ${i + 1}`, 'aria-selected': String(this.picked.has(i)), 'aria-current': String(i === this.current) },
        el('div', { class: 'ph' }), el('span', { class: 'n', text: String(i + 1) }));
      t.onclick = (e) => this.pickThumb(i, e);
      t.onkeydown = (e) => this.thumbKey(i, e);
      t.ondragstart = (e) => { this.dragFrom = i; e.dataTransfer.effectAllowed = 'move'; e.dataTransfer.setData('text/plain', String(i)); };
      t.ondragover = (e) => {
        e.preventDefault();
        const r = t.getBoundingClientRect();
        const after = e.clientY > r.top + r.height / 2;
        for (const o of this.thumbsEl.children) o.classList.remove('drop-before', 'drop-after');
        t.classList.add(after ? 'drop-after' : 'drop-before');
      };
      t.ondragleave = () => t.classList.remove('drop-before', 'drop-after');
      t.ondrop = (e) => {
        e.preventDefault();
        const after = t.classList.contains('drop-after');
        t.classList.remove('drop-before', 'drop-after');
        e.dataTransfer.files?.length ? this.insertFiles([...e.dataTransfer.files], after ? i + 1 : i) : this.movePages(this.picked.size && this.picked.has(this.dragFrom) ? [...this.picked] : [this.dragFrom], after ? i + 1 : i);
      };
      this.thumbsEl.append(t);
      io.observe(t);
    }
  }

  thumb(i) {
    const t = this.thumbsEl.children[i];
    if (!t) return;
    const r = this.pdf.thumb(i, 100 * (devicePixelRatio || 1));
    const c = el('canvas', { 'aria-hidden': 'true' });
    c.width = r.width;
    c.height = r.height;
    c.getContext('2d').putImageData(new ImageData(r.pixels, r.width, r.height), 0, 0);
    t.firstChild.replaceWith(c);
  }

  pickThumb(i, e) {
    if (e.shiftKey && this.picked.size) {
      const from = Math.min(...this.picked), to = i;
      for (let k = Math.min(from, to); k <= Math.max(from, to); k++) this.picked.add(k);
    } else if (e.ctrlKey || e.metaKey) {
      this.picked.has(i) ? this.picked.delete(i) : this.picked.add(i);
    } else {
      this.picked = new Set([i]);
      this.goTo(i);
    }
    for (const t of this.thumbsEl.children) t.setAttribute('aria-selected', String(this.picked.has(Number(t.dataset.i))));
    if (this.tool === 'pages') this.inspector();
  }

  thumbKey(i, e) {
    const all = [...this.thumbsEl.children];
    const go = (k) => { if (!all[k]) return; all[i].tabIndex = -1; all[k].tabIndex = 0; all[k].focus(); this.pickThumb(k, e); };
    if (e.key === 'ArrowDown') { e.preventDefault(); go(i + 1); }
    if (e.key === 'ArrowUp') { e.preventDefault(); go(i - 1); }
    if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); this.pickThumb(i, e); }
    if (e.key === 'Delete' && !this.readOnly) { e.preventDefault(); this.deletePages(this.picked.size ? [...this.picked] : [i]); }
    if (e.altKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) { e.preventDefault(); this.movePages([i], e.key === 'ArrowUp' ? i - 1 : i + 2); }
  }

  overlay(p) {
    for (const n of [...p.layer.children]) if (n !== p.svg) n.remove();
    p.svg.replaceChildren();
    p.box.dataset.tool = this.tool;
    const k = this.zoom;
    const box = (r, cls) => {
      const d = el('div', { class: cls });
      Object.assign(d.style, { left: `${r[0] * k}px`, top: `${r[1] * k}px`, width: `${(r[2] - r[0]) * k}px`, height: `${(r[3] - r[1]) * k}px` });
      p.layer.append(d);
      return d;
    };
    if (this.tool === 'form' || this.tool === 'addfield' || this.tool === 'sign') {
      for (const f of this.pdf.fields(p.i)) {
        if (f.type === 'signature') {
          if (!f.signed && this.tool !== 'addfield') {
            const b = el('button', { class: 'pe-signhere', type: 'button', text: 'Sign here', onclick: (e) => { e.stopPropagation(); this.signInto(p, f); } });
            Object.assign(b.style, { left: `${f.rect[0] * k}px`, top: `${f.rect[3] * k}px` });
            p.layer.append(b);
            box(f.rect, 'pe-field empty');
          }
        } else if (this.tool !== 'sign') box(f.rect, `pe-field${!f.value || f.value === 'Off' ? ' empty' : ''}`);
      }
    }
    if (this.tool === 'edittext') for (const [n, para] of this.paras(p.i).entries()) { const d = box(para.bbox, 'pe-para'); d.dataset.n = String(n); }
    for (const [h, hit] of this.hits.entries()) {
      if (hit.page !== p.i) continue;
      for (const q of hit.quads) {
        const xs = [q[0], q[2], q[4], q[6]], ys = [q[1], q[3], q[5], q[7]];
        box([Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)], `pe-hit${h === this.hitAt ? ' now' : ''}`);
      }
    }
    if (this.textSel?.page === p.i) for (const q of this.textSel.quads) box(quadRect(q), 'pe-textsel');
    if (this.selected?.page === p.i) this.selBox(p);
  }

  paras(i) {
    if (!this.paraCache.has(i)) this.paraCache.set(i, this.pdf.paragraphs(i));
    return this.paraCache.get(i);
  }

  selBox(p) {
    const s = this.selected;
    const k = this.zoom;
    const r = s.rect;
    const d = el('div', { class: 'pe-sel', 'aria-label': `Selected ${s.type}` });
    Object.assign(d.style, { left: `${r[0] * k}px`, top: `${r[1] * k}px`, width: `${(r[2] - r[0]) * k}px`, height: `${(r[3] - r[1]) * k}px` });
    if (!/Highlight|Underline|StrikeOut|Squiggly|Text|Line/.test(s.type)) {
      for (const c of ['nw', 'ne', 'sw', 'se']) d.append(el('span', { class: `h ${c}`, 'data-h': c }));
    }
    p.layer.append(d);
  }

  at(p, e) {
    const r = p.layer.getBoundingClientRect();
    return [(e.clientX - r.left) / this.zoom, (e.clientY - r.top) / this.zoom];
  }

  hover(p, e) {
    if (this.tool !== 'edittext' || this.drag) return;
    const [x, y] = this.at(p, e);
    const n = this.paras(p.i).findIndex((q) => x >= q.bbox[0] - 2 && x <= q.bbox[2] + 2 && y >= q.bbox[1] - 2 && y <= q.bbox[3] + 2);
    for (const d of p.layer.querySelectorAll('.pe-para')) d.classList.toggle('hot', Number(d.dataset.n) === n);
  }

  down(p, e) {
    if (e.button !== 0 || !this.pdf) return;
    if (e.target.closest('.pe-edit, .pe-signhere')) return;
    const pt = this.at(p, e);
    this.current = p.i;
    const t = this.tool;
    if (this.readOnly && !['select', 'find', 'highlight'].includes(t)) { this.say('This library is shared with you to look at: changes can be downloaded, not saved here.'); }
    const handle = e.target.dataset?.h;
    // a click that opens a field to type in must not take the focus away from it
    if (['text', 'edittext', 'note', 'form'].includes(t)) e.preventDefault();
    if (t === 'select') return this.selectDown(p, pt, e, handle);
    if (t === 'text') return this.textAt(p, pt);
    if (t === 'edittext') return this.editAt(p, pt);
    if (t === 'note') return this.noteAt(p, pt);
    if (t === 'form') return this.fieldAt(p, pt);
    if (t === 'sign') return this.signAt(p, pt);
    if (t === 'image' && this.image) return this.dragRect(p, pt, e, (r) => this.placeImage(p, r));
    if (t === 'draw') return this.drawInk(p, pt, e);
    if (t === 'shape') return this.dragRect(p, pt, e, (r, a, b) => this.addShape(p, r, a, b), this.shape === 'Line' || this.shape === 'Arrow');
    if (t === 'addfield') return this.dragRect(p, pt, e, (r) => this.addField(p, r));
    if (t === 'highlight') return this.textDrag(p, pt, e, (sel) => this.addMarkup(p, sel));
    if (t === 'redact') {
      if (this.redactMode === 'text') return this.textDrag(p, pt, e, (sel) => this.addRedact(p, { quads: sel.quads }));
      return this.dragRect(p, pt, e, (r) => this.addRedact(p, { rect: r }));
    }
    if (t === 'find' || t === 'pages') return this.textDrag(p, pt, e, (sel) => this.showTextSel(p, sel));
  }

  dbl(p, e) {
    if (this.tool !== 'select') return;
    const pt = this.at(p, e);
    const a = this.annotAt(p, pt);
    if (a && (a.type === 'FreeText' || a.type === 'Text')) this.editAnnotText(p, a);
  }

  annotAt(p, [x, y]) {
    const list = this.pdf.annots(p.i);
    for (let k = list.length - 1; k >= 0; k--) {
      const r = list[k].rect;
      if (x >= r[0] - 3 && x <= r[2] + 3 && y >= r[1] - 3 && y <= r[3] + 3) return list[k];
    }
    return null;
  }

  selectDown(p, pt, e, handle) {
    const s = this.selected;
    if (handle && s) return this.resizeDrag(p, pt, e, handle);
    const a = this.annotAt(p, pt);
    if (!a) {
      this.selected = null;
      this.overlay(p);
      this.inspector();
      return this.textDrag(p, pt, e, (sel) => this.showTextSel(p, sel));
    }
    this.selected = { page: p.i, n: a.n, rect: a.rect, type: a.type };
    this.textSel = null;
    this.overlay(p);
    this.inspector();
    if (this.readOnly) return;
    const start = pt;
    const box = p.layer.querySelector('.pe-sel');
    const k = this.zoom;
    let moved = false;
    this.capture(e, (ev) => {
      const [x, y] = this.at(p, ev);
      moved = moved || Math.abs(x - start[0]) + Math.abs(y - start[1]) > 2;
      box.style.transform = `translate(${(x - start[0]) * k}px, ${(y - start[1]) * k}px)`;
    }, (ev) => {
      if (!moved) return;
      const [x, y] = this.at(p, ev);
      this.pdf.moveAnnot(p.i, a.n, x - start[0], y - start[1]);
      const now = this.pdf.annots(p.i).find((o) => o.n === a.n);
      this.selected.rect = now?.rect || this.selected.rect;
      this.redraw(p.i);
    });
  }

  resizeDrag(p, _pt, e, h) {
    const s = this.selected;
    const r0 = [...s.rect];
    const box = p.layer.querySelector('.pe-sel');
    const k = this.zoom;
    const calc = (x, y) => {
      const r = [...r0];
      if (h.includes('w')) r[0] = Math.min(x, r0[2] - 4);
      if (h.includes('e')) r[2] = Math.max(x, r0[0] + 4);
      if (h.includes('n')) r[1] = Math.min(y, r0[3] - 4);
      if (h.includes('s')) r[3] = Math.max(y, r0[1] + 4);
      return r;
    };
    this.capture(e, (ev) => {
      const r = calc(...this.at(p, ev));
      Object.assign(box.style, { left: `${r[0] * k}px`, top: `${r[1] * k}px`, width: `${(r[2] - r[0]) * k}px`, height: `${(r[3] - r[1]) * k}px` });
    }, (ev) => {
      const r = calc(...this.at(p, ev));
      this.pdf.resizeAnnot(p.i, s.n, r);
      s.rect = r;
      this.redraw(p.i);
    });
  }

  // follow the pointer until it lets go
  capture(e, move, up) {
    const t = e.currentTarget || e.target;
    t.setPointerCapture?.(e.pointerId);
    this.drag = true;
    const m = (ev) => move(ev);
    const u = (ev) => {
      t.removeEventListener('pointermove', m);
      t.removeEventListener('pointerup', u);
      t.removeEventListener('pointercancel', u);
      this.drag = false;
      up(ev);
    };
    t.addEventListener('pointermove', m);
    t.addEventListener('pointerup', u);
    t.addEventListener('pointercancel', u);
    e.preventDefault();
  }

  dragRect(p, a, e, done, line = false) {
    const k = this.zoom;
    const shape = document.createElementNS('http://www.w3.org/2000/svg', line ? 'line' : 'rect');
    shape.setAttribute('fill', this.tool === 'redact' ? 'rgba(0,0,0,.75)' : 'rgba(29,68,112,.08)');
    shape.setAttribute('stroke', hex(this.tool === 'redact' ? [0, 0, 0] : this.color));
    shape.setAttribute('stroke-width', String(Math.max(1, this.width * k)));
    shape.setAttribute('stroke-dasharray', this.tool === 'addfield' || this.tool === 'image' ? '4 3' : '');
    p.svg.append(shape);
    let b = a;
    const rectOf = () => [Math.min(a[0], b[0]), Math.min(a[1], b[1]), Math.max(a[0], b[0]), Math.max(a[1], b[1])];
    this.capture(e, (ev) => {
      b = this.at(p, ev);
      if (line) {
        shape.setAttribute('x1', a[0] * k); shape.setAttribute('y1', a[1] * k);
        shape.setAttribute('x2', b[0] * k); shape.setAttribute('y2', b[1] * k);
      } else {
        const r = rectOf();
        shape.setAttribute('x', r[0] * k); shape.setAttribute('y', r[1] * k);
        shape.setAttribute('width', (r[2] - r[0]) * k); shape.setAttribute('height', (r[3] - r[1]) * k);
      }
    }, async () => {
      shape.remove();
      const r = rectOf();
      const tiny = Math.hypot(b[0] - a[0], b[1] - a[1]) < 4;
      if (this.readOnly && this.tool !== 'redact') return;
      try {
        await done(tiny ? null : r, a, b);
      } catch (err) {
        this.say(err.message);
      }
    });
  }

  textDrag(p, a, e, done) {
    let b = a;
    this.capture(e, (ev) => {
      b = this.at(p, ev);
      const sel = this.pdf.select(p.i, a, b);
      this.textSel = { page: p.i, ...sel };
      for (const d of p.layer.querySelectorAll('.pe-textsel')) d.remove();
      for (const q of sel.quads) {
        const r = quadRect(q);
        const d = el('div', { class: 'pe-textsel' });
        Object.assign(d.style, { left: `${r[0] * this.zoom}px`, top: `${r[1] * this.zoom}px`, width: `${(r[2] - r[0]) * this.zoom}px`, height: `${(r[3] - r[1]) * this.zoom}px` });
        p.layer.append(d);
      }
    }, () => {
      const sel = this.textSel;
      if (!sel?.quads.length || Math.hypot(b[0] - a[0], b[1] - a[1]) < 2) {
        this.textSel = null;
        this.overlay(p);
        return;
      }
      done(sel);
    });
  }

  showTextSel(p, sel) {
    this.textSel = { page: p.i, ...sel };
    this.overlay(p);
    this.inspector();
  }

  async addMarkup(p, sel) {
    if (this.readOnly) return;
    const color = this.markup === 'Highlight' ? this.marker : this.color;
    await this.pdf.add(p.i, { type: this.markup, quads: sel.quads, color, author: this.who });
    this.textSel = null;
    this.redraw(p.i);
  }

  drawInk(p, a, e) {
    if (this.readOnly) return;
    const k = this.zoom;
    const path = document.createElementNS('http://www.w3.org/2000/svg', 'path');
    const marker = this.inkKind === 'marker';
    path.setAttribute('fill', 'none');
    path.setAttribute('stroke', hex(marker ? this.marker : this.color));
    path.setAttribute('stroke-width', String((marker ? this.width * 5 : this.width) * k));
    path.setAttribute('stroke-linecap', 'round');
    path.setAttribute('stroke-linejoin', 'round');
    if (marker) path.setAttribute('stroke-opacity', '0.45');
    p.svg.append(path);
    const pts = [a];
    let d = `M${a[0] * k} ${a[1] * k}`;
    this.capture(e, (ev) => {
      const q = this.at(p, ev);
      const last = pts.at(-1);
      if (Math.hypot(q[0] - last[0], q[1] - last[1]) < 0.8) return;
      pts.push(q);
      d += ` L${q[0] * k} ${q[1] * k}`;
      path.setAttribute('d', d);
    }, async () => {
      if (pts.length < 2) pts.push([a[0] + 0.5, a[1] + 0.5]);
      await this.pdf.add(p.i, { type: 'Ink', paths: [smooth(pts)], color: marker ? this.marker : this.color, width: marker ? this.width * 5 : this.width, opacity: marker ? 0.45 : 1, author: this.who });
      path.remove();
      this.redraw(p.i);
    });
  }

  async addShape(p, r, a, b) {
    if (!r && this.shape !== 'Line' && this.shape !== 'Arrow') return;
    if (this.shape === 'Line' || this.shape === 'Arrow') {
      if (Math.hypot(b[0] - a[0], b[1] - a[1]) < 4) return;
      await this.pdf.add(p.i, { type: 'Line', a, b, color: this.color, width: this.width, arrow: this.shape === 'Arrow', author: this.who });
    } else {
      await this.pdf.add(p.i, { type: this.shape, rect: r, color: this.color, width: this.width, fill: this.fill ? this.marker : null, author: this.who });
    }
    this.redraw(p.i);
  }

  async addRedact(p, where) {
    if (!where.rect && !where.quads) return;
    await this.pdf.add(p.i, { type: 'Redact', ...where });
    this.textSel = null;
    this.redraw(p.i);
    this.inspector();
  }

  // a text box: typed where it was clicked, then a mark (or part of the page)
  textAt(p, [x, y]) {
    if (this.readOnly) return;
    const k = this.zoom;
    const ta = el('textarea', { class: 'pe-edit', rows: '1', 'aria-label': 'Text to add', placeholder: 'Type here' });
    Object.assign(ta.style, { left: `${x * k}px`, top: `${(y - this.size * 0.7) * k}px`, fontSize: `${this.size * k}px`, color: hex(this.color), minWidth: `${80 * k}px` });
    const fit = () => { ta.style.height = 'auto'; ta.style.height = `${ta.scrollHeight}px`; ta.style.width = 'auto'; ta.style.width = `${Math.max(80 * k, ta.scrollWidth + 4)}px`; };
    ta.oninput = fit;
    p.layer.append(ta);
    ta.focus();
    let done = false;
    const commit = async () => {
      if (done) return;
      done = true;
      const text = ta.value.replace(/\s+$/, '');
      const w = ta.offsetWidth / k, h = ta.offsetHeight / k;
      ta.remove();
      if (!text) return;
      if (this.pageText) await this.pdf.addPageText(p.i, [x, y - this.size * 0.7], text, { size: this.size, color: this.color, family: this.family || 'Sans' });
      else await this.pdf.add(p.i, { type: 'FreeText', rect: [x, y - this.size * 0.7, x + w + 4, y - this.size * 0.7 + h + 2], text, size: this.size, color: this.color, font: this.family === 'Serif' ? 'TiRo' : this.family === 'Mono' ? 'Cour' : 'Helv', author: this.who });
      this.redraw(p.i);
    };
    ta.onblur = commit;
    ta.onkeydown = (e) => {
      if (e.key === 'Escape') { done = true; ta.remove(); }
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); ta.blur(); }
    };
  }

  editAnnotText(p, a) {
    const k = this.zoom;
    const r = a.rect;
    const ta = el('textarea', { class: 'pe-edit', 'aria-label': 'Text' });
    ta.value = a.text || '';
    Object.assign(ta.style, { left: `${r[0] * k}px`, top: `${r[1] * k}px`, width: `${Math.max(120, (r[2] - r[0]) * k)}px`, height: `${Math.max(40, (r[3] - r[1]) * k)}px`, fontSize: `${12 * k}px` });
    p.layer.append(ta);
    ta.focus();
    let done = false;
    ta.onblur = () => {
      if (done) return;
      done = true;
      ta.remove();
      if (ta.value !== (a.text || '')) { this.pdf.styleAnnot(p.i, a.n, { text: ta.value }); this.redraw(p.i); }
    };
    ta.onkeydown = (e) => { if (e.key === 'Escape') { done = true; ta.remove(); } };
  }

  noteAt(p, [x, y]) {
    if (this.readOnly) return;
    const k = this.zoom;
    const ta = el('textarea', { class: 'pe-edit', 'aria-label': 'Comment', placeholder: 'Comment' });
    Object.assign(ta.style, { left: `${x * k}px`, top: `${y * k}px`, width: '220px', height: '80px', fontSize: '14px' });
    p.layer.append(ta);
    ta.focus();
    let done = false;
    ta.onblur = async () => {
      if (done) return;
      done = true;
      ta.remove();
      if (!ta.value.trim()) return;
      await this.pdf.add(p.i, { type: 'Text', at: [x - 10, y - 10], text: ta.value.trim(), author: this.who });
      this.redraw(p.i);
    };
    ta.onkeydown = (e) => { if (e.key === 'Escape') { done = true; ta.remove(); } if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) ta.blur(); };
  }

  editAt(p, [x, y]) {
    if (this.readOnly) return;
    const paras = this.paras(p.i);
    const n = paras.findIndex((q) => x >= q.bbox[0] - 2 && x <= q.bbox[2] + 2 && y >= q.bbox[1] - 2 && y <= q.bbox[3] + 2);
    if (n < 0) {
      if (!this.pdf.hasText(p.i)) this.say('This page is a picture of text. Recognise text first (in this tool’s settings), or add text over it.');
      return;
    }
    const para = paras[n];
    const k = this.zoom;
    const ta = el('textarea', { class: `pe-edit ${/Mono/.test(this.guessFamily(para)) ? 'mono' : /Serif/.test(this.guessFamily(para)) ? 'serif' : ''}`, 'aria-label': 'Edit this paragraph' });
    ta.value = para.text;
    const b = para.bbox;
    Object.assign(ta.style, {
      left: `${(b[0] - 2) * k}px`, top: `${(b[1] - 2) * k}px`,
      width: `${(b[2] - b[0] + 6) * k}px`, minHeight: `${(b[3] - b[1] + 4) * k}px`,
      fontSize: `${para.size * k}px`,
    });
    const fit = () => { ta.style.height = 'auto'; ta.style.height = `${ta.scrollHeight + 2}px`; };
    ta.oninput = fit;
    p.layer.append(ta);
    fit();
    ta.focus();
    if (para.unreadable) this.note('attention', 'Some letters in this paragraph are not readable from the file and show as boxes. Type them in before you finish, or they come out as “?”.');
    let done = false;
    const commit = async (fitIt = false) => {
      if (done) return;
      done = true;
      const text = ta.value;
      ta.remove();
      if (text.replace(/\s+/g, ' ').trim() === para.text) return;
      try {
        const r = await this.pdf.editParagraph(p.i, n, text, { fit: fitIt });
        this.redraw(p.i);
        this.signedWarning();
        this.lastEdit = { page: p.i, n, text, r };
        if (r.overflow) this.note('attention', 'The new words run into what is below them.', [['Shrink to fit', () => this.refit()], ['Undo', () => this.undo()]]);
        else this.say(`Set in ${r.font}, the nearest face we have to the PDF’s own.`);
      } catch (e) {
        this.note('broken', `That paragraph could not be changed: ${e.message}`);
      }
    };
    ta.onblur = () => commit();
    ta.onkeydown = (e) => {
      if (e.key === 'Escape') { done = true; ta.remove(); }
      if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) { e.preventDefault(); ta.blur(); }
    };
  }

  guessFamily(para) {
    const f = para.first?.font;
    const n = f?.getName?.() || '';
    return /mono|courier|cmtt/i.test(n) ? 'Mono' : /sans|arial|helvet|calibri/i.test(n) ? 'Sans' : 'Serif';
  }

  async refit() {
    const e = this.lastEdit;
    if (!e) return;
    this.pdf.undo();
    this.paraCache.delete(e.page);
    const r = await this.pdf.editParagraph(e.page, e.n, e.text, { fit: true });
    this.redraw(e.page);
    this.note(r.overflow ? 'attention' : 'ok', r.overflow ? 'Even at four fifths of its size it does not fit. Shorten it, or undo.' : `Fitted at ${Math.round(r.scale * 100)}% of its size.`);
  }

  // Changing a signed file adds to it: the signed version stays inside, so
  // its signatures still check, and anyone with the file can see what was
  // signed. Said once, the first time words or pages are taken out.
  signedWarning() {
    if (!this.checks?.length || this.warnedSigned) return;
    this.warnedSigned = true;
    this.note('attention', 'This file is signed. Your change is saved after the signed version, which stays in the file so its signatures still check: anyone with the file can still see what was signed. To take something out for good, redact it; that writes the whole file again and its signatures stop checking.');
  }

  // a message in the settings, above the tool's own
  note(state, text, actions = []) {
    this.notice = { state, text, actions };
    this.inspector();
  }

  fieldAt(p, [x, y]) {
    const fields = this.pdf.fields(p.i);
    const f = fields.find((o) => x >= o.rect[0] && x <= o.rect[2] && y >= o.rect[1] && y <= o.rect[3]);
    if (!f) return;
    this.openField(p, f);
  }

  openField(p, f) {
    if (this.readOnly) return;
    if (f.readOnly) { this.say('That field is locked by the PDF.'); return; }
    if (f.type === 'signature') { this.signInto(p, f); return; }
    if (f.type === 'checkbox' || f.type === 'radiobutton') {
      this.pdf.toggleField(p.i, f.n);
      this.redraw(p.i);
      this.inspector();
      return;
    }
    const k = this.zoom;
    const r = f.rect;
    let input;
    if (f.type === 'combobox' || f.type === 'listbox') {
      input = el('select', { class: 'pe-edit', 'aria-label': f.label || f.name }, ...f.options.map((o) => el('option', { value: o, text: o, selected: o === f.value })));
    } else if (f.multiline) {
      input = el('textarea', { class: 'pe-edit', 'aria-label': f.label || f.name });
      input.value = f.value;
    } else {
      input = el('input', { class: 'pe-edit', type: 'text', 'aria-label': f.label || f.name, value: f.value, maxlength: f.maxLen || undefined });
    }
    const h = (r[3] - r[1]) * k;
    Object.assign(input.style, { left: `${r[0] * k}px`, top: `${r[1] * k}px`, width: `${(r[2] - r[0]) * k}px`, height: `${h}px`, fontSize: `${Math.min(14 * k, h * 0.7)}px` });
    p.layer.append(input);
    input.focus();
    input.select?.();
    let done = false;
    const commit = (next) => {
      if (done) return;
      done = true;
      input.remove();
      if (input.value !== f.value) { this.pdf.setField(p.i, f.n, input.value); this.redraw(p.i); }
      this.inspector();
      if (next) this.nextField(p.i, f.n, next);
    };
    input.onblur = () => commit();
    input.onchange = () => { if (input.tagName === 'SELECT') commit(); };
    input.onkeydown = (e) => {
      if (e.key === 'Escape') { done = true; input.remove(); }
      if (e.key === 'Tab') { e.preventDefault(); commit(e.shiftKey ? -1 : 1); }
      if (e.key === 'Enter' && input.tagName === 'INPUT') { e.preventDefault(); commit(1); }
    };
  }

  // the next field to fill, across pages, in the order the file has them
  nextField(i, n, dir) {
    const all = [];
    for (let k = 0; k < this.pdf.count; k++) for (const f of this.pdf.fields(k)) if (!f.readOnly && f.type !== 'button') all.push({ page: k, f });
    const at = all.findIndex((x) => x.page === i && x.f.n === n);
    const next = all[(at + dir + all.length) % all.length];
    if (!next) return;
    this.goTo(next.page);
    setTimeout(() => this.openField(this.pages[next.page], next.f), 50);
  }

  addField(p, r) {
    if (!r) r = [0, 0, 0, 0];
    if (r[2] - r[0] < 8) {
      const w = { checkbox: 14, signature: 160 }[this.fieldKind] || 160;
      const h = { checkbox: 14, signature: 40 }[this.fieldKind] || 20;
      r = [r[0], r[1], r[0] + w, r[1] + h];
    }
    const name = this.pdf.addField(p.i, this.fieldKind === 'paragraph' ? 'multiline' : this.fieldKind, r);
    this.redraw(p.i);
    this.say(`Added “${name}”.`);
  }

  async loadSigns() {
    if (this.lib.reader || this.lib.demo) return;
    try {
      const b = await fetchPlain(this.lib, SIGS_FILE);
      this.signs = JSON.parse(new TextDecoder().decode(b));
    } catch { this.signs = []; }
    if (this.tool === 'sign') this.inspector();
  }

  async keepSigns() {
    const body = new TextEncoder().encode(JSON.stringify(this.signs));
    await put(this.lib, SIGS_FILE, new Blob([body]));
  }

  // The signing key is this device's alone: made unexportable by WebCrypto
  // and kept in the device's own storage, never in the library or on a box.
  // Each device signs with its own key and certificate, in the same name.
  async ensureIdentity() {
    if (this.identity) return this.identity;
    let kept = await idKept(this.who);
    if (!kept) {
      const name = await this.ask({ title: 'Your name on signatures', text: 'PDF readers show this as who signed. Your signing key is made on this device and never leaves it.', value: this.who || '', ok: 'Use this name' });
      if (!name) return null;
      kept = await sig.makeIdentity(name);
      if (!(await idKeep(this.who, kept))) this.say('This browser keeps nothing between visits here, so this signing key lasts until the tab closes.');
    }
    this.identity = sig.loadIdentity(kept);
    return this.identity;
  }

  async checkSignatures() {
    const list = this.pdf.signatures().filter((s) => s.signed && s.range?.[1] !== sig.RANGE_MARK);
    if (!list.length) { this.checks = []; return; }
    this.checks = await Promise.all(list.map(async (s) => ({ ...s, check: await sig.check(this.pdf.bytes, s.range, s.contents) })));
    if (this.tool === 'select' || this.tool === 'sign') this.inspector();
    const bad = this.checks.filter((c) => !c.check.ok).length;
    this.state();
    if (bad) this.say(`${bad === 1 ? 'A signature' : `${bad} signatures`} in this file did not check out. See Sign for which.`);
  }

  signAt(p, pt) {
    if (this.readOnly) return;
    if (!this.signature) { this.newSignature(); return; }
    const s = this.signature;
    const w = 150, h = w / (s.ratio || 3);
    this.placeSignature(p, [pt[0] - w / 2, pt[1] - h / 2, pt[0] + w / 2, pt[1] + h / 2]);
  }

  signInto(p, f) {
    if (this.readOnly) return;
    if (!this.signature) { this.afterSign = () => this.signInto(p, f); this.newSignature(); return; }
    const r = f.rect;
    const s = this.signature;
    // inside the field, kept to its shape
    const fw = r[2] - r[0], fh = r[3] - r[1];
    let w = fw, h = w / (s.ratio || 3);
    if (h > fh) { h = fh; w = h * (s.ratio || 3); }
    const x = r[0] + (fw - w) / 2, y = r[1] + (fh - h) / 2;
    this.placeSignature(p, [x, y, x + w, y + h], f);
  }

  async placeSignature(p, rect, field) {
    const png = await this.signaturePng(this.signature);
    await this.pdf.add(p.i, { type: 'Stamp', rect, png, author: this.who });
    if (this.digital && !this.pendingSeal) {
      const id = await this.ensureIdentity();
      if (id) {
        this.sealTag = this.pdf.prepareSignature(p.i, rect, { name: id.who, reason: 'Signed in Commonty', into: field ? { page: p.i, n: field.n } : undefined });
        this.pendingSeal = true;
      }
    }
    this.redraw(p.i);
    this.inspector();
    this.state();
    this.say(this.pendingSeal ? 'Signed. Your key seals the file when you save it; change nothing after that you want covered.' : 'Signature placed.');
  }

  // a signature as a PNG with see-through paper
  async signaturePng(s) {
    if (s.png) return Uint8Array.from(atob(s.png), (c) => c.charCodeAt(0));
    const c = typedCanvas(s.text, s.ink);
    return pngBytes(c);
  }

  newSignature() {
    const d = el('dialog', { 'aria-labelledby': 'pe-sig-title' });
    let tab = 'draw', ink = '#1d2a44', picture = null, inked = false;
    const tabs = el('div', { class: 'pe-tabs', role: 'tablist' });
    const panes = {};
    const pad = el('canvas', { 'aria-label': 'Draw your signature here' });
    panes.draw = el('div', { class: 'pe-pad' }, pad, el('span', { class: 'x', text: '×' }), el('span', { class: 'base' }));
    const typedIn = el('input', { type: 'text', 'aria-label': 'Your name', value: this.identity?.who || this.who || '' });
    const typedShow = el('div', { class: 'pe-typed' });
    panes.type = el('div', { hidden: true, style: 'display:grid;gap:10px' }, typedIn, typedShow);
    const file = el('input', { type: 'file', accept: 'image/*', class: 'sr-only' });
    const dropShow = el('div', { class: 'pe-drop' }, el('span', { text: 'Choose a photo of your signature on paper' }), el('small', { text: 'The paper is made see-through; only the ink stays.' }), file);
    dropShow.onclick = () => file.click();
    panes.upload = el('div', { hidden: true }, dropShow);
    for (const [k, label] of [['draw', 'Draw'], ['type', 'Type'], ['upload', 'Picture']]) {
      const b = el('button', { type: 'button', role: 'tab', 'aria-selected': String(k === tab), text: label, onclick: () => {
        tab = k;
        for (const x of tabs.children) x.setAttribute('aria-selected', String(x === b));
        for (const [n, p] of Object.entries(panes)) p.hidden = n !== k;
        if (k === 'draw') sizePad();
        if (k === 'type') showTyped();
      } });
      tabs.append(b);
    }
    const ctx = pad.getContext('2d');
    const sizePad = () => {
      const r = pad.getBoundingClientRect(), s = devicePixelRatio || 1;
      pad.width = r.width * s; pad.height = r.height * s;
      ctx.setTransform(s, 0, 0, s, 0, 0);
      ctx.lineCap = 'round'; ctx.lineJoin = 'round';
      inked = false;
    };
    let last = null, lastT = 0, wd = 2.4;
    pad.onpointerdown = (e) => { last = [e.offsetX, e.offsetY]; lastT = e.timeStamp; pad.setPointerCapture(e.pointerId); };
    pad.onpointermove = (e) => {
      if (!last) return;
      const q = [e.offsetX, e.offsetY];
      const v = Math.hypot(q[0] - last[0], q[1] - last[1]) / Math.max(1, e.timeStamp - lastT);
      // faster strokes draw thinner, as a pen does
      wd = wd * 0.7 + Math.max(1.1, 3.4 - v * 1.6) * 0.3;
      ctx.strokeStyle = ink; ctx.lineWidth = wd;
      ctx.beginPath(); ctx.moveTo(...last); ctx.lineTo(...q); ctx.stroke();
      last = q; lastT = e.timeStamp; inked = true;
    };
    pad.onpointerup = () => { last = null; };
    const showTyped = () => {
      typedShow.replaceChildren();
      if (typedIn.value.trim()) typedShow.append(typedCanvas(typedIn.value.trim(), ink));
    };
    typedIn.oninput = showTyped;
    file.onchange = async () => {
      const f = file.files[0];
      if (!f) return;
      picture = await inkOnly(f, ink);
      const img = el('img', { alt: 'Your signature, the paper taken away' });
      img.src = picture.toDataURL();
      dropShow.replaceChildren(img, el('small', { text: 'Choose another' }), file);
    };
    const inks = el('span', { class: 'pe-swatches', role: 'group', 'aria-label': 'Ink' });
    for (const [n, c] of [['Navy', '#1d2a44'], ['Black', '#111111'], ['Blue', '#1d4fb0']]) {
      const b = el('button', { type: 'button', 'aria-label': n, 'aria-pressed': String(c === ink), onclick: () => {
        ink = c;
        for (const x of inks.children) x.setAttribute('aria-pressed', String(x === b));
        showTyped();
      } });
      b.style.background = c;
      inks.append(b);
    }
    const keep = el('input', { type: 'checkbox', checked: !this.lib.reader && !this.lib.demo, disabled: this.lib.reader || this.lib.demo });
    const err = el('p', { role: 'alert' });
    const ok = el('button', { class: 'pe-btn go', type: 'button', text: 'Use this signature' });
    const cancel = el('button', { class: 'pe-btn quiet', type: 'button', text: 'Cancel', onclick: () => d.close() });
    ok.onclick = async () => {
      let canvas = null, text = null;
      if (tab === 'draw') { if (!inked) { err.textContent = 'Draw your signature in the box first.'; return; } canvas = trim(pad); }
      if (tab === 'type') { text = typedIn.value.trim(); if (!text) { err.textContent = 'Type your name first.'; return; } }
      if (tab === 'upload') { if (!picture) { err.textContent = 'Choose a picture first.'; return; } canvas = trim(picture); }
      const s = canvas
        ? { png: b64(await pngBytes(canvas)), ratio: canvas.width / canvas.height, kind: tab }
        : { text, ink, ratio: typedCanvas(text, ink).width / typedCanvas(text, ink).height, kind: 'type' };
      s.made = new Date().toISOString();
      this.signature = s;
      if (keep.checked) {
        this.signs.push(s);
        this.keepSigns().catch((e) => this.say(`Your signature was not kept: ${e.message}`));
      }
      d.close();
      this.inspector();
      const then = this.afterSign;
      this.afterSign = null;
      if (then) then();
      else this.say('Now click where it goes, or a yellow Sign here.');
    };
    d.append(
      el('div', { class: 'pe-dl-head' }, el('h2', { id: 'pe-sig-title', text: 'Add your signature' }), tabs),
      el('div', { class: 'pe-dl-body' },
        panes.draw, panes.type, panes.upload,
        el('div', { class: 'pe-row', style: 'justify-content:space-between' },
          el('span', { class: 'pe-row' }, el('span', { text: 'Ink', style: 'font-size:13px;color:var(--ink-2)' }), inks),
          el('button', { class: 'pe-btn quiet small', type: 'button', text: 'Clear', onclick: () => { if (tab === 'draw') sizePad(); else { typedIn.value = ''; showTyped(); } } })),
        el('label', { class: 'pe-check' }, keep, el('span', { text: 'Keep it for next time. The picture is stored in your library, locked like your files; the box cannot see it.' })),
        err),
      el('div', { class: 'pe-dl-foot' }, cancel, ok));
    d.onclose = () => { d.remove(); if (!this.signature) this.afterSign = null; };
    this.root.append(d);
    d.showModal();
    sizePad();
  }

  async chooseImage(fromLibrary) {
    let blob;
    if (fromLibrary) {
      const it = await pickFile(this.lib, { title: 'Choose a picture', want: /\.(png|jpe?g|webp|gif|bmp|avif)$/i, start: this.here(), host: this.root });
      if (!it) return;
      blob = new Blob([await fetchPlain(this.lib, it.path)]);
    } else {
      const f = await pickLocal('image/*');
      if (!f) return;
      blob = f;
    }
    const bmp = await createImageBitmap(blob);
    const max = 2000;
    const s = Math.min(1, max / Math.max(bmp.width, bmp.height));
    const c = el('canvas');
    c.width = Math.round(bmp.width * s);
    c.height = Math.round(bmp.height * s);
    c.getContext('2d').drawImage(bmp, 0, 0, c.width, c.height);
    this.image = { png: await pngBytes(c), ratio: c.width / c.height };
    this.inspector();
    this.say('Now drag on the page where the picture goes, or click to place it.');
  }

  async placeImage(p, r) {
    const im = this.image;
    if (!r) {
      const pt = this.lastDown || [p.w / 2 - 75, p.h / 2 - 50];
      r = [pt[0], pt[1], pt[0] + 150, pt[1] + 150 / im.ratio];
    } else {
      // kept to its own shape, inside what was dragged
      const w = r[2] - r[0], h = r[3] - r[1];
      if (w / h > im.ratio) r = [r[0], r[1], r[0] + h * im.ratio, r[3]];
      else r = [r[0], r[1], r[2], r[1] + w / im.ratio];
    }
    await this.pdf.add(p.i, { type: 'Stamp', rect: r, png: im.png, author: this.who });
    this.redraw(p.i);
  }

  async movePages(list, to) {
    if (this.readOnly) return;
    const n = this.pdf.count;
    const moving = [...new Set(list)].sort((a, b) => a - b);
    const rest = [...Array(n).keys()].filter((i) => !moving.includes(i));
    const at = rest.filter((i) => i < to).length;
    const order = [...rest.slice(0, at), ...moving, ...rest.slice(at)];
    if (order.every((v, i) => v === i)) return;
    this.pdf.reorder(order);
    this.picked = new Set(moving.map((_, k) => at + k));
    this.refresh();
  }

  deletePages(list) {
    if (this.readOnly) return;
    if (list.length >= this.pdf.count) { this.say('A PDF needs at least one page.'); return; }
    this.pdf.deletePages(list);
    this.picked.clear();
    this.refresh();
    this.signedWarning();
    this.say(`Deleted ${list.length === 1 ? 'a page' : `${list.length} pages`}. Undo brings ${list.length === 1 ? 'it' : 'them'} back.`);
  }

  rotatePages(list, by) {
    for (const i of list) this.pdf.rotate(i, by);
    this.refresh();
  }

  async insertFiles(files, at) {
    let n = 0;
    for (const f of files) {
      if (!/pdf$/i.test(f.type) && !/\.pdf$/i.test(f.name)) continue;
      n += await this.pdf.insertFrom(at + n, new Uint8Array(await f.arrayBuffer()));
    }
    this.refresh();
    if (n) this.say(`Added ${n} page${n === 1 ? '' : 's'}.`);
  }

  async insertFromLibrary(at) {
    const it = await pickFile(this.lib, { title: 'Add pages from a PDF', want: /\.pdf$/i, start: this.here(), host: this.root });
    if (!it) return;
    try {
      const n = await this.pdf.insertFrom(at, await fetchPlain(this.lib, it.path));
      this.refresh();
      this.say(`Added ${n} page${n === 1 ? '' : 's'} from ${it.name}.`);
    } catch (e) {
      this.say(e.password ? 'That PDF has a password; open it and save it without one first.' : `Not added: ${e.message}`);
    }
  }

  // pages out into new files beside this one
  async extract(groups, label) {
    const dir = this.item.path ? this.item.path.split('/').slice(0, -1).join('/') : await pickFolder(this.lib, { title: 'Where the new files go', start: this.folder, host: this.root });
    if (!dir) return;
    const stem = this.item.name.replace(/\.pdf$/i, '');
    let made = 0;
    for (const g of groups) {
      await saveInto(this.lib, dir, `${stem} (${label(g)}).pdf`, this.pdf.extract(g));
      made++;
    }
    this.saved?.(null);
    this.say(`Made ${made} new file${made === 1 ? '' : 's'} beside this one.`);
  }

  find(q) {
    this.hits = [];
    this.hitAt = -1;
    if (q) for (let i = 0; i < this.pdf.count; i++) for (const quads of this.pdf.search(i, q)) this.hits.push({ page: i, quads });
    for (const p of this.pages) if (p.drawn) this.overlay(p);
    return this.hits.length;
  }

  nextHit(dir = 1) {
    if (!this.hits.length) return;
    this.hitAt = (this.hitAt + dir + this.hits.length) % this.hits.length;
    const h = this.hits[this.hitAt];
    const p = this.pages[h.page];
    for (const x of this.pages) if (x.drawn) this.overlay(x);
    const r = quadRect(h.quads[0]);
    this.pagesEl.scrollTo({ top: p.box.offsetTop + r[1] * this.zoom - this.pagesEl.clientHeight / 3 });
    this.inspector();
  }

  async recognise(list) {
    const { recognise } = await import('./pdf-ocr.js');
    const scale = 300 / 72;
    this.ocr = { done: 0, of: list.length, now: 0 };
    this.inspector();
    try {
      for (const i of list) {
        const r = this.pdf.render(i, scale);
        const c = el('canvas');
        c.width = r.width;
        c.height = r.height;
        c.getContext('2d').putImageData(new ImageData(r.pixels, r.width, r.height), 0, 0);
        const words = await recognise(c, scale, (f) => { this.ocr.now = f; this.ocrBar(); });
        if (words.length) await this.pdf.addHiddenText(i, words);
        this.ocr.done++;
        this.ocr.now = 0;
        this.redraw(i);
        this.ocrBar();
      }
      this.say(`Read the text on ${list.length === 1 ? 'the page' : `${list.length} pages`}: it can be searched and copied now.`);
    } catch (e) {
      this.note('broken', `Text could not be read: ${e.message}`);
    }
    this.ocr = null;
    this.inspector();
  }

  ocrBar() {
    const bar = this.insp.querySelector('.pe-bar i');
    if (bar && this.ocr) bar.style.width = `${(100 * (this.ocr.done + this.ocr.now)) / this.ocr.of}%`;
  }

  setTool(t) {
    this.tool = t;
    for (const [k, b] of this.toolBtns) b.setAttribute('aria-pressed', String(k === t));
    if (t !== 'select') this.selected = null;
    if (t !== 'find') { this.hits = []; this.hitAt = -1; }
    this.textSel = null;
    this.notice = null;
    for (const p of this.pages) { p.box.dataset.tool = t; if (p.drawn) this.overlay(p); }
    this.inspector();
    if (t === 'sign' && !this.signature && this.signs.length) this.signature = this.signs.at(-1);
  }

  inspector() {
    if (!this.pdf) return;
    const box = this.insp;
    const keepFocus = box.contains(document.activeElement) ? document.activeElement.id : null;
    box.replaceChildren();
    // a section may come back as a list of them
    const I = { append: (...xs) => box.append(...xs.flat(Infinity).filter(Boolean)) };
    if (this.notice) {
      const n = this.notice;
      I.append(el('section', {},
        el('div', { class: 'pe-note', 'data-state': n.state, role: n.state === 'broken' ? 'alert' : 'status' }, n.text),
        n.actions.length ? el('div', { class: 'pe-row' }, ...n.actions.map(([l, f]) => el('button', { class: 'pe-btn small', type: 'button', text: l, onclick: () => { this.notice = null; f(); } }))) : null));
    }
    if (this.readOnly) I.append(el('div', { class: 'pe-note', 'data-state': 'working', text: 'A library shared with you: you can mark it up and download a copy. Only its owner saves changes here.' }));
    const sec = (title, ...kids) => el('section', {}, el('h2', { text: title }), ...kids);
    const p = (t) => el('p', { text: t });
    const btn = (label, fn, cls = '') => el('button', { class: `pe-btn small ${cls}`, type: 'button', text: label, onclick: fn });
    const colors = (list, now, set) => el('span', { class: 'pe-swatches', role: 'group', 'aria-label': 'Colour' },
      ...list.map(([n, c]) => { const b = el('button', { type: 'button', 'aria-label': n, 'aria-pressed': String(hex(c) === hex(now)), onclick: () => { set(c); this.inspector(); } }); b.style.background = hex(c); return b; }));
    const seg = (label, opts, now, set) => el('div', { class: 'pe-row' }, el('label', { text: label }),
      el('span', { class: 'pe-seg', role: 'group', 'aria-label': label },
        ...opts.map(([v, l]) => el('button', { type: 'button', text: l, 'aria-pressed': String(v === now), onclick: () => { set(v); this.inspector(); } }))));
    const widthRow = () => el('div', { class: 'pe-row' }, el('label', { for: 'pe-w', text: 'Width' }),
      el('input', { id: 'pe-w', type: 'range', min: '1', max: '12', value: String(this.width), oninput: (e) => { this.width = Number(e.target.value); } }));

    const t = this.tool;
    if (t === 'select') {
      if (this.selected) {
        const s = this.selected;
        const a = this.pdf.annots(s.page).find((o) => o.n === s.n);
        I.append(sec(labelOf(s.type),
          a?.text ? p(`“${a.text}”`) : null,
          !this.readOnly && !/Stamp|Redact/.test(s.type) ? el('div', { class: 'pe-row' }, el('label', { text: 'Colour' }), colors(/Highlight/.test(s.type) ? MARKERS : COLORS, a?.color || this.color, (c) => { this.pdf.styleAnnot(s.page, s.n, { color: c }); this.redraw(s.page); })) : null,
          !this.readOnly ? el('div', { class: 'pe-row' },
            /FreeText|Text/.test(s.type) ? btn('Change the words', () => this.editAnnotText(this.pages[s.page], a)) : null,
            btn('Delete', () => { this.pdf.deleteAnnot(s.page, s.n); this.selected = null; this.redraw(s.page); this.inspector(); }, 'danger')) : null,
          p('Drag it to move it; the corners resize it. Delete removes it.')));
      } else if (this.textSel) {
        I.append(this.textSelSection());
      } else {
        I.append(sec('Select', p('Click a mark to move, resize or delete it. Drag across words to copy, highlight or redact them. Double-click a text box to change it.')));
      }
      I.append(this.docSection());
    }
    if (t === 'text') {
      I.append(sec('Add text',
        p('Click where the words go, then type. Ctrl+Enter finishes.'),
        seg('Face', [['Sans', 'Sans'], ['Serif', 'Serif'], ['Mono', 'Mono']], this.family || 'Sans', (v) => { this.family = v; }),
        el('div', { class: 'pe-row' }, el('label', { for: 'pe-size', text: 'Size' }), el('input', { id: 'pe-size', type: 'number', min: '5', max: '96', value: String(this.size), style: 'width:76px', oninput: (e) => { this.size = Number(e.target.value) || 12; } }), el('span', { class: 'mono', text: 'pt', style: 'color:var(--ink-2);font-size:12px' })),
        el('div', { class: 'pe-row' }, el('label', { text: 'Colour' }), colors(COLORS, this.color, (c) => { this.color = c; })),
        el('label', { class: 'pe-check' }, el('input', { type: 'checkbox', checked: !!this.pageText, onchange: (e) => { this.pageText = e.target.checked; } }),
          el('span', { text: 'Part of the page: set into the page itself, not a box that can be moved. Use it to fill in a form that has no fields.' }))));
    }
    if (t === 'edittext') {
      const scans = [...Array(this.pdf.count).keys()].filter((i) => !this.pdf.hasText(i));
      I.append(sec('Edit text',
        p('Click a paragraph to change its words. They are set again in the nearest face we have to the PDF’s own, at its size and colour; each line keeps its place.'),
        p('Ctrl+Enter or clicking away finishes; Escape leaves it as it was.')));
      if (scans.length) I.append(this.scanSection(scans));
    }
    if (t === 'highlight') {
      I.append(sec('Mark words',
        p('Drag across the words.'),
        seg('Kind', [['Highlight', 'Highlight'], ['Underline', 'Underline'], ['StrikeOut', 'Strike'], ['Squiggly', 'Squiggle']], this.markup, (v) => { this.markup = v; }),
        el('div', { class: 'pe-row' }, el('label', { text: 'Colour' }), this.markup === 'Highlight' ? colors(MARKERS, this.marker, (c) => { this.marker = c; }) : colors(COLORS, this.color, (c) => { this.color = c; }))));
    }
    if (t === 'draw') {
      I.append(sec('Draw',
        p('Draw with the mouse, a finger or a pen.'),
        seg('Kind', [['pen', 'Pen'], ['marker', 'Marker']], this.inkKind || 'pen', (v) => { this.inkKind = v; }),
        el('div', { class: 'pe-row' }, el('label', { text: 'Colour' }), this.inkKind === 'marker' ? colors(MARKERS, this.marker, (c) => { this.marker = c; }) : colors(COLORS, this.color, (c) => { this.color = c; })),
        widthRow()));
    }
    if (t === 'shape') {
      I.append(sec('Shape',
        p('Drag on the page to draw it.'),
        seg('Kind', [['Square', 'Box'], ['Circle', 'Oval'], ['Line', 'Line'], ['Arrow', 'Arrow']], this.shape, (v) => { this.shape = v; }),
        el('div', { class: 'pe-row' }, el('label', { text: 'Colour' }), colors(COLORS, this.color, (c) => { this.color = c; })),
        widthRow(),
        /Square|Circle/.test(this.shape) ? el('label', { class: 'pe-check' }, el('input', { type: 'checkbox', checked: !!this.fill, onchange: (e) => { this.fill = e.target.checked; this.inspector(); } }), el('span', { text: 'Filled' })) : null,
        this.fill && /Square|Circle/.test(this.shape) ? el('div', { class: 'pe-row' }, el('label', { text: 'Fill' }), colors(MARKERS, this.marker, (c) => { this.marker = c; })) : null));
    }
    if (t === 'note') I.append(sec('Comment', p('Click to pin a comment. Other PDF readers show it as a note.'), this.commentsList()));
    if (t === 'image') {
      I.append(sec('Image',
        p('Put a picture on the page: a logo, a stamp, a photo of a receipt.'),
        el('div', { class: 'pe-row' }, btn('From your files', () => this.chooseImage(true)), btn('From this device', () => this.chooseImage(false))),
        this.image ? el('div', { class: 'pe-note', 'data-state': 'working', text: 'Picture ready: drag on the page where it goes.' }) : null));
    }
    if (t === 'form') I.append(this.formSection());
    if (t === 'addfield') {
      I.append(sec('Add a field',
        p('Drag on the page where the field goes. Anyone filling the PDF in, here or in another reader, can type into it.'),
        seg('Kind', [['text', 'Text'], ['paragraph', 'Paragraph'], ['checkbox', 'Check box'], ['date', 'Date'], ['signature', 'Signature']], this.fieldKind, (v) => { this.fieldKind = v; })));
    }
    if (t === 'sign') I.append(this.signSection());
    if (t === 'redact') I.append(this.redactSection());
    if (t === 'find') I.append(this.findSection());
    if (t === 'pages') I.append(this.pagesSection());
    if (keepFocus) document.getElementById(keepFocus)?.focus();
  }

  textSelSection() {
    const s = this.textSel;
    return el('section', {}, el('h2', { text: 'The words picked' }),
      el('p', { text: s.text.length > 160 ? `${s.text.slice(0, 160)}…` : s.text }),
      el('div', { class: 'pe-row' },
        el('button', { class: 'pe-btn small', type: 'button', text: 'Copy', onclick: async () => { try { await navigator.clipboard.writeText(s.text); this.say('Copied.'); } catch { this.say('This browser did not let the page copy; use Ctrl+C.'); } } }),
        this.readOnly ? null : el('button', { class: 'pe-btn small', type: 'button', text: 'Highlight', onclick: () => this.addMarkup(this.pages[s.page], s) }),
        this.readOnly ? null : el('button', { class: 'pe-btn small', type: 'button', text: 'Redact', onclick: () => this.addRedact(this.pages[s.page], { quads: s.quads }) })));
  }

  docSection() {
    const fields = [...Array(this.pdf.count).keys()].reduce((n, i) => n + this.pdf.fields(i).length, 0);
    const marks = [...Array(this.pdf.count).keys()].reduce((n, i) => n + this.pdf.annots(i).length, 0);
    const out = [el('section', {}, el('h2', { text: 'This document' }),
      el('div', { class: 'pe-list' },
        el('div', {}, el('span', { text: 'Pages' }), el('span', { text: String(this.pdf.count) })),
        el('div', {}, el('span', { text: 'Size' }), el('span', { text: human(this.pdf.bytes.length) })),
        el('div', {}, el('span', { text: 'Marks and comments' }), el('span', { text: String(marks) })),
        el('div', {}, el('span', { text: 'Form fields' }), el('span', { text: String(fields) })),
        el('div', {}, el('span', { text: 'Signatures' }), el('span', { text: this.checks ? String(this.checks.length) : '—' }))))];
    if (this.checks?.length) out.push(this.checksSection());
    return out;
  }

  checksSection() {
    return el('section', {}, el('h2', { text: 'Signatures in this file' }),
      ...this.checks.map((c) => {
        const k = c.check;
        const state = !k.ok ? 'broken' : k.whole ? 'ok' : 'attention';
        const head = k.ok ? `Signed by ${k.who}` : `Signature by ${k.who || 'someone'} did not check out`;
        const when = k.when ? ` on ${k.when.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' })}` : '';
        const body = !k.ok ? `Because ${k.why}.` : k.whole ? 'Nothing has changed since it was signed.' : 'The file was added to after it was signed. What was signed is unchanged; what came later is not covered.';
        // no certificate authority is checked here, so a name is only ever the signer's claim
        const vouch = k.ok ? (k.selfSigned ? ` The name is ${k.who}'s own claim; no authority vouches for it.` : ` Its certificate is from ${k.issuer || 'someone else'}, which this page does not check.`) : '';
        const day = (d) => d.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
        const dated = k.ok && k.from ? ` Its certificate runs ${day(k.from)} to ${day(k.until)}${k.inDate ? '' : ', and it was not in date when it signed'}.` : '';
        return el('div', { class: 'pe-note', 'data-state': k.ok && !k.inDate ? 'attention' : state }, el('b', { text: head + when }), el('br'), body + vouch + dated);
      }));
  }

  scanSection(scans) {
    return el('section', {}, el('h2', { text: 'Pictures of text' }),
      el('p', { text: `${scans.length === this.pdf.count ? 'Every page' : scans.length === 1 ? `Page ${scans[0] + 1}` : `${scans.length} pages`} ${scans.length === 1 && scans.length !== this.pdf.count ? 'is' : 'are'} a picture with no text in it, like a scan. Recognise text reads the words so they can be searched, copied, redacted and edited. It runs in this tab.` }),
      this.ocr
        ? el('div', { style: 'display:grid;gap:6px' }, el('div', { class: 'pe-bar', role: 'progressbar', 'aria-label': 'Reading the words' }, el('i')), el('p', { text: `Reading page ${this.ocr.done + 1} of ${this.ocr.of}…` }))
        : el('button', { class: 'pe-btn small', type: 'button', text: 'Recognise text', disabled: this.readOnly, onclick: () => this.recognise(scans) }));
  }

  commentsList() {
    const rows = [];
    for (let i = 0; i < this.pdf.count; i++) for (const a of this.pdf.annots(i)) if (a.type === 'Text' || (a.type === 'FreeText' && a.text)) rows.push({ i, a });
    if (!rows.length) return el('p', { text: 'No comments yet.' });
    return el('div', { class: 'pe-list' }, ...rows.map(({ i, a }) => el('button', { type: 'button', onclick: () => { this.setTool('select'); this.selected = { page: i, n: a.n, rect: a.rect, type: a.type }; this.goTo(i); this.overlay(this.pages[i]); this.inspector(); } },
      el('span', { text: (a.text || '').slice(0, 60) || '—' }), el('span', { text: `p. ${i + 1}` }))));
  }

  formSection() {
    const rows = [];
    for (let i = 0; i < this.pdf.count; i++) for (const f of this.pdf.fields(i)) if (f.type !== 'button') rows.push({ i, f });
    if (!rows.length) {
      return el('section', {}, el('h2', { text: 'Fill form' }),
        el('p', { text: 'This PDF has no fields. Use Text with “Part of the page” to write on it, or Add field to give it some.' }));
    }
    const empty = rows.filter(({ f }) => !f.readOnly && (f.type === 'signature' ? !f.signed : !f.value || f.value === 'Off')).length;
    return el('section', {}, el('h2', { text: 'Fill form' }),
      el('p', { text: `${rows.length} field${rows.length === 1 ? '' : 's'}, ${empty} still empty. Click one to fill it; Tab moves to the next.` }),
      el('button', { class: 'pe-btn small', type: 'button', text: 'Go to the next empty field', disabled: !empty || this.readOnly, onclick: () => {
        const next = rows.find(({ f }) => !f.readOnly && (f.type === 'signature' ? !f.signed : !f.value || f.value === 'Off'));
        if (next) { this.goTo(next.i); setTimeout(() => this.openField(this.pages[next.i], next.f), 60); }
      } }),
      el('div', { class: 'pe-list' }, ...rows.slice(0, 200).map(({ i, f }) => el('button', { type: 'button', onclick: () => { this.goTo(i); setTimeout(() => this.openField(this.pages[i], f), 60); } },
        el('span', { text: f.label || f.name || 'Field' }),
        el('span', { text: f.type === 'checkbox' || f.type === 'radiobutton' ? (f.checked ? 'ticked' : '—') : f.type === 'signature' ? (f.signed ? 'signed' : 'to sign') : (f.value ? f.value.slice(0, 18) : '—') })))));
  }

  signSection() {
    const out = [];
    const cards = el('div', { class: 'pe-sigs' });
    const all = this.signature && !this.signs.includes(this.signature) ? [...this.signs, this.signature] : this.signs;
    all.forEach((s) => {
      const ink = el('div', { class: 'ink' });
      if (s.png) { const img = el('img', { alt: '' }); img.src = `data:image/png;base64,${s.png}`; ink.append(img); } else ink.append(typedCanvas(s.text, s.ink, 40));
      const card = el('button', { class: 'pe-sigcard', type: 'button', 'aria-pressed': String(s === this.signature), 'aria-label': `Use this signature (${s.kind === 'type' ? 'typed' : s.kind === 'upload' ? 'from a picture' : 'drawn'})` }, ink,
        el('span', { text: `${s.kind === 'type' ? 'Typed' : s.kind === 'upload' ? 'From a picture' : 'Drawn'} · ${s.made ? new Date(s.made).toLocaleDateString() : ''}` }));
      card.onclick = () => { this.signature = s; this.inspector(); this.say('Now click where it goes, or a yellow Sign here.'); };
      const gone = this.signs.includes(s) ? el('button', { class: 'pe-btn quiet small gone', type: 'button', text: 'Forget', 'aria-label': 'Forget this signature', onclick: () => { this.signs = this.signs.filter((x) => x !== s); if (this.signature === s) this.signature = null; this.keepSigns(); this.inspector(); } }) : null;
      cards.append(el('div', { class: 'pe-sig-row' }, card, gone));
    });
    out.push(el('section', {}, el('h2', { text: 'Sign' }),
      el('p', { text: this.signature ? 'Click where it goes, or a yellow Sign here.' : 'Make your signature, then click where it goes.' }),
      cards,
      el('button', { class: 'pe-btn small', type: 'button', text: '+ New signature', disabled: this.readOnly, onclick: () => this.newSignature() })));
    const can = !this.lib.reader && !this.lib.demo;
    out.push(el('section', {}, el('h2', { text: 'Checkable signature' }),
      el('label', { class: 'pe-check' },
        el('input', { type: 'checkbox', checked: this.digital && can, disabled: !can || this.pendingSeal, onchange: (e) => { this.digital = e.target.checked; this.inspector(); } }),
        el('span', { text: 'Also sign with my key, so anyone can check who signed and that nothing changed afterwards.' })),
      this.pendingSeal
        ? el('div', { class: 'pe-note', 'data-state': 'working', text: 'Your key seals the file when you save it. Anything changed after saving is shown by PDF readers as a later change.' })
        : this.digital && can
          ? el('p', { text: 'Adobe Reader and others show who signed and that the file is unchanged. They also say no authority vouches for the name, which is true: you do.' })
          : el('p', { text: can ? 'Only the picture of your signature goes on the page.' : 'Signing with a key needs your own library.' })));
    if (this.checks?.length) out.push(this.checksSection());
    return out;
  }

  redactSection() {
    const marked = this.pdf.marked();
    const box = el('input', { id: 'pe-rq', type: 'search', placeholder: 'A word or a number', 'aria-label': 'Find words to redact' });
    const picks = Object.entries(PATTERNS).map(([k, [, l]]) => el('label', { class: 'pe-check' }, el('input', { type: 'checkbox', value: k }), el('span', { text: l })));
    const run = async () => {
      let n = 0;
      const res = [];
      if (box.value.trim()) res.push(new RegExp(box.value.trim().replace(/[.*+?^${}()|[\]\\]/g, '\\$&'), 'gi'));
      for (const l of picks) { const c = l.querySelector('input'); if (c.checked) res.push(new RegExp(PATTERNS[c.value][0].source, 'g')); }
      if (!res.length) return;
      for (let i = 0; i < this.pdf.count; i++) {
        let any = false;
        for (const re of res) for (const m of this.pdf.match(i, re)) { await this.pdf.add(i, { type: 'Redact', quads: m.quads }); n++; any = true; }
        if (any) this.redraw(i);
      }
      this.note(n ? 'attention' : 'ok', n ? `Marked ${n} place${n === 1 ? '' : 's'}. Look them over; they are removed for good when you save.` : 'Nothing found to mark.');
    };
    return el('section', {}, el('h2', { text: 'Redact' }),
      el('p', { text: 'What is under a redaction is removed from the file when you save: the words, the parts of pictures it covers, and any marks, comments or field values over it. Not just hidden. The file’s title and author details, and the hidden words some PDFs keep for screen readers, are removed too.' }),
      el('p', { text: 'Not touched: bookmark titles and files attached inside the PDF; look over those yourself. Save replaces the file, and the one before, unredacted, stays locked in your Trash for 30 days; empty it from the Trash to be rid of it sooner.' }),
      this.checks?.length ? el('div', { class: 'pe-note', 'data-state': 'attention', text: 'This file is signed. Redacting writes the whole file again so nothing redacted survives in an earlier version, and its signatures will no longer check.' }) : null,
      el('div', { class: 'pe-row' }, el('label', { text: 'By' }), el('span', { class: 'pe-seg', role: 'group', 'aria-label': 'Redact by' },
        ...[['area', 'Area'], ['text', 'Words']].map(([v, l]) => el('button', { type: 'button', text: l, 'aria-pressed': String(this.redactMode === v), onclick: () => { this.redactMode = v; this.inspector(); } })))),
      el('p', { text: this.redactMode === 'area' ? 'Drag a box over anything that must go.' : 'Drag across the words that must go.' }),
      el('h2', { text: 'Find and mark' }),
      box, ...picks,
      el('button', { class: 'pe-btn small', type: 'button', text: 'Mark every match', disabled: this.readOnly, onclick: run }),
      el('div', { class: 'pe-note', 'data-state': marked ? 'attention' : 'ok', text: marked ? `${marked} redaction${marked === 1 ? '' : 's'} marked, carried out when you save.` : 'Nothing marked yet.' }));
  }

  findSection() {
    const q = el('input', { id: 'pe-find', type: 'search', placeholder: 'Find in this PDF', 'aria-label': 'Find in this PDF', value: this.query || '' });
    const count = el('p', { role: 'status', text: this.query ? (this.hits.length ? `${this.hitAt + 1 || '—'} of ${this.hits.length}` : 'Not found.') : '' });
    q.oninput = () => { this.query = q.value; this.find(q.value.trim()); count.textContent = q.value.trim() ? (this.hits.length ? `${this.hits.length} found` : 'Not found.') : ''; };
    q.onkeydown = (e) => { if (e.key === 'Enter') { e.preventDefault(); this.nextHit(e.shiftKey ? -1 : 1); } };
    const scans = [...Array(this.pdf.count).keys()].filter((i) => !this.pdf.hasText(i));
    return [el('section', {}, el('h2', { text: 'Find' }), q, count,
      el('div', { class: 'pe-row' },
        el('button', { class: 'pe-btn small', type: 'button', text: '↑ Previous', onclick: () => this.nextHit(-1) }),
        el('button', { class: 'pe-btn small', type: 'button', text: '↓ Next', onclick: () => this.nextHit(1) })),
      !this.readOnly && this.hits.length ? el('div', { class: 'pe-row' },
        el('button', { class: 'pe-btn small', type: 'button', text: 'Highlight all', onclick: async () => { for (const h of this.hits) await this.pdf.add(h.page, { type: 'Highlight', quads: h.quads, color: this.marker }); this.refresh(); } }),
        el('button', { class: 'pe-btn small', type: 'button', text: 'Redact all', onclick: async () => { for (const h of this.hits) await this.pdf.add(h.page, { type: 'Redact', quads: h.quads }); this.hits = []; this.refresh(); this.note('attention', 'Marked for redaction; removed for good when you save.'); } })) : null),
    scans.length ? this.scanSection(scans) : null];
  }

  pagesSection() {
    const sel = this.picked.size ? [...this.picked].sort((a, b) => a - b) : [this.current];
    const label = sel.length === 1 ? `page ${sel[0] + 1}` : `${sel.length} pages`;
    const ro = this.readOnly;
    const b = (l, f, dis) => el('button', { class: 'pe-btn small', type: 'button', text: l, disabled: ro || dis, onclick: f });
    const every = el('input', { id: 'pe-every', type: 'number', min: '1', value: '1', style: 'width:64px', 'aria-label': 'Pages in each file' });
    return el('section', {}, el('h2', { text: 'Pages' }),
      el('p', { text: `Click a page in the rail to pick it; Ctrl or Shift picks more. Drag them to a new place, or drop a PDF from this device between them. Working on ${label}.` }),
      el('div', { class: 'pe-row' },
        b('↺ Rotate left', () => this.rotatePages(sel, -90)),
        b('↻ Rotate right', () => this.rotatePages(sel, 90))),
      el('div', { class: 'pe-row' },
        b('Duplicate', () => { for (const i of [...sel].reverse()) this.pdf.duplicate(i); this.refresh(); }),
        b('Delete', () => this.deletePages(sel), sel.length >= this.pdf.count)),
      el('div', { class: 'pe-row' },
        b('Move up', () => this.movePages(sel, sel[0] - 1), sel[0] === 0),
        b('Move down', () => this.movePages(sel, sel.at(-1) + 2), sel.at(-1) >= this.pdf.count - 1)),
      el('h2', { text: 'Add pages' }),
      el('div', { class: 'pe-row' },
        b('Blank page after', () => { this.pdf.insertBlank(sel.at(-1) + 1); this.refresh(); }),
        b('From your files', () => this.insertFromLibrary(sel.at(-1) + 1)),
        b('From this device', async () => { const f = await pickLocal('application/pdf', true); if (f) this.insertFiles(f, sel.at(-1) + 1); })),
      el('h2', { text: 'Into new files' }),
      el('div', { class: 'pe-row' },
        b(`Copy ${label} out`, () => this.extract([sel], (g) => (g.length === 1 ? `page ${g[0] + 1}` : `pages ${g[0] + 1}-${g.at(-1) + 1}`)))),
      el('div', { class: 'pe-row' },
        el('label', { for: 'pe-every', text: 'Split every' }), every, el('span', { text: 'pages', style: 'font-size:13px;color:var(--ink-2)' }),
        b('Split', () => {
          const n = Math.max(1, Number(every.value) || 1);
          const groups = [];
          for (let i = 0; i < this.pdf.count; i += n) groups.push([...Array(Math.min(n, this.pdf.count - i)).keys()].map((k) => i + k));
          this.extract(groups, (g) => (g.length === 1 ? `page ${g[0] + 1}` : `pages ${g[0] + 1}-${g.at(-1) + 1}`));
        })),
      el('p', { text: 'New files are saved beside this one at once; this one is not changed.' }));
  }

  undo() {
    if (!this.pdf?.canUndo()) return;
    this.pdf.undo();
    if (this.pendingSeal && !this.pdf.signatures().some((s) => s.range?.[1] === sig.RANGE_MARK)) this.pendingSeal = false;
    this.refresh();
  }

  redo() {
    if (!this.pdf?.canRedo()) return;
    this.pdf.redo();
    if (this.pdf.signatures().some((s) => s.range?.[1] === sig.RANGE_MARK)) this.pendingSeal = true;
    this.refresh();
  }

  steps() {
    try { return this.pdf.doc.getJournal().position - this.stamp; } catch { return this.pdf.changed() ? 1 : 0; }
  }

  dirty() { return !!this.pdf && (this.steps() > 0 || this.pendingSeal); }

  state(override) {
    if (!this.pdf) return;
    const n = Math.max(0, this.steps());
    const s = this.stateEl;
    if (override) { s.dataset.state = override[0]; s.textContent = override[1]; return; }
    if (n) { s.dataset.state = 'attention'; s.textContent = `${n} unsaved change${n === 1 ? '' : 's'}`; }
    else if (this.checks?.some((c) => !c.check.ok)) { s.dataset.state = 'broken'; s.textContent = 'A signature does not check out'; }
    // green only when every signature covers the whole file; the signer's
    // name is never checked against an authority, so it is not claimed here
    else if (this.checks?.some((c) => !c.check.whole)) { s.dataset.state = 'attention'; s.textContent = 'Signed · added to after signing'; }
    else if (this.checks?.length) { s.dataset.state = 'ok'; s.textContent = 'Signed · unchanged since'; }
    else { s.dataset.state = 'ok'; s.textContent = 'No changes'; }
    this.undoBtn.disabled = !this.pdf.canUndo();
    this.redoBtn.disabled = !this.pdf.canRedo();
    // a document not in Files yet can always be saved there, changed or not
    if (!this.readOnly) this.saveBtn.disabled = !n && !this.pendingSeal && !!this.item.path;
  }

  key(e) {
    if (!this.root.isConnected || this.root.querySelector('dialog[open]')) return;
    const typing = e.target.closest?.('input, textarea, select, [contenteditable]');
    const mod = e.ctrlKey || e.metaKey;
    if (mod && e.key.toLowerCase() === 's') { e.preventDefault(); if (!this.readOnly && this.dirty()) this.saveAs('replace'); return; }
    if (mod && e.key.toLowerCase() === 'f') { e.preventDefault(); this.setTool('find'); this.insp.querySelector('#pe-find')?.focus(); return; }
    if (typing) return;
    if (mod && e.key.toLowerCase() === 'z') { e.preventDefault(); e.shiftKey ? this.redo() : this.undo(); return; }
    if (mod && e.key.toLowerCase() === 'y') { e.preventDefault(); this.redo(); return; }
    if (mod && e.key.toLowerCase() === 'c' && this.textSel) { navigator.clipboard?.writeText(this.textSel.text).catch(() => {}); return; }
    if (e.key === 'Escape') {
      if (!this.menu.hidden) { this.menu.hidden = true; this.more.focus(); return; }
      if (this.selected || this.textSel) { this.selected = null; this.textSel = null; for (const p of this.pages) if (p.drawn) this.overlay(p); this.inspector(); return; }
      if (this.tool !== 'select') { this.setTool('select'); return; }
      this.close();
      return;
    }
    if ((e.key === 'Delete' || e.key === 'Backspace') && this.selected && !this.readOnly) {
      e.preventDefault();
      const s = this.selected;
      this.pdf.deleteAnnot(s.page, s.n);
      this.selected = null;
      this.redraw(s.page);
      this.inspector();
      return;
    }
    if (mod || e.altKey) return;
    if (e.key === '+' || e.key === '=') { this.setZoom(this.zoom * 1.15); return; }
    if (e.key === '-') { this.setZoom(this.zoom / 1.15); return; }
    if (e.target.closest?.('.pe-insp, .pe-thumbs')) return;
    const t = KEYS[e.key.toLowerCase()];
    if (t) this.setTool(t);
  }

  // What is open now is what was saved: a sealed file is opened afresh, so
  // the next change goes after its signature, not into it
  async adopt(bytes) {
    const reopen = await Pdf.open(bytes);
    this.pdf.destroy();
    this.pdf = reopen;
    this.pendingSeal = false;
    this.stamp = 0;
    try { this.stamp = this.pdf.doc.getJournal().position; } catch { /* */ }
    this.layout();
    await this.checkSignatures();
  }

  async saveAs(how, action) {
    if (this.saving) return;
    this.saving = true;
    this.menu.hidden = true;
    try {
      this.state(['working', how === 'download' ? 'Writing the PDF…' : 'Writing the PDF…']);
      let bytes;
      const seal = this.pendingSeal && how !== 'flatten';
      if (seal) {
        bytes = this.pdf.saveForSigning();
        this.state(['working', 'Sealing with your key…']);
        bytes = await sig.seal(bytes, await this.ensureIdentity(), this.sealTag);
      } else {
        if (this.pendingSeal && how === 'flatten') { this.say('A flattened copy cannot carry a checkable signature; only its picture stays.'); }
        bytes = this.pdf.save({ flatten: how === 'flatten', smaller: how === 'smaller' });
      }
      if (how === 'download') {
        toDevice(bytes, this.item.name);
        this.state(['ok', 'Downloaded']);
        return;
      }
      if (how === 'action') {
        await action.run(bytes, this.item.name);
        if (seal) await this.adopt(bytes);
        this.state(['ok', 'Done']);
        return;
      }
      const loose = !this.item.path;
      const dir = loose ? await pickFolder(this.lib, { title: `Save ${this.item.name} to Files`, start: this.folder, host: this.root }) : this.here();
      if (!dir) { this.state(); return; }
      this.state(['working', `Locking and sending ${human(bytes.length)}…`]);
      if (loose || how === 'copy' || how === 'flatten' || how === 'smaller') {
        const stem = this.item.name.replace(/\.pdf$/i, '');
        const word = how === 'copy' ? (seal ? 'signed' : 'edited') : how === 'flatten' ? 'flattened' : 'smaller';
        const path = await saveInto(this.lib, dir, loose && how === 'replace' ? this.item.name : `${stem} (${word}).pdf`, bytes);
        const name = path.split('/').pop();
        this.saved?.(path);
        if (loose && how === 'replace') {
          // it is a file in Files now: the next Save replaces it there
          this.item = { path, name };
          this.root.querySelector('.pe-title h1').textContent = name;
          this.whereEl.textContent = ['My files', ...path.split('/').slice(1, -1)].join(' / ');
          await this.adopt(bytes);
          this.state(['ok', `Saved to ${this.whereEl.textContent}`]);
          return;
        }
        this.state(['ok', `Saved as ${name}`]);
        this.say(`Saved as ${name}${loose ? '' : ', beside it'}. This one is still open as it was.`);
        return;
      }
      // in place: the new file first under a name of its own, then the old
      // one to the Trash, then the new one into its place, so nothing is lost
      // if the tab goes away part way
      const temp = `${dir}/.${this.item.name}.saving-${Date.now()}`;
      await put(this.lib, temp, new Blob([bytes]));
      await trash(this.lib, this.item.path);
      await move(this.lib, temp, this.item.path);
      this.saved?.(this.item.path);
      await this.adopt(bytes);
      this.state(['ok', `Saved · ${new Date().toTimeString().slice(0, 5)}`]);
      this.say('Saved. The old one is in the Trash for 30 days.');
    } catch (e) {
      this.state(['broken', 'Not saved']);
      this.note('broken', `It was not saved: ${e.message}. Your changes are still here; try again.`);
    } finally {
      this.saving = false;
      if (this.pdf) { this.undoBtn.disabled = !this.pdf.canUndo(); this.redoBtn.disabled = !this.pdf.canRedo(); }
    }
  }

  async close(force) {
    if (!force && this.dirty()) {
      const r = await this.ask({ title: 'Leave without saving?', text: `${this.steps() || 1} change${this.steps() === 1 ? '' : 's'} to ${this.item.name} would be lost.`, ok: 'Leave without saving', danger: true, cancel: 'Keep editing' });
      if (r === null) return;
    }
    removeEventListener('keydown', this.keyHandler, true);
    removeEventListener('beforeunload', this.leaveHandler);
    this.io?.disconnect();
    this.pdf?.destroy();
    this.root.remove();
    document.documentElement.style.overflow = '';
    import('./pdf-ocr.js').then((m) => m.stop()).catch(() => {});
  }

  ask({ title, text = '', value = null, ok = 'OK', danger = false, cancel = 'Cancel', type = 'text' }) {
    return new Promise((resolve) => {
      const d = el('dialog', { 'aria-labelledby': 'pe-ask-t' });
      const input = value === null ? null : el('input', { type, value, 'aria-labelledby': 'pe-ask-t', autocomplete: 'off' });
      const yes = el('button', { class: `pe-btn ${danger ? 'danger' : 'go'}`, type: 'button', text: ok });
      let answer = null;
      yes.onclick = () => { answer = input ? input.value.trim() : ''; if (input && !answer) return; d.close(); };
      if (input) input.onkeydown = (e) => { if (e.key === 'Enter') { e.preventDefault(); yes.click(); } };
      d.append(
        el('div', { class: 'pe-dl-head' }, el('h2', { id: 'pe-ask-t', text: title })),
        el('div', { class: 'pe-dl-body' }, text ? el('p', { text }) : null, input),
        el('div', { class: 'pe-dl-foot' }, el('button', { class: 'pe-btn quiet', type: 'button', text: cancel, onclick: () => d.close() }), yes));
      d.onclose = () => { d.remove(); resolve(answer); };
      this.root.append(d);
      d.showModal();
      (input || yes).focus();
    });
  }

  // the folder this file is in, or where a new one would go
  here() { return this.item.path ? this.item.path.split('/').slice(0, -1).join('/') : this.folder; }
}

function quadRect(q) {
  const xs = [q[0], q[2], q[4], q[6]], ys = [q[1], q[3], q[5], q[7]];
  return [Math.min(...xs), Math.min(...ys), Math.max(...xs), Math.max(...ys)];
}

function labelOf(type) {
  return { FreeText: 'Text box', Text: 'Comment', Highlight: 'Highlight', Underline: 'Underline', StrikeOut: 'Strike-out', Squiggly: 'Squiggle', Ink: 'Drawing', Square: 'Box', Circle: 'Oval', Line: 'Line', Stamp: 'Picture', Redact: 'Redaction', Widget: 'Field' }[type] || type;
}

// a pen's points, eased so a shaky hand draws a smooth line
function smooth(pts) {
  if (pts.length < 3) return pts;
  const out = [pts[0]];
  for (let i = 1; i < pts.length - 1; i++) out.push([(pts[i - 1][0] + 2 * pts[i][0] + pts[i + 1][0]) / 4, (pts[i - 1][1] + 2 * pts[i][1] + pts[i + 1][1]) / 4]);
  out.push(pts.at(-1));
  return out;
}

// a typed signature, drawn in a handwriting face if this device has one
function typedCanvas(text, ink, height = 120) {
  const c = document.createElement('canvas');
  const x = c.getContext('2d');
  const font = `italic ${height * 0.55}px "Segoe Script", "Snell Roundhand", "Brush Script MT", "URW Chancery L", cursive`;
  x.font = font;
  const w = Math.ceil(x.measureText(text).width + height * 0.4);
  c.width = Math.max(40, w);
  c.height = height;
  x.font = font;
  x.fillStyle = ink;
  x.textBaseline = 'middle';
  x.fillText(text, height * 0.2, height / 2);
  return c;
}

// a photo of a signature on paper: the paper made see-through, the ink kept
async function inkOnly(file, ink) {
  const bmp = await createImageBitmap(file);
  const s = Math.min(1, 1600 / Math.max(bmp.width, bmp.height));
  const c = document.createElement('canvas');
  c.width = Math.round(bmp.width * s);
  c.height = Math.round(bmp.height * s);
  const x = c.getContext('2d');
  x.drawImage(bmp, 0, 0, c.width, c.height);
  const d = x.getImageData(0, 0, c.width, c.height);
  // the paper's brightness, from the brightest tenth of the picture
  const lum = [];
  for (let i = 0; i < d.data.length; i += 16) lum.push((d.data[i] + d.data[i + 1] + d.data[i + 2]) / 3);
  lum.sort((a, b) => a - b);
  const paper = lum[Math.floor(lum.length * 0.9)] || 255;
  const cut = paper * 0.72;
  const [r, g, b] = [1, 3, 5].map((k) => parseInt(ink.slice(k, k + 2), 16));
  for (let i = 0; i < d.data.length; i += 4) {
    const l = (d.data[i] + d.data[i + 1] + d.data[i + 2]) / 3;
    const a = l >= cut ? 0 : Math.min(255, ((cut - l) / cut) * 255 * 2.2);
    d.data[i] = r; d.data[i + 1] = g; d.data[i + 2] = b; d.data[i + 3] = a;
  }
  x.putImageData(d, 0, 0);
  return c;
}

// the drawn part of a canvas, without the empty margin around it
function trim(cv) {
  const x = cv.getContext('2d');
  const { width: w, height: h } = cv;
  const d = x.getImageData(0, 0, w, h).data;
  let t = h, l = w, r = 0, b = 0;
  for (let y = 0; y < h; y++) for (let i = 0; i < w; i++) if (d[(y * w + i) * 4 + 3] > 8) { if (y < t) t = y; if (y > b) b = y; if (i < l) l = i; if (i > r) r = i; }
  if (r <= l || b <= t) return cv;
  const pad = 6;
  const o = document.createElement('canvas');
  o.width = r - l + 2 * pad;
  o.height = b - t + 2 * pad;
  o.getContext('2d').drawImage(cv, l - pad, t - pad, o.width, o.height, 0, 0, o.width, o.height);
  return o;
}

function pickLocal(accept, multiple = false) {
  return new Promise((resolve) => {
    const i = document.createElement('input');
    i.type = 'file';
    i.accept = accept;
    i.multiple = multiple;
    i.onchange = () => resolve(multiple ? [...i.files] : i.files[0] || null);
    i.oncancel = () => resolve(null);
    i.click();
  });
}

// the device's signing identity, in IndexedDB (a CryptoKey is kept as it is)
function idStore() {
  return new Promise((ok, no) => {
    const r = indexedDB.open('commonty-signing', 1);
    r.onupgradeneeded = () => r.result.createObjectStore('identity');
    r.onsuccess = () => ok(r.result);
    r.onerror = () => no(r.error);
  });
}
// kept per account: another member signing on this device has their own
async function idKept(account) {
  try {
    const db = await idStore();
    return await new Promise((ok) => {
      const q = db.transaction('identity').objectStore('identity').get(`account:${account}`);
      q.onsuccess = () => ok(q.result?.account === account ? q.result : null);
      q.onerror = () => ok(null);
    });
  } catch { return null; }
}
async function idKeep(account, kept) {
  try {
    const db = await idStore();
    return await new Promise((ok) => {
      const t = db.transaction('identity', 'readwrite');
      t.objectStore('identity').put({ ...kept, account }, `account:${account}`);
      t.oncomplete = () => ok(true);
      t.onerror = () => ok(false);
    });
  } catch { return false; }
}
