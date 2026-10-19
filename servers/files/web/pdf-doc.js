// A PDF open for editing, on MuPDF (WebAssembly, pinned beside our own wasm
// in flake.nix and loaded the first time Edit is clicked). Everything that
// changes the file is here: marks, text, forms, pages, redaction, the
// signature's field and its hole, the text a scan was read into, and
// saving. Coordinates are the page's own points, from its top left, y down:
// what MuPDF's annotations take and what the editor draws in.

import { HOLE, RANGE_MARK, nonce } from './pdf-sign.js';

let mu = null;
let fontBytes = async (file) => {
  const r = await fetch(`/_dd/web/${file}`);
  if (!r.ok) throw new Error(`a font did not load (${r.status})`);
  return new Uint8Array(await r.arrayBuffer());
};

export async function engine() {
  mu ??= await import('/_dd/web/mupdf.js');
  return mu;
}
// tests run the same code under node with the npm build and files on disk
export function useEngine(m, fonts) { mu = m; if (fonts) fontBytes = fonts; }

// Courier, so text set again in them takes the room the old text took

const FACES = {};
async function face(family, bold, italic) {
  const style = bold && italic ? 'BoldItalic' : bold ? 'Bold' : italic ? 'Italic' : 'Regular';
  const file = `Liberation${family}-${style}.ttf`;
  FACES[file] ??= fontBytes(file).then((b) => new mu.Font(`Liberation${family}-${style}`, b));
  return FACES[file];
}

// which of our faces stands in for a font the PDF names
export function familyOf(fontName, font) {
  const n = (fontName || '').replace(/^[A-Z]{6}\+/, '');
  if (/mono|courier|consol|cmtt|lmmono|typewriter|menlo|inconsolata/i.test(n) || font?.isMono?.()) return 'Mono';
  if (/sans|arial|helvet|verdana|calibri|segoe|roboto|inter|cmss|lmsans|gothic|tahoma|trebuchet|aptos|lato|noto ?sans|open ?sans/i.test(n)) return 'Sans';
  // a name that says nothing (F37, T1_0: TeX's and some printers') is
  // nearly always a book face
  return 'Serif';
}
const isBold = (n, f) => /bold|black|heavy|semibold|cmbx|-bd|demi/i.test(n || '') || !!f?.isBold?.();
const isItalic = (n, f) => /italic|oblique|cmti|cmsl|-it\b/i.test(n || '') || !!f?.isItalic?.();

// ligatures and look-alikes a font of ours may not hold as one glyph
const LIGATURES = { '\ufb00': 'ff', '\ufb01': 'fi', '\ufb02': 'fl', '\ufb03': 'ffi', '\ufb04': 'ffl', '\ufb05': 'st', '\ufb06': 'st', '\u2010': '-', '\u2011': '-', '\u00ad': '' };
export const plainText = (s) => s.replace(/[\ufb00-\ufb06\u2010\u2011\u00ad]/g, (c) => LIGATURES[c]);
// TeX's fonts put their ligatures at control codes (T1 at 1b-1f, OT1 at
// 0b-0f) and a PDF often does not say what they are; no text uses those codes
const CONTROL = { '\x1b': 'ff', '\x1c': 'fi', '\x1d': 'fl', '\x1e': 'ffi', '\x1f': 'ffl', '\x0b': 'ff', '\x0c': 'fi', '\x0d': 'fl', '\x0e': 'ffi', '\x0f': 'ffl' };
// a character with no meaning the file gives: a bullet, a symbol, a lost glyph
const BULLETS = /^[\u2022\u00b7\u25aa\u25e6\u2023\u2219\u2013\u2014*-]$/;
const opaque = (c) => {
  const n = c.codePointAt(0);
  return n < 0x20 || (n >= 0x7f && n <= 0x9f) || (n >= 0xe000 && n <= 0xf8ff) || n === 0xfffd;
};

// A line's text, and where each character starts in it: a ligature set
// out as two letters ("fi") makes the text longer than the characters, so
// a place in the text is turned back into a character through this
function lineText(l) {
  l.at = [];
  let n = 0;
  for (const c of l.chars) { l.at.push(n); n += c.c.length; }
  l.text = l.chars.map((c) => c.c).join('');
  return l;
}
// the character a place in a line's text falls in
const charOf = (l, off) => {
  let k = 0;
  while (k + 1 < l.at.length && l.at[k + 1] <= off) k++;
  return k;
};

export class Pdf {
  constructor(doc, bytes) {
    this.doc = doc;
    this.bytes = bytes;                 // as opened: signatures are checked against these
    this.fieldSerial = 0;
  }

  /// A new PDF of pictures, a page each, as a scan is: pages are A4 wide
  /// and as tall as each picture's shape. images: [{ bytes, width, height }]
  /// (JPEG or PNG). The document opens for editing like any other.
  static async fromImages(images) {
    await engine();
    const doc = new mu.PDFDocument();
    for (const im of images) {
      const w = 595.28;
      const h = (w * im.height) / im.width;
      const ref = doc.addImage(new mu.Image(im.bytes));
      const page = doc.addPage([0, 0, w, h], 0, { XObject: { Im0: ref } }, `q ${w.toFixed(2)} 0 0 ${h.toFixed(2)} 0 0 cm /Im0 Do Q`);
      doc.insertPage(-1, page);
    }
    const bytes = doc.saveToBuffer('garbage,compress').asUint8Array().slice();
    return Pdf.open(bytes);
  }

  /// Open `bytes`. A file with a password: { password: true } is thrown,
  /// and open again with `password`.
  static async open(bytes, password) {
    await engine();
    const any = mu.Document.openDocument(bytes, 'application/pdf');
    if (any.needsPassword()) {
      if (!password || !any.authenticatePassword(password)) throw Object.assign(new Error('password'), { password: true, wrong: !!password });
    }
    const doc = any.asPDF();
    if (!doc) throw new Error('this is not a PDF');
    doc.enableJournal();
    return new Pdf(doc, bytes);
  }

  get count() { return this.doc.countPages(); }
  page(i) { return this.doc.loadPage(i); }

  /// [width, height] in points, as shown (rotation applied)
  size(i) {
    const b = this.page(i).getBounds();
    return [b[2] - b[0], b[3] - b[1]];
  }

  /// the page drawn at `scale`, marks and fields included: RGBA pixels
  render(i, scale) {
    const pm = this.page(i).toPixmap(mu.Matrix.scale(scale, scale), mu.ColorSpace.DeviceRGB, true, true);
    const out = { width: pm.getWidth(), height: pm.getHeight(), pixels: new Uint8ClampedArray(pm.getPixels()) };
    pm.destroy?.();
    return out;
  }

  /// small, for the pages rail
  thumb(i, width = 120) {
    const [w] = this.size(i);
    return this.render(i, width / w);
  }

  step(label, fn) {
    this.doc.beginOperation(label);
    try {
      const r = fn();
      this.doc.endOperation();
      return r;
    } catch (e) {
      this.doc.abandonOperation();
      throw e;
    }
  }
  canUndo() { return this.doc.canUndo(); }
  canRedo() { return this.doc.canRedo(); }
  undo() { this.doc.undo(); }
  redo() { this.doc.redo(); }
  changed() { return this.doc.hasUnsavedChanges(); }

  text(i) {
    const blocks = [];
    let blk = null, line = null;
    this.page(i).toStructuredText('preserve-whitespace,preserve-ligatures').walk({
      beginTextBlock(bbox) { blk = { bbox, lines: [] }; blocks.push(blk); },
      beginLine(bbox, _wmode, dir) { line = { bbox, dir, chars: [] }; blk.lines.push(line); },
      onChar(c, origin, font, size, quad, color) { line.chars.push({ c: CONTROL[c] || c, origin, font, size, quad, color }); },
    });
    for (const b of blocks) for (const l of b.lines) lineText(l);
    return blocks;
  }

  /// every place `needle` is on page i, as quads (one list per hit)
  search(i, needle) { return this.page(i).search(needle); }

  /// every place a pattern matches on page i: [{ text, quads }]
  match(i, re) {
    const out = [];
    for (const b of this.text(i)) {
      for (const l of b.lines) {
        re.lastIndex = 0;
        let m = re.exec(l.text);
        while (m) {
          if (!m[0]) re.lastIndex++;
          else out.push({ text: m[0], quads: l.chars.slice(charOf(l, m.index), charOf(l, m.index + m[0].length - 1) + 1).map((c) => c.quad) });
          if (!re.global) break;
          m = re.exec(l.text);
        }
      }
    }
    return out;
  }

  /// the words between two points, for selecting with the pointer
  select(i, a, b) {
    const st = this.page(i).toStructuredText('preserve-whitespace');
    return { text: st.copy(a, b), quads: st.highlight(a, b) };
  }

  annots(i) {
    return this.page(i).getAnnotations().map((a, n) => {
      const type = a.getType();
      let rect;
      try { rect = a.getRect(); } catch { rect = a.getBounds(); }
      const o = { n, type, rect };
      try { o.color = a.getColor(); } catch { /* not every kind has one */ }
      try { o.text = a.getContents(); } catch { /* */ }
      return o;
    }).filter((a) => a.type !== 'Popup' && a.type !== 'Link');
  }

  /// a new mark: spec.type is one of FreeText, Highlight, Underline,
  /// StrikeOut, Squiggly, Ink, Square, Circle, Line, Text, Stamp, Redact
  async add(i, spec) {
    const image = spec.type === 'Stamp' ? await this.image(spec.png) : null;
    return this.step(`add ${spec.type}`, () => {
      const page = this.page(i);
      const a = page.createAnnotation(spec.type);
      const color = spec.color || [0, 0, 0];
      switch (spec.type) {
        case 'FreeText':
          a.setRect(spec.rect);
          a.setContents(spec.text || '');
          a.setDefaultAppearance(spec.font || 'Helv', spec.size || 12, color);
          if (spec.fill) a.setInteriorColor(spec.fill);
          a.setBorderWidth(0);
          break;
        case 'Highlight': case 'Underline': case 'StrikeOut': case 'Squiggly':
          a.setColor(color);
          a.setQuadPoints(spec.quads);
          if (spec.type === 'Highlight') a.setOpacity?.(spec.opacity ?? 1);
          break;
        case 'Ink':
          a.setColor(color);
          a.setBorderWidth(spec.width || 2);
          a.setInkList(spec.paths);
          if (spec.opacity !== undefined) a.setOpacity?.(spec.opacity);
          break;
        case 'Square': case 'Circle':
          a.setRect(spec.rect);
          a.setColor(color);
          a.setBorderWidth(spec.width || 2);
          if (spec.fill) a.setInteriorColor(spec.fill);
          break;
        case 'Line':
          a.setLine(spec.a, spec.b);
          a.setColor(color);
          a.setBorderWidth(spec.width || 2);
          if (spec.arrow) a.setLineEndingStyles('None', 'OpenArrow');
          break;
        case 'Text':
          a.setRect([spec.at[0], spec.at[1], spec.at[0] + 20, spec.at[1] + 20]);
          a.setContents(spec.text || '');
          a.setColor(spec.color || [1, 0.84, 0.04]);
          a.setIcon('Comment');
          break;
        case 'Stamp':
          a.setRect(spec.rect);
          a.setStampImage(image);
          break;
        case 'Redact':
          if (spec.quads) a.setQuadPoints(spec.quads);
          else a.setRect(spec.rect);
          break;
        default:
          throw new Error(`no mark called ${spec.type}`);
      }
      if (spec.author) a.setAuthor?.(spec.author);
      a.update();
      return page.getAnnotations().length - 1;
    });
  }

  async image(png) {
    await engine();
    return new mu.Image(png);
  }

  // the mark itself, by its place in the page's list
  annot(i, n) { return this.page(i).getAnnotations()[n]; }

  /// move a mark by dx, dy points
  moveAnnot(i, n, dx, dy) {
    this.step('move', () => {
      const a = this.annot(i, n);
      const t = a.getType();
      if (t === 'Ink') a.setInkList(a.getInkList().map((s) => s.map(([x, y]) => [x + dx, y + dy])));
      else if (t === 'Line') { const [p, q] = a.getLine(); a.setLine([p[0] + dx, p[1] + dy], [q[0] + dx, q[1] + dy]); }
      else if (/Highlight|Underline|StrikeOut|Squiggly/.test(t) || (t === 'Redact' && a.hasQuadPoints?.())) {
        a.setQuadPoints(a.getQuadPoints().map((q) => q.map((v, k) => v + (k % 2 ? dy : dx))));
      } else {
        const r = a.getRect();
        a.setRect([r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy]);
      }
      a.update();
    });
  }

  /// a new rect for a mark that has one (a box, a picture, a text box)
  resizeAnnot(i, n, rect) {
    this.step('resize', () => {
      const a = this.annot(i, n);
      if (a.getType() === 'Ink') {
        const r = a.getRect();
        const sx = (rect[2] - rect[0]) / Math.max(1, r[2] - r[0]), sy = (rect[3] - rect[1]) / Math.max(1, r[3] - r[1]);
        a.setInkList(a.getInkList().map((s) => s.map(([x, y]) => [rect[0] + (x - r[0]) * sx, rect[1] + (y - r[1]) * sy])));
      } else a.setRect(rect);
      a.update();
    });
  }

  styleAnnot(i, n, { color, text, width, size }) {
    this.step('change', () => {
      const a = this.annot(i, n);
      if (color) {
        if (a.getType() === 'FreeText') a.setDefaultAppearance('Helv', size || 12, color);
        else a.setColor(color);
      }
      if (text !== undefined) a.setContents(text);
      if (width) a.setBorderWidth(width);
      a.update();
    });
  }

  deleteAnnot(i, n) {
    this.step('delete', () => {
      const page = this.page(i);
      page.deleteAnnotation(page.getAnnotations()[n]);
    });
  }

  fields(i) {
    return this.page(i).getWidgets().map((w, n) => ({
      n,
      name: w.getName(),
      label: w.getLabel?.() || '',
      type: w.getFieldType(),
      rect: w.getRect(),
      value: w.getValue(),
      options: w.isChoice() ? w.getOptions() : [],
      readOnly: w.isReadOnly(),
      multiline: w.isText() && w.isMultiline(),
      checked: (w.isCheckbox() || w.isRadioButton()) ? w.getValue() !== 'Off' && w.getValue() !== '' : false,
      signed: w.getFieldType() === 'signature' ? !w.getObject().get('V').isNull() : false,
      maxLen: w.isText() ? w.getMaxLen() : 0,
    }));
  }

  widget(i, n) { return this.page(i).getWidgets()[n]; }

  setField(i, n, value) {
    this.step('fill', () => {
      const w = this.widget(i, n);
      if (w.isText()) w.setTextValue(value);
      else if (w.isChoice()) w.setChoiceValue(value);
      w.update();
    });
  }

  toggleField(i, n) {
    this.step('tick', () => { const w = this.widget(i, n); w.toggle(); w.update(); });
  }

  // the form's dictionary, made if the file has none
  acroForm() {
    const root = this.doc.getTrailer().get('Root');
    let form = root.get('AcroForm');
    if (form.isNull()) {
      form = this.doc.addObject({ Fields: [] });
      root.put('AcroForm', form);
    }
    if (form.get('Fields').isNull()) form.put('Fields', this.doc.newArray());
    if (form.get('DA').isNull()) form.put('DA', '(/Helv 0 Tf 0 g)');
    let dr = form.get('DR');
    if (dr.isNull()) { dr = this.doc.newDictionary(); form.put('DR', dr); }
    let fonts = dr.get('Font');
    if (fonts.isNull()) { fonts = this.doc.newDictionary(); dr.put('Font', fonts); }
    if (fonts.get('Helv').isNull()) fonts.put('Helv', this.doc.addObject({ Type: 'Font', Subtype: 'Type1', BaseFont: 'Helvetica', Encoding: 'WinAnsiEncoding' }));
    return form;
  }

  // page points (top left, y down) to the file's own space
  toPdf(i, rect) {
    const inv = mu.Matrix.invert(this.page(i).getTransform());
    const p = (x, y) => [inv[0] * x + inv[2] * y + inv[4], inv[1] * x + inv[3] * y + inv[5]];
    const [ax, ay] = p(rect[0], rect[1]), [bx, by] = p(rect[2], rect[3]);
    return [Math.min(ax, bx), Math.min(ay, by), Math.max(ax, bx), Math.max(ay, by)];
  }

  uniqueName(base) {
    const taken = new Set();
    for (let p = 0; p < this.count; p++) for (const f of this.fields(p)) taken.add(f.name);
    let n = ++this.fieldSerial, name = `${base} ${n}`;
    while (taken.has(name)) name = `${base} ${++n}`;
    this.fieldSerial = n;
    return name;
  }

  /// a new field on page i: kind is text, multiline, checkbox, date or signature
  addField(i, kind, rect, label) {
    return this.step('add field', () => {
      const name = label || this.uniqueName(kind === 'signature' ? 'Signature' : kind === 'checkbox' ? 'Check box' : 'Field');
      this.newWidget(i, rect, name, kind === 'checkbox'
        ? { FT: 'Btn', V: 'Off', AS: 'Off', DA: '(/ZaDb 0 Tf 0 g)', MK: { BC: [0.4, 0.4, 0.4], CA: '(4)' } }
        : kind === 'signature'
          ? { FT: 'Sig' }
          : { FT: 'Tx', DA: '(/Helv 0 Tf 0 g)', V: '()', ...(kind === 'multiline' ? { Ff: 4096 } : {}), ...(kind === 'date' ? { TU: '(Date)' } : {}) });
      return name;
    });
  }

  // A widget made through MuPDF's own call, so the open page knows it at
  // once (one made from bare objects shows only once the file is opened
  // again), then given its field's keys and put in the form
  newWidget(i, rect, name, keys) {
    const form = this.acroForm();
    const w = this.page(i).createAnnotation('Widget');
    w.setRect(rect);
    const o = w.getObject();
    o.put('T', `(${name})`);
    o.put('F', keys.FT === 'Sig' && keys.V ? 132 : 4);
    if (!keys.MK) o.put('MK', { BC: [0.6, 0.6, 0.6] });
    for (const [k, v] of Object.entries(keys)) o.put(k, v);
    const fields = form.get('Fields');
    let listed = false;
    fields.forEach((f) => { if (f.asIndirect() === o.asIndirect()) listed = true; });
    if (!listed) fields.push(o);
    if (keys.FT === 'Sig') form.put('SigFlags', 3);
    // a reader draws a field with no appearance of its own from this
    else form.put('NeedAppearances', true);
    w.update();
    return w;
  }

  /// The page's text as paragraphs, which is what Edit text changes. A
  /// block MuPDF found is split where a line starts a list item or a gap
  /// opens up; a list item's bullet is kept apart, so it is never set again.
  paragraphs(i) {
    // every line on the page, pieces on one baseline joined back together
    const raw = [];
    for (const blk of this.text(i)) {
      for (const l of blk.lines) {
        if (!l.text.trim() || Math.abs(l.dir?.[1] || 0) > 0.01) continue;
        const f = l.chars.find((c) => c.c.trim());
        raw.push({ chars: l.chars, y: f.origin[1], x0: l.bbox[0], x1: l.bbox[2], size: f.size });
      }
    }
    raw.sort((a, b) => a.y - b.y || a.x0 - b.x0);
    const lines = [];
    for (const l of raw) {
      const prev = lines.findLast((o) => Math.abs(o.y - l.y) < 0.6 && l.x0 >= o.x1 - 0.5 && l.x0 - o.x1 < l.size * 1.6);
      if (prev) {
        const space = l.x0 - prev.x1 > l.size * 0.15 && prev.chars.at(-1).c !== ' ' && l.chars[0].c !== ' ';
        if (space) prev.chars.push({ ...prev.chars.at(-1), c: ' ' });
        prev.chars.push(...l.chars);
        prev.x1 = l.x1;
      } else lines.push({ ...l, chars: [...l.chars] });
    }
    for (const l of lines) {
      lineText(l);
      l.bbox = [l.x0, Math.min(...l.chars.map((c) => Math.min(c.quad[1], c.quad[3]))), l.x1, Math.max(...l.chars.map((c) => Math.max(c.quad[5], c.quad[7])))];
    }

    // lines into paragraphs: one goes on the paragraph just above it when it
    // sits a line below, overlaps it across, is the same size, and does not
    // start a list item. Columns fall out of the overlap.
    const out = [];
    const open = [];
    for (const l of lines) {
      const first = l.chars.findIndex((c) => c.c.trim());
      const ch = l.chars[first];
      let upto = -1;
      if (BULLETS.test(ch.c) || opaque(ch.c)) upto = first + 1;
      else {
        const m = /^\(?[0-9a-zA-Z]{1,3}[.)]\s/.exec(l.text.slice(l.at[first]));
        if (m) upto = charOf(l, l.at[first] + m[0].trimEnd().length - 1) + 1;
      }
      if (upto >= 0) while (l.chars[upto] && !l.chars[upto].c.trim()) upto++;
      const bullet = upto > 0 && upto < l.chars.length;
      const word = l.chars[bullet ? upto : first];
      let into = null;
      if (!bullet) {
        into = open.find((p) => {
          const last = p.lines.at(-1);
          const gap = word.origin[1] - last.y;
          const lead = p.lines.length > 1 ? p.lines.at(-1).y - p.lines.at(-2).y : p.size * 1.25;
          return Math.abs(word.size - p.size) < 0.6 && gap > p.size * 0.6 && gap < Math.max(lead * 1.3, p.size * 1.7)
            && l.x0 < last.x1 && l.x1 > last.x0 - p.size * 2;
        });
      }
      if (into) {
        into.lines.push(l);
      } else {
        into = { lines: [l], size: word.size, bullet: bullet ? { upto } : null };
        out.push(into);
      }
      // a paragraph stays open to the lines below it until one passes it by
      for (let k = open.length - 1; k >= 0; k--) if (word.origin[1] - open[k].lines.at(-1).y > open[k].size * 3) open.splice(k, 1);
      if (!open.includes(into)) open.push(into);
    }
    for (const p of out) {
      for (const l of p.lines) l.y = l.chars.find((c) => c.c.trim()).origin[1];
      p.text = p.lines.map((l, k) => (k === 0 && p.bullet ? l.text.slice(l.at[p.bullet.upto]) : l.text).trim())
        .reduce((s, t) => (!s ? t : s.endsWith('-') && /^[a-z]/.test(t) ? s.slice(0, -1) + t : `${s} ${t}`), '');
      const xs = p.lines.map((l) => l.bbox), first = p.lines[0];
      p.bbox = [Math.min(...xs.map((b) => b[0])), Math.min(...xs.map((b) => b[1])), Math.max(...xs.map((b) => b[2])), Math.max(...xs.map((b) => b[3]))];
      p.unreadable = [...p.text].some(opaque);
      p.first = p.bullet ? first.chars[p.bullet.upto] : first.chars.find((c) => c.c.trim());
    }
    return out;
  }

  /// Set paragraph `pi` of page i again as `text`, in our face nearest its
  /// own font, at its size, colour and leading. Each line keeps the indent
  /// and width it had; justified text stays justified. Lines before the
  /// first change are left as they were. `overflow` when the new words run
  /// into what is below.
  // which of the page's fonts are monospaced, judged by their letters'
  // widths where the font's name and flags say nothing
  monoFonts(i) {
    const seen = new Map();
    for (const b of this.text(i)) for (const l of b.lines) for (const c of l.chars) {
      if (!/[a-zA-Z]/.test(c.c)) continue;
      const k = c.font.getName();
      const w = (c.quad[2] - c.quad[0]) / c.size;
      const o = seen.get(k) || { narrow: [], wide: [] };
      if (/[ijlt]/.test(c.c)) o.narrow.push(w);
      if (/[mwMW]/.test(c.c)) o.wide.push(w);
      seen.set(k, o);
    }
    const mono = new Set();
    const avg = (a) => a.reduce((x, y) => x + y, 0) / a.length;
    for (const [k, o] of seen) if (o.narrow.length && o.wide.length && avg(o.narrow) / avg(o.wide) > 0.85) mono.add(k);
    return mono;
  }

  async editParagraph(i, pi, text, { fit = false } = {}) {
    const paras = this.paragraphs(i);
    const p = paras[pi];
    if (!p) throw new Error('that text is gone');
    text = text.replace(/\s+/g, ' ').trim();
    if (text === p.text) return { same: true };
    const lines = p.lines;

    // the old words, each with the style of its first letter
    const oldWords = [];
    lines.forEach((l, k) => {
      const chars = k === 0 && p.bullet ? l.chars.slice(p.bullet.upto) : l.chars;
      let cur = null;
      for (const c of chars) {
        if (!c.c.trim()) { cur = null; continue; }
        if (!cur) { cur = { text: '', c, line: k }; oldWords.push(cur); }
        cur.text += c.c;
      }
    });
    // a word broken across lines with a hyphen is one word
    for (let k = oldWords.length - 2; k >= 0; k--) {
      const a = oldWords[k], b = oldWords[k + 1];
      if (a.line !== b.line && a.text.endsWith('-') && /^[a-z]/.test(b.text)) { a.text = a.text.slice(0, -1) + b.text; oldWords.splice(k + 1, 1); }
    }
    const mono = this.monoFonts(i);
    const styleOf = (c) => {
      const n = c.font.getName();
      return { family: mono.has(n) ? 'Mono' : familyOf(n, c.font), bold: isBold(n, c.font), italic: isItalic(n, c.font), color: (c.color || [0, 0, 0]).concat([0, 0, 0]).slice(0, 3), size: c.size };
    };
    const tally = new Map();
    for (const w of oldWords) tally.set(w.c.font.getName(), (tally.get(w.c.font.getName()) || 0) + w.text.length);
    const mainName = [...tally.entries()].sort((a, b) => b[1] - a[1])[0]?.[0];
    const main = styleOf((oldWords.find((w) => w.c.font.getName() === mainName) || { c: p.first }).c);
    const size = main.size;

    // the new words matched to the old (longest common run), so a word that
    // stayed keeps its own face and colour; a new word takes the one before it
    const nw = plainText(text).split(' ').filter(Boolean);
    const ow = oldWords.map((w) => plainText(w.text));
    const L = Array.from({ length: nw.length + 1 }, () => new Int32Array(ow.length + 1));
    for (let a = nw.length - 1; a >= 0; a--) for (let b = ow.length - 1; b >= 0; b--) L[a][b] = nw[a] === ow[b] ? L[a + 1][b + 1] + 1 : Math.max(L[a + 1][b], L[a][b + 1]);
    const words = [];
    // a new word stands in for an old one dropped at the same place, and
    // takes its style; past those, the paragraph's own
    let dropped = [];
    for (let a = 0, b = 0; a < nw.length;) {
      if (b < ow.length && nw[a] === ow[b]) { words.push({ text: nw[a], style: styleOf(oldWords[b].c), from: b }); a++; b++; dropped = []; }
      else if (b < ow.length && L[a][b + 1] >= L[a + 1][b]) { dropped.push(b); b++; }
      else {
        const stand = dropped.length ? dropped.shift() : -1;
        words.push({ text: nw[a], style: stand >= 0 ? styleOf(oldWords[stand].c) : main, from: -1 });
        a++;
      }
    }

    // the faces this needs, loaded before the step that uses them
    const faces = new Map();
    const keyOf = (st) => `${st.family}${st.bold ? 'B' : ''}${st.italic ? 'I' : ''}`;
    for (const st of [...words.map((w) => w.style), ...oldWords.map((w) => styleOf(w.c))]) {
      const k = keyOf(st);
      if (!faces.has(k)) faces.set(k, await face(st.family, st.bold, st.italic));
    }
    for (const w of words) { w.faceKey = keyOf(w.style); w.face = faces.get(w.faceKey); }
    const glyph = (f, c) => { const g = f.encodeCharacter(c); return g > 0 ? g : f.encodeCharacter('?'); };
    // Our face is a near twin, not the font itself: each font's words are
    // narrowed or widened (Tz) to the width the original letters took
    const ratios = new Map();
    for (const w of oldWords) {
      const k = w.c.font.getName();
      const f = faces.get(keyOf(styleOf(w.c)));
      const line = lines[w.line];
      const at0 = line.chars.indexOf(w.c);
      const run = line.chars.slice(at0, at0 + w.text.length);
      if (!f || run.length < 2 || !run.every((c) => c.c.trim())) continue;
      const theirs = run.at(-1).origin[0] - run[0].origin[0];
      const ours = run.slice(0, -1).reduce((n, c) => n + f.advanceGlyph(glyph(f, c.c)) * c.size, 0);
      if (ours > 0 && theirs > 0) (ratios.get(k) || ratios.set(k, []).get(k)).push(theirs / ours);
    }
    const hscaleOf = new Map([...ratios].map(([k, r]) => [k, Math.min(1.12, Math.max(0.88, r.sort((a, b) => a - b)[r.length >> 1]))]));
    for (const w of words) {
      const src = w.from >= 0 ? oldWords[w.from].c : null;
      w.h = (src && hscaleOf.get(src.font.getName())) || hscaleOf.get(mainName) || 1;
    }
    let scale = 1;
    const adv = (w, s = w.text) => [...s].reduce((n, c) => n + w.face.advanceGlyph(glyph(w.face, c)) * w.style.size * scale * w.h, 0);
    const measure = () => { for (const w of words) { w.width = adv(w); w.space = adv(w, ' '); } };
    measure();

    // each line's place: where its words start, where it ends, its baseline
    const geo = lines.map((l, k) => {
      const from = k === 0 && p.bullet ? l.chars[p.bullet.upto] : l.chars.find((c) => c.c.trim());
      return { x: from.origin[0], right: l.bbox[2], y: from.origin[1] };
    });
    const leading = lines.length > 1 ? (geo.at(-1).y - geo[0].y) / (lines.length - 1) : size * 1.2;
    // justified when the full lines end together and their spaces are wider
    // than the last line's, which is set at its natural width
    const rights = geo.slice(0, -1).map((g) => g.right);
    const spaces = (l) => {
      const w = [];
      l.chars.forEach((c, k) => { if (c.c === ' ' && l.chars[k + 1] && l.chars[k - 1]) w.push(l.chars[k + 1].origin[0] - c.origin[0]); });
      return w.length ? w.reduce((a, b) => a + b, 0) / w.length : 0;
    };
    const lastGap = spaces(lines.at(-1));
    const fullGap = lines.length > 1 ? Math.max(...lines.slice(0, -1).map(spaces)) : 0;
    const justified = lines.length >= 2 && Math.max(...rights) - Math.min(...rights) < 1.5
      && (lines.length >= 3 || (lastGap > 0 && fullGap > lastGap * 1.12));
    const right = Math.max(...geo.map((g) => g.right));
    const [pw, ph] = this.size(i);
    // a paragraph that was one line may grow to the page's right margin
    // (the right edge of the page's text, where the other paragraphs end)
    const textRight = Math.max(...paras.filter((o) => o.bbox[0] < geo[0].x + 4 * size).map((o) => o.bbox[2]));
    const edge = lines.length > 1 ? right : Math.max(right, Math.min(textRight, pw - 36));
    const at = (k) => {
      const x = k < geo.length ? geo[k].x : (geo[1] || geo[0]).x;
      return { x, width: edge - x, y: geo[0].y + k * leading };
    };

    // lines before the first change stay as they were
    let keep = 0, used = 0;
    for (; keep < lines.length - 1; keep++) {
      const inLine = oldWords.filter((w) => w.line === keep);
      const ok = inLine.length && inLine.every((w, k) => words[used + k]?.from === oldWords.indexOf(w)) && !lines[keep].text.trim().endsWith('-');
      if (!ok) break;
      used += inLine.length;
    }

    const wrap = () => {
      const rows = [];
      let row = [], wide = 0;
      for (const w of words.slice(used)) {
        const need = row.length ? wide + row.at(-1).space + w.width : w.width;
        if (row.length && need > at(keep + rows.length).width) { rows.push(row); row = [w]; wide = w.width; } else { row.push(w); wide = need; }
      }
      if (row.length) rows.push(row);
      return rows;
    };
    const below = paras.filter((o) => o !== p && o.bbox[1] > p.bbox[3] - 1 && o.bbox[0] < p.bbox[2] && o.bbox[2] > p.bbox[0]).map((o) => o.bbox[1]);
    const limit = below.length ? Math.min(...below) : ph;
    const runsOver = (rows) => at(keep + rows.length - 1).y + size * 0.25 > limit;
    let rows = wrap();
    // shrink to fit: down to four fifths of the size, a twentieth at a time
    while (fit && runsOver(rows) && scale > 0.81) { scale -= 0.05; measure(); rows = wrap(); }
    const overflow = runsOver(rows);

    this.step('edit text', () => {
      const page = this.page(i);
      const r = page.createAnnotation('Redact');
      r.setQuadPoints(lines.slice(keep).flatMap((l, k) => (k + keep === 0 && p.bullet ? l.chars.slice(p.bullet.upto) : l.chars).filter((c) => c.c.trim()).map((c) => c.quad)));
      page.applyRedactions(false, mu.PDFPage.REDACT_IMAGE_NONE, mu.PDFPage.REDACT_LINE_ART_NONE, mu.PDFPage.REDACT_TEXT_REMOVE);
      if (!rows.length) return;
      const keys = new Map();
      for (const w of words) if (!keys.has(w.faceKey)) keys.set(w.faceKey, this.fontKey(page, w.face));
      const tx = this.unmap(page);
      const hex = (w, s) => [...s].map((c) => glyph(w.face, c).toString(16).padStart(4, '0')).join('');
      let ops = 'q BT\n';
      let font = '', color = '';
      rows.forEach((ws, k) => {
        const g = at(keep + k);
        const natural = ws.reduce((n, w, j) => n + w.width + (j ? ws[j - 1].space : 0), 0);
        const spare = g.width - natural;
        // justified: the spare room shared between the words, but on the last line
        const extra = justified && ws.length > 1 && k < rows.length - 1 && spare > 0 && spare < g.width * 0.25 ? spare / (ws.length - 1) : 0;
        let x = g.x;
        ws.forEach((w, j) => {
          const f = `/${keys.get(w.faceKey)} ${(w.style.size * scale).toFixed(2)} Tf ${(w.h * 100).toFixed(1)} Tz`;
          if (f !== font) { ops += `${f}\n`; font = f; }
          const c = `${w.style.color.map((v) => v.toFixed(3)).join(' ')} rg`;
          if (c !== color) { ops += `${c}\n`; color = c; }
          const [px, py] = tx(x, g.y);
          // the space after each word is drawn too, so text copied out of
          // the page has its spaces
          ops += `1 0 0 1 ${px.toFixed(2)} ${py.toFixed(2)} Tm <${hex(w, j < ws.length - 1 ? `${w.text} ` : w.text)}> Tj\n`;
          x += w.width + w.space + extra;
        });
      });
      this.append(page, `${ops}ET Q\n`);
    });
    const kept = words.filter((w) => w.from >= 0).length;
    return { font: `Liberation ${main.family}`, from: (mainName || '').replace(/^[A-Z]{6}\+/, ''), lines: rows.length + keep, overflow, justified, kept, added: words.length - kept, scale };
  }

  // a font of ours in the page's resources, under a name not taken
  fontKey(page, font, stem = 'Ed') {
    const pobj = page.getObject();
    let res = pobj.get('Resources');
    if (res.isNull()) { res = this.doc.newDictionary(); pobj.put('Resources', res); }
    let fonts = res.get('Font');
    if (fonts.isNull()) { fonts = this.doc.newDictionary(); res.put('Font', fonts); }
    let key = `${stem}1`;
    for (let k = 1; !fonts.get(key).isNull(); k++) key = `${stem}${k}`;
    fonts.put(key, this.doc.addFont(font));
    return key;
  }

  // page points (top left, y down) to the file's own space, for drawing
  unmap(page) {
    const inv = mu.Matrix.invert(page.getTransform());
    return (x, y) => [inv[0] * x + inv[2] * y + inv[4], inv[1] * x + inv[3] * y + inv[5]];
  }

  // drawing added after what the page had
  append(page, ops) {
    const pobj = page.getObject();
    // what was there is wrapped in q/Q, so a state it left set cannot leak into ours
    const add = this.doc.addStream(ops, {});
    const c = pobj.get('Contents');
    const arr = this.doc.newArray();
    const said = (o) => { try { return o.readStream().asString().trim(); } catch { return ''; } };
    if (c.isArray()) {
      // wrapped by an earlier edit already: q first, Q before our first addition
      const wrapped = c.length > 1 && said(c.get(0)) === 'q';
      if (!wrapped) arr.push(this.doc.addStream('q\n', {}));
      c.forEach((v) => { arr.push(v); });
      if (!wrapped) arr.push(this.doc.addStream('Q\n', {}));
    } else if (!c.isNull()) {
      arr.push(this.doc.addStream('q\n', {}));
      arr.push(c);
      arr.push(this.doc.addStream('Q\n', {}));
    }
    arr.push(add);
    pobj.put('Contents', arr);
  }

  /// words where there were none: a text box in our face, set into the page
  /// itself rather than as a mark (Add text with "part of the page")
  async addPageText(i, at, text, { size = 12, color = [0, 0, 0], family = 'Sans', bold = false, italic = false } = {}) {
    const font = await face(family, bold, italic);
    this.step('add text', () => {
      const page = this.page(i);
      const key = this.fontKey(page, font);
      const tx = this.unmap(page);
      const glyph = (c) => { const g = font.encodeCharacter(c); return g > 0 ? g : font.encodeCharacter('?'); };
      let ops = `q BT /${key} ${size} Tf ${color.join(' ')} rg\n`;
      text.split('\n').forEach((s, k) => {
        const [x, y] = tx(at[0], at[1] + size + k * size * 1.2);
        ops += `1 0 0 1 ${x.toFixed(2)} ${y.toFixed(2)} Tm <${[...plainText(s)].map((c) => glyph(c).toString(16).padStart(4, '0')).join('')}> Tj\n`;
      });
      this.append(page, `${ops}ET Q\n`);
    });
  }

  /// Text a scan was read into, laid over it unseen (render mode 3), so the
  /// page can be searched, selected and copied. words: [{ text, bbox }] in
  /// page points.
  async addHiddenText(i, words) {
    const font = await face('Sans', false, false);
    this.step('recognise text', () => {
      const page = this.page(i);
      const key = this.fontKey(page, font, 'Ocr');
      const tx = this.unmap(page);
      const glyph = (c) => { const g = font.encodeCharacter(c); return g > 0 ? g : font.encodeCharacter('?'); };
      let ops = 'q BT 3 Tr\n';
      for (const w of words) {
        const t = plainText(w.text).trim();
        if (!t) continue;
        const h = w.bbox[3] - w.bbox[1];
        const size = Math.max(1, h * 0.9);
        const natural = [...t].reduce((s, c) => s + font.advanceGlyph(glyph(c)) * size, 0);
        const scale = natural > 0 ? (100 * (w.bbox[2] - w.bbox[0])) / natural : 100;
        const [x, y] = tx(w.bbox[0], w.bbox[3] - h * 0.2);
        ops += `/${key} ${size.toFixed(2)} Tf ${scale.toFixed(1)} Tz 1 0 0 1 ${x.toFixed(2)} ${y.toFixed(2)} Tm <${[...t].map((c) => glyph(c).toString(16).padStart(4, '0')).join('')}> Tj\n`;
      }
      this.append(page, `${ops}ET Q\n`);
    });
  }

  /// whether page i has text to select, or is only a picture (a scan)
  hasText(i) { return this.text(i).some((b) => b.lines.some((l) => l.text.trim())); }

  rotate(i, by) {
    this.step('rotate', () => {
      const p = this.doc.findPage(i);
      const now = p.getInheritable('Rotate');
      p.put('Rotate', (((now.isNull() ? 0 : now.asNumber()) + by) % 360 + 360) % 360);
    });
  }

  deletePages(list) {
    this.step('delete pages', () => { for (const i of [...list].sort((a, b) => b - a)) this.doc.deletePage(i); });
  }

  /// the pages in a new order: order[k] is the page that goes at k
  reorder(order) { this.step('reorder', () => this.doc.rearrangePages(order)); }

  insertBlank(at, size) {
    this.step('insert page', () => {
      const [w, h] = size || this.size(Math.max(0, Math.min(at, this.count - 1)));
      const obj = this.doc.addPage([0, 0, w, h], 0, {}, '');
      this.doc.insertPage(at, obj);
    });
  }

  duplicate(i) {
    this.step('duplicate page', () => {
      const map = this.doc.newGraftMap();
      map.graftPage(i + 1, this.doc, i);
    });
  }

  /// pages of another PDF (bytes), all or `pages`, at `at`
  async insertFrom(at, bytes, pages) {
    const other = await Pdf.open(bytes);
    const list = pages || [...Array(other.count).keys()];
    this.step('insert pages', () => {
      const map = this.doc.newGraftMap();
      list.forEach((p, k) => { map.graftPage(at + k, other.doc, p); });
    });
    return list.length;
  }

  /// a new PDF of some of these pages
  extract(pages) {
    const out = new mu.PDFDocument();
    const map = out.newGraftMap();
    pages.forEach((p, k) => { map.graftPage(k, this.doc, p); });
    return out.saveToBuffer('garbage,compress').asUint8Array().slice();
  }

  /// the signature fields: [{ page, n, name, rect, signed, range, contents }]
  signatures() {
    const out = [];
    for (let i = 0; i < this.count; i++) {
      this.page(i).getWidgets().forEach((w, n) => {
        if (w.getFieldType() !== 'signature') return;
        const v = w.getObject().get('V');
        const o = { page: i, n, name: w.getName(), rect: w.getRect(), signed: false };
        if (!v.isNull()) {
          const br = v.get('ByteRange');
          const ct = v.get('Contents');
          if (br.isArray() && ct.isString()) {
            o.signed = true;
            o.range = [0, 1, 2, 3].map((k) => br.get(k).asNumber());
            o.contents = ct.asByteString();
            const m = v.get('M');
            o.date = m.isString() ? m.asString() : '';
            const nm = v.get('Name');
            o.signer = nm.isString() ? nm.asString() : '';
            const why = v.get('Reason');
            o.reason = why.isString() ? why.asString() : '';
          }
        }
        out.push(o);
      });
    }
    return out;
  }

  hasSignatures() { return this.signatures().some((s) => s.signed); }

  /// Make room for a signature: field `into` ({ page, n }) if the file has
  /// an empty one there, else a new field over `rect` on page i. The value
  /// has a /ByteRange of marks and a /Contents of zeros, filled in by
  /// pdf-sign.js once the file is written.
  /// Returns the tag `seal` finds the hole by.
  prepareSignature(i, rect, { name, reason, location, into } = {}) {
    const tag = nonce();
    this.step('sign', () => {
      const form = this.acroForm();
      form.put('SigFlags', 3);
      const now = new Date();
      const p = (n) => String(n).padStart(2, '0');
      const m = `D:${now.getUTCFullYear()}${p(now.getUTCMonth() + 1)}${p(now.getUTCDate())}${p(now.getUTCHours())}${p(now.getUTCMinutes())}${p(now.getUTCSeconds())}Z`;
      const v = this.doc.addObject({
        Type: 'Sig', Filter: 'Adobe.PPKLite', SubFilter: 'ETSI.CAdES.detached',
        ByteRange: [0, RANGE_MARK, RANGE_MARK, RANGE_MARK],
        Contents: this.doc.newByteString(new Uint8Array(HOLE)),
        M: `(${m})`,
        ...(name ? { Name: `(${name})` } : {}),
        ...(reason ? { Reason: `(${reason})` } : {}),
        ...(location ? { Location: `(${location})` } : {}),
        Prop_Build: { App: { Name: 'Commonty', REx: `(${tag})` } },
      });
      if (into) {
        this.widget(into.page, into.n).getObject().put('V', v);
        return;
      }
      this.newWidget(i, rect, this.uniqueName('Signature'), { FT: 'Sig', V: v });
    });
    return tag;
  }

  /// carry out the marked redactions, for good: what is under them is gone
  applyRedactions() {
    const marked = [];
    for (let i = 0; i < this.count; i++) if (this.page(i).getAnnotations().some((a) => a.getType() === 'Redact')) marked.push(i);
    if (!marked.length) return;
    const meets = (a, b) => a[0] < b[2] && a[2] > b[0] && a[1] < b[3] && a[3] > b[1];
    this.step('redact', () => {
      for (const i of marked) {
        const page = this.page(i);
        const areas = page.getAnnotations().filter((a) => a.getType() === 'Redact').map((a) => { try { return a.getRect(); } catch { return a.getBounds(); } });
        // what sits on the page over a redaction goes too: marks and
        // comments are deleted, fields there are emptied
        for (const a of page.getAnnotations()) {
          if (a.getType() === 'Redact') continue;
          let r;
          try { r = a.getRect(); } catch { r = a.getBounds(); }
          if (areas.some((x) => meets(x, r))) page.deleteAnnotation(a);
        }
        for (const w of page.getWidgets()) {
          if (!areas.some((x) => meets(x, w.getRect()))) continue;
          if (w.isText()) w.setTextValue('');
          else if ((w.isCheckbox() || w.isRadioButton()) && w.getValue() !== 'Off') w.toggle();
          else if (w.isChoice()) w.setChoiceValue('');
          w.update();
        }
        page.applyRedactions(true, mu.PDFPage.REDACT_IMAGE_PIXELS, mu.PDFPage.REDACT_LINE_ART_REMOVE_IF_COVERED, mu.PDFPage.REDACT_TEXT_REMOVE);
      }
      // the file's own title, author, subject and keywords can repeat what
      // was redacted; a redacted file keeps none of them
      this.doc.getTrailer().delete('Info');
      const root = this.doc.getTrailer().get('Root');
      root.delete('Metadata');
      for (const i of marked) this.doc.findPage(i).delete('Metadata');
      // nor the words a tagged PDF keeps for screen readers (/ActualText,
      // /Alt, /E), which can say what a redacted picture or word was
      const seen = new Set();
      const strip = (o, depth) => {
        if (depth > 200 || !o || o.isNull()) return;
        if (o.isIndirect()) { if (seen.has(o.asIndirect())) return; seen.add(o.asIndirect()); }
        const d = o.resolve();
        if (d.isArray()) { d.forEach((v) => { strip(v, depth + 1); }); return; }
        if (!d.isDictionary()) return;
        for (const k of ['ActualText', 'Alt', 'E']) d.delete(k);
        const kids = d.get('K');
        if (!kids.isNull()) strip(kids, depth + 1);
      };
      strip(root.get('StructTreeRoot'), 0);
    });
  }

  /// how many redactions are marked and not yet carried out
  marked() {
    let n = 0;
    for (let i = 0; i < this.count; i++) n += this.page(i).getAnnotations().filter((a) => a.getType() === 'Redact').length;
    return n;
  }

  /// The file as it now is. Marked redactions are carried out first, for
  /// good. flatten: marks and fields burnt into the pages. smaller: unused
  /// objects dropped, streams compressed, fonts subset. A file that holds
  /// signatures is saved as an update after its old bytes, so they still
  /// check, unless flattened (which breaks them anyway).
  save({ flatten = false, smaller = false, keepSignatures = true } = {}) {
    // a redaction is never saved as an update: the earlier revision would
    // still hold what was redacted. The whole file is written again, which
    // breaks any signature it had (the editor says so before)
    const redacting = this.marked() > 0;
    this.applyRedactions();
    if (flatten) this.step('flatten', () => this.doc.bake(true, true));
    if (smaller) this.step('smaller', () => this.doc.subsetFonts());
    const signed = !redacting && !flatten && keepSignatures && this.hasSignatures() && this.doc.canBeSavedIncrementally();
    const opts = signed ? 'incremental' : smaller ? 'garbage=compact,compress,compress-fonts,compress-images,clean' : 'garbage,compress';
    return this.doc.saveToBuffer(opts).asUint8Array().slice();
  }

  /// written plainly (no object streams) so the signature's hole is in the
  /// clear, as an update when it already has signatures
  saveForSigning() {
    const redacting = this.marked() > 0;
    const prior = !redacting && this.signatures().some((s) => s.signed && s.range?.[1] !== RANGE_MARK);
    this.applyRedactions();
    return this.doc.saveToBuffer(prior && this.doc.canBeSavedIncrementally() ? 'incremental' : 'garbage').asUint8Array().slice();
  }

  destroy() { this.doc.destroy?.(); }
}
