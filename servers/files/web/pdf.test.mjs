// pdf-doc.js and pdf-sign.js under node, on the same MuPDF the pages pin
// (nix/tests/pdf-editor.nix). The PDFs are made here, with MuPDF, so the
// test carries no files. Run: node pdf.test.mjs <web dir> <fonts dir>
// (both the built `packages.web`, which holds mupdf.js and the faces)
import fs from 'node:fs';
import { execFileSync } from 'node:child_process';

const [, , MUPDF, FONTS] = process.argv;
const mupdf = await import(`${MUPDF}/mupdf.js`);
const { Pdf, useEngine } = await import('./pdf-doc.js');
const sign = await import('./pdf-sign.js');
useEngine(mupdf, async (f) => new Uint8Array(fs.readFileSync(`${FONTS}/${f}`)));
const TMP = fs.mkdtempSync('pdf-test-');

const failed = [];
async function t(name, fn) {
  try {
    await fn();
    console.log(`PASS ${name}`);
  } catch (e) {
    console.log(`FAIL ${name}: ${e.message}`);
    failed.push(name);
  }
}
const ok = (c, m) => { if (!c) throw new Error(m); };
const text = (bytes, i = 0) => mupdf.Document.openDocument(bytes, 'application/pdf').loadPage(i).toStructuredText().asText().replace(/\s+/g, ' ');
// every stream written out uncompressed, so a search of the bytes sees all
const raw = (bytes) => Buffer.from(mupdf.Document.openDocument(bytes, 'application/pdf').asPDF().saveToBuffer('decompress').asUint8Array().slice()).toString('latin1');
const pdfsig = (bytes) => {
  const f = `${TMP}/s-${Date.now()}.pdf`;
  fs.writeFileSync(f, bytes);
  try { return execFileSync('pdfsig', [f]).toString(); } catch (e) { return String(e.stdout); }
};

// a lease, as a word processor writes one: a serif face, a mono word, a
// two-line paragraph, a justified one of three lines, a list item
function lease() {
  const doc = new mupdf.PDFDocument();
  const serif = new mupdf.Font('LiberationSerif-Regular', fs.readFileSync(`${FONTS}/LiberationSerif-Regular.ttf`));
  const mono = new mupdf.Font('LiberationMono-Regular', fs.readFileSync(`${FONTS}/LiberationMono-Regular.ttf`));
  const res = { Font: { F1: doc.addFont(serif), F2: doc.addFont(mono) } };
  const hex = (f, s) => [...s].map((c) => f.encodeCharacter(c).toString(16).padStart(4, '0')).join('');
  const w = (f, s, size) => [...s].reduce((n, c) => n + f.advanceGlyph(f.encodeCharacter(c)) * size, 0);
  let ops = 'BT\n';
  const at = (x, y) => `1 0 0 1 ${x} ${842 - y} Tm `;
  const line = (x, y, s, f = serif, key = 'F1', size = 12) => { ops += `/${key} ${size} Tf ${at(x, y)}<${hex(f, s)}> Tj\n`; };
  // a full line set word by word to the margin, as justified text is
  const justify = (y, s) => {
    const words = s.split(' ');
    const room = 495 - 72 - w(serif, words.join(''), 12);
    const gap = room / (words.length - 1);
    let x = 72;
    for (const word of words) { line(x.toFixed(2), y, word); x += w(serif, word, 12) + gap; }
  };
  line(72, 80, 'RESIDENTIAL LEASE AGREEMENT', serif, 'F1', 16);
  line(72, 110, 'This Lease is made between Harbor Street Properties LLC and the');
  line(72, 124, 'Tenant for 48 Harbor Street, Unit 3, Somerville.');
  line(72, 154, 'The deposit is held at Eastern Bank, account no. 4471-0093-2218.');
  justify(184, 'Tenant shall keep the premises clean and report needed repairs');
  justify(198, 'promptly in writing, and Landlord shall maintain the structure,');
  line(72, 212, 'heating and plumbing in good repair.');
  ops += `/F1 12 Tf ${at(72, 242)}<${hex(serif, 'Run ')}> Tj /F2 12 Tf <${hex(mono, 'fixnames()')}> Tj /F1 12 Tf <${hex(serif, ' before the move.')}> Tj\n`;
  line(80, 272, '•');
  line(92, 272, 'One cat is permitted. No other animals without consent.');
  ops += 'ET\n';
  const page = doc.addPage([0, 0, 595, 842], 0, res, ops);
  doc.insertPage(-1, page);
  return doc.saveToBuffer('compress').asUint8Array().slice();
}

const LEASE = lease();

await t('marks are made, moved, undone and redone', async () => {
  const p = await Pdf.open(LEASE);
  await p.add(0, { type: 'Highlight', quads: p.search(0, 'Harbor Street Properties').flat(), color: [1, 0.85, 0] });
  await p.add(0, { type: 'Ink', paths: [[[80, 400], [120, 380], [160, 410]]], color: [0, 0, 0.5], width: 2 });
  await p.add(0, { type: 'FreeText', rect: [300, 380, 520, 410], text: 'Checked', size: 11 });
  await p.add(0, { type: 'Square', rect: [80, 450, 200, 500], width: 2 });
  await p.add(0, { type: 'Line', a: [340, 470], b: [460, 470], arrow: true });
  await p.add(0, { type: 'Text', at: [60, 60], text: 'Ask about parking' });
  ok(p.annots(0).length === 6, `six marks, not ${p.annots(0).length}`);
  p.moveAnnot(0, 3, 10, 10);
  p.undo();
  p.undo();
  ok(p.annots(0).length === 5, 'undo took the comment back');
  p.redo();
  const q = await Pdf.open(p.save());
  ok(q.annots(0).length === 6, 'all six saved');
});

await t('a paragraph is changed in place: wrapped, its first line kept, the old words gone', async () => {
  const p = await Pdf.open(LEASE);
  const paras = p.paragraphs(0);
  const k = paras.findIndex((x) => x.text.includes('Harbor Street Properties'));
  ok(k >= 0 && paras[k].lines.length === 2, 'the two-line paragraph is one paragraph');
  const r = await p.editParagraph(0, k, paras[k].text.replace('Somerville.', 'Somerville, Massachusetts, in the county of Middlesex.'));
  ok(r.font === 'Liberation Serif' && !r.overflow, JSON.stringify(r));
  const after = text(p.save());
  ok(after.includes('county of Middlesex') && after.includes('This Lease is made'), 'new words in, first line kept');
});

await t('a justified paragraph stays justified; a mono word keeps its face', async () => {
  const p = await Pdf.open(LEASE);
  const paras = p.paragraphs(0);
  const j = paras.findIndex((x) => x.text.startsWith('Tenant shall'));
  ok(j >= 0 && paras[j].lines.length === 3, `the justified paragraph has three lines (${paras[j]?.lines.length})`);
  const r = await p.editParagraph(0, j, paras[j].text.replace('promptly', 'promptly and fully'));
  ok(r.justified, 'judged justified');
  const m = p.paragraphs(0).findIndex((x) => x.text.includes('fixnames()'));
  await p.editParagraph(0, m, p.paragraphs(0)[m].text.replace('before the move', 'before moving in'));
  const fonts = new Set();
  p.page(0).toStructuredText().walk({ onChar(c, o, font) { if ('fixnames()'.includes(c)) fonts.add(font.getName()); } });
  ok([...fonts].some((n) => /Mono/.test(n)), `fixnames() is still monospaced (${[...fonts]})`);
});

await t('a list item keeps its bullet', async () => {
  const p = await Pdf.open(LEASE);
  const paras = p.paragraphs(0);
  const b = paras.findIndex((x) => x.text.startsWith('One cat'));
  ok(b >= 0 && paras[b].bullet, 'found as a list item');
  await p.editParagraph(0, b, 'Two cats are permitted. No other animals without consent.');
  const after = text(p.save());
  ok(after.includes('•') && after.includes('Two cats'), 'bullet kept, words changed');
});

await t('redaction is gone from the bytes, with marks, field values and metadata over it', async () => {
  const p = await Pdf.open(LEASE);
  const hits = p.match(0, /\b\d{4}-\d{4}-\d{4}\b/g);
  ok(hits.length === 1 && hits[0].text === '4471-0093-2218', 'the account number is found');
  const name = p.addField(0, 'text', [300, 290, 500, 310]);
  const f = p.fields(0).find((x) => x.name === name);
  p.setField(0, f.n, 'SECRET-FIELD-77');
  await p.add(0, { type: 'FreeText', rect: [300, 290, 500, 310], text: 'SECRET-NOTE-88', size: 10 });
  p.step('info', () => {
    p.doc.getTrailer().put('Info', p.doc.addObject({ Title: '(SECRET-TITLE-99)' }));
    const tree = p.doc.addObject({ Type: 'StructTreeRoot', K: p.doc.addObject({ S: 'Figure', Alt: '(SECRET-ALT-66)', K: [p.doc.addObject({ S: 'Span', ActualText: '(SECRET-ACTUAL-55)' })] }) });
    p.doc.getTrailer().get('Root').put('StructTreeRoot', tree);
  });
  for (const h of hits) await p.add(0, { type: 'Redact', quads: h.quads });
  await p.add(0, { type: 'Redact', rect: [296, 286, 504, 314] });
  const out = raw(p.save());
  for (const s of ['4471', 'SECRET-FIELD-77', 'SECRET-NOTE-88', 'SECRET-TITLE-99', 'SECRET-ALT-66', 'SECRET-ACTUAL-55']) ok(!out.includes(s), `${s} survived`);
});

await t('forms: fields added, filled, kept', async () => {
  const p = await Pdf.open(LEASE);
  const a = p.addField(0, 'text', [100, 700, 300, 720]);
  const b = p.addField(0, 'checkbox', [320, 700, 335, 715]);
  p.addField(0, 'signature', [350, 690, 500, 730]);
  const fa = p.fields(0).find((x) => x.name === a);
  p.setField(0, fa.n, 'David Bascom');
  p.toggleField(0, p.fields(0).find((x) => x.name === b).n);
  const q = await Pdf.open(p.save());
  const fs2 = q.fields(0);
  ok(fs2.length === 3, `three fields, not ${fs2.length}`);
  ok(fs2.find((x) => x.name === a).value === 'David Bascom', 'text kept');
  ok(fs2.find((x) => x.name === b).checked, 'tick kept');
  ok(q.signatures().length === 1 && !q.signatures()[0].signed, 'an empty signature field');
});

await t('pages: rotate, blank, duplicate, insert, reorder, delete, extract', async () => {
  const p = await Pdf.open(LEASE);
  p.rotate(0, 90);
  p.insertBlank(1);
  p.duplicate(0);
  const n = await p.insertFrom(p.count, LEASE);
  p.reorder([3, 0, 1, 2]);
  p.deletePages([1]);
  const q = await Pdf.open(p.save());
  ok(n === 1 && q.count === 3, `three pages, not ${q.count}`);
  ok((await Pdf.open(p.extract([0, 1]))).count === 2, 'two pages extracted');
});

let id;
let signed;
await t('a signature pdfsig checks: valid and the whole file', async () => {
  id = sign.loadIdentity(await sign.makeIdentity('David Bascom'));
  let refused = false;
  try { await crypto.subtle.exportKey('pkcs8', id.key); } catch { refused = true; }
  ok(refused, 'the key could be exported');
  const p = await Pdf.open(LEASE);
  const tag = p.prepareSignature(0, [300, 330, 450, 375], { name: 'David Bascom', reason: 'Tenant' });
  signed = await sign.seal(p.saveForSigning(), id, tag);
  const v = pdfsig(signed);
  ok(/Signature is Valid/.test(v) && /Total document signed/.test(v) && /ETSI\.CAdES\.detached/.test(v), v);
  const s = (await Pdf.open(signed)).signatures().find((x) => x.signed);
  const c = await sign.check(signed, s.range, s.contents);
  ok(c.ok && c.whole && c.who === 'David Bascom' && c.selfSigned && c.inDate, JSON.stringify(c));
});

await t('a second signer fills a field another left; both still check', async () => {
  const p = await Pdf.open(signed);
  p.addField(0, 'signature', [80, 600, 230, 640]);
  const q = await Pdf.open(p.save());
  const empty = q.signatures().find((s) => !s.signed);
  const tag = q.prepareSignature(0, null, { name: 'Landlord', into: { page: empty.page, n: empty.n } });
  const two = await sign.seal(q.saveForSigning(), id, tag);
  ok((pdfsig(two).match(/Signature is Valid/g) || []).length === 2, 'pdfsig: both valid');
  const checks = await Promise.all((await Pdf.open(two)).signatures().filter((s) => s.signed).map((s) => sign.check(two, s.range, s.contents)));
  ok(checks.every((c) => c.ok) && checks.filter((c) => c.whole).length === 1, JSON.stringify(checks.map((c) => [c.ok, c.whole])));
});

await t('a changed byte, or a byte range wider than the hole, fails the check', async () => {
  const s = (await Pdf.open(signed)).signatures().find((x) => x.signed);
  const bent = new Uint8Array(signed);
  bent[600] ^= 1;
  ok(!(await sign.check(bent, s.range, s.contents)).ok, 'a changed byte passed');
  const r = s.range;
  ok(!(await sign.check(signed, [r[0], r[1] - 10, r[2], r[3]], s.contents)).ok, 'a wider hole passed');
  ok(!(await sign.check(signed, [5, r[1], r[2], r[3]], s.contents)).ok, 'a range not from byte 0 passed');
});

await t('redacting a signed file writes it whole: the redacted words are in no revision', async () => {
  const p = await Pdf.open(signed);
  for (const h of p.match(0, /\b\d{4}-\d{4}-\d{4}\b/g)) await p.add(0, { type: 'Redact', quads: h.quads });
  const bytes = p.save();
  ok(Buffer.compare(Buffer.from(bytes.slice(0, signed.length)), Buffer.from(signed)) !== 0, 'saved as an update after the signed bytes');
  ok(!raw(bytes).includes('4471'), 'the account number survived');
});

fs.rmSync(TMP, { recursive: true, force: true });
console.log(failed.length ? `${failed.length} failed` : 'all passed');
process.exit(failed.length ? 1 : 0);
