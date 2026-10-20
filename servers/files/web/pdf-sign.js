// Signatures a PDF reader can check: a CMS SignedData over the file's bytes
// but the signature's own hole (PAdES baseline, ETSI.CAdES.detached), made
// with a P-256 key made on the member's device, unexportable, that never
// leaves it. The certificate is the member's own, signed by that key: a reader shows
// who signed and that nothing changed since, and that no authority vouches
// for the name. Also reads the signatures a PDF already has and checks them.
// No library: DER is written and read here, and WebCrypto does the maths.

const subtle = globalThis.crypto.subtle;

const OID = {
  data: '1.2.840.113549.1.7.1',
  signedData: '1.2.840.113549.1.7.2',
  contentType: '1.2.840.113549.1.9.3',
  messageDigest: '1.2.840.113549.1.9.4',
  signingTime: '1.2.840.113549.1.9.5',
  signingCertV2: '1.2.840.113549.1.9.16.2.47',
  sha256: '2.16.840.1.101.3.4.2.1',
  sha384: '2.16.840.1.101.3.4.2.2',
  sha512: '2.16.840.1.101.3.4.2.3',
  sha1: '1.3.14.3.2.26',
  ecPublicKey: '1.2.840.10045.2.1',
  p256: '1.2.840.10045.3.1.7',
  p384: '1.3.132.0.34',
  ecdsaSha256: '1.2.840.10045.4.3.2',
  ecdsaSha384: '1.2.840.10045.4.3.3',
  ecdsaSha512: '1.2.840.10045.4.3.4',
  rsa: '1.2.840.113549.1.1.1',
  rsaSha1: '1.2.840.113549.1.1.5',
  rsaSha256: '1.2.840.113549.1.1.11',
  rsaSha384: '1.2.840.113549.1.1.12',
  rsaSha512: '1.2.840.113549.1.1.13',
  rsaPss: '1.2.840.113549.1.1.10',
  cn: '2.5.4.3',
  o: '2.5.4.10',
  email: '1.2.840.113549.1.9.1',
  keyUsage: '2.5.29.15',
  basicConstraints: '2.5.29.19',
  subjectKeyId: '2.5.29.14',
};

const cat = (...parts) => {
  const out = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
  let at = 0;
  for (const p of parts) { out.set(p, at); at += p.length; }
  return out;
};

function len(n) {
  if (n < 128) return Uint8Array.of(n);
  const b = [];
  while (n) { b.unshift(n & 255); n >>>= 8; }
  return Uint8Array.of(0x80 | b.length, ...b);
}
const tlv = (tag, body) => cat(Uint8Array.of(tag), len(body.length), body);
const seq = (...k) => tlv(0x30, cat(...k));
const set = (...k) => tlv(0x31, cat(...k));
const ctx = (n, body, constructed = true) => tlv((constructed ? 0xa0 : 0x80) | n, body);
const octets = (b) => tlv(0x04, b);
const bool = (v) => Uint8Array.of(1, 1, v ? 255 : 0);
const utf8 = (s) => tlv(0x0c, new TextEncoder().encode(s));
const bits = (b, unused = 0) => tlv(0x03, cat(Uint8Array.of(unused), b));

function int(v) {
  let b = typeof v === 'number' ? (() => { const a = []; do { a.unshift(v & 255); v = Math.floor(v / 256); } while (v); return Uint8Array.from(a); })() : v;
  let i = 0;
  while (i < b.length - 1 && b[i] === 0 && !(b[i + 1] & 0x80)) i++;
  b = b.subarray(i);
  return tlv(0x02, b[0] & 0x80 ? cat(Uint8Array.of(0), b) : b);
}

function oid(s) {
  const p = s.split('.').map(Number);
  const out = [40 * p[0] + p[1]];
  for (const n of p.slice(2)) {
    const b = [n & 127];
    for (let v = Math.floor(n / 128); v; v = Math.floor(v / 128)) b.unshift(128 | (v & 127));
    out.push(...b);
  }
  return tlv(0x06, Uint8Array.from(out));
}

function time(d) {
  const p = (n) => String(n).padStart(2, '0');
  const y = d.getUTCFullYear();
  const s = `${p(d.getUTCMonth() + 1)}${p(d.getUTCDate())}${p(d.getUTCHours())}${p(d.getUTCMinutes())}${p(d.getUTCSeconds())}Z`;
  // UTCTime until 2049, as RFC 5280 asks
  return y < 2050 ? tlv(0x17, new TextEncoder().encode(String(y).slice(2) + s)) : tlv(0x18, new TextEncoder().encode(y + s));
}

const algo = (o, params) => seq(oid(o), ...(params === undefined ? [] : [params]));
const name = (cn) => seq(set(seq(oid(OID.cn), utf8(cn))), set(seq(oid(OID.o), utf8('Commonty'))));

// WebCrypto's ECDSA signature is r||s; CMS and X.509 want SEQUENCE { r, s }
function ecdsaDer(raw) {
  const h = raw.length / 2;
  return seq(int(raw.subarray(0, h)), int(raw.subarray(h)));
}

export function parse(b, at = 0) {
  const tag = b[at];
  let n = b[at + 1], off = at + 2;
  if (n & 0x80) {
    const k = n & 127;
    n = 0;
    for (let i = 0; i < k; i++) n = n * 256 + b[off + i];
    off += k;
  }
  const node = { tag, start: at, body: off, end: off + n, bytes: b.subarray(at, off + n), value: b.subarray(off, off + n) };
  if (tag & 0x20) {
    node.kids = [];
    for (let p = off; p < off + n;) { const k = parse(b, p); node.kids.push(k); p = k.end; }
  }
  return node;
}

function readOid(node) {
  const v = node.value;
  const out = [Math.floor(v[0] / 40), v[0] % 40];
  for (let i = 1, n = 0; i < v.length; i++) {
    n = n * 128 + (v[i] & 127);
    if (!(v[i] & 128)) { out.push(n); n = 0; }
  }
  return out.join('.');
}

function readTime(node) {
  const s = new TextDecoder().decode(node.value);
  const full = node.tag === 0x17 ? (Number(s.slice(0, 2)) < 50 ? '20' : '19') + s : s;
  return new Date(Date.UTC(+full.slice(0, 4), +full.slice(4, 6) - 1, +full.slice(6, 8), +full.slice(8, 10), +full.slice(10, 12), +full.slice(12, 14) || 0));
}

function readName(node) {
  const out = {};
  for (const rdn of node.kids || []) for (const atv of rdn.kids || []) {
    const k = readOid(atv.kids[0]);
    const v = new TextDecoder(atv.kids[1].tag === 0x1e ? 'utf-16be' : 'utf-8').decode(atv.kids[1].value);
    if (k === OID.cn) out.cn = v;
    if (k === OID.o) out.o = v;
    if (k === OID.email) out.email = v;
  }
  return out;
}

/// A new key and its certificate, for `who` (the name a reader shows).
/// The private key is made unexportable: it never leaves WebCrypto, so it
/// never leaves this device. The caller keeps the returned object in the
/// device's own storage (IndexedDB holds a CryptoKey as it is).
export async function makeIdentity(who) {
  const pair = await subtle.generateKey({ name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign', 'verify']);
  const spki = new Uint8Array(await subtle.exportKey('spki', pair.publicKey));
  const keyId = new Uint8Array(await subtle.digest('SHA-1', parse(spki).kids[1].value.subarray(1))).subarray(0, 20);
  const now = new Date();
  const until = new Date(now.getTime() + 20 * 365.25 * 864e5);
  const serial = crypto.getRandomValues(new Uint8Array(16));
  serial[0] &= 0x7f;
  const tbs = seq(
    ctx(0, int(2)),
    int(serial),
    algo(OID.ecdsaSha256),
    name(who),
    seq(time(now), time(until)),
    name(who),
    spki,
    ctx(3, seq(
      seq(oid(OID.basicConstraints), octets(seq())),
      // digitalSignature and nonRepudiation (contentCommitment)
      seq(oid(OID.keyUsage), bool(true), octets(bits(Uint8Array.of(0xc0), 6))),
      seq(oid(OID.subjectKeyId), octets(octets(keyId))),
    )),
  );
  const sig = new Uint8Array(await subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, pair.privateKey, tbs));
  const cert = seq(tbs, algo(OID.ecdsaSha256), bits(ecdsaDer(sig)));
  return { v: 2, who, key: pair.privateKey, cert, made: now.toISOString() };
}

/// a kept identity, ready to sign with
export function loadIdentity(kept) {
  return { who: kept.who, key: kept.key, cert: kept.cert, info: certInfo(kept.cert) };
}

export function certInfo(der) {
  const c = parse(der);
  const tbs = c.kids[0];
  const k = tbs.kids[0].tag === 0xa0 ? tbs.kids.slice(1) : tbs.kids;
  return {
    serial: k[0].value,
    issuer: readName(k[2]),
    issuerDer: k[2].bytes,
    from: readTime(k[3].kids[0]),
    until: readTime(k[3].kids[1]),
    subject: readName(k[4]),
    spki: k[5].bytes,
    selfSigned: bytesEq(k[2].bytes, k[4].bytes),
  };
}

const bytesEq = (a, b) => a.length === b.length && a.every((x, i) => x === b[i]);

/// The CMS SignedData (DER) for `content`: the file's bytes but the hole.
export async function cms(id, content, when = new Date()) {
  const digest = new Uint8Array(await subtle.digest('SHA-256', content));
  const certHash = new Uint8Array(await subtle.digest('SHA-256', id.cert));
  const info = certInfo(id.cert);
  const issuerSerial = seq(seq(ctx(4, info.issuerDer)), int(info.serial));
  const attrs = [
    seq(oid(OID.contentType), set(oid(OID.data))),
    seq(oid(OID.signingTime), set(time(when))),
    seq(oid(OID.messageDigest), set(octets(digest))),
    // ESS signing-certificate-v2: which certificate, so it cannot be swapped
    seq(oid(OID.signingCertV2), set(seq(seq(seq(octets(certHash), issuerSerial))))),
  ];
  // the signature is over the attributes as a SET (tag 0x31), stored as [0]
  const signedSet = set(...sortDer(attrs));
  const raw = new Uint8Array(await subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, id.key, signedSet));
  const signerInfo = seq(
    int(1),
    seq(info.issuerDer, int(info.serial)),
    algo(OID.sha256),
    ctx(0, signedSet.subarray(signedSet.length - lenOfBody(signedSet))),
    algo(OID.ecdsaSha256),
    octets(ecdsaDer(raw)),
  );
  const sd = seq(
    int(1),
    set(algo(OID.sha256)),
    seq(oid(OID.data)),
    ctx(0, id.cert),
    set(signerInfo),
  );
  return seq(oid(OID.signedData), ctx(0, sd));
}

function lenOfBody(t) { const n = parse(t); return n.end - n.body; }
// DER sorts a SET OF by its encodings
function sortDer(list) {
  return [...list].sort((a, b) => {
    for (let i = 0; i < Math.min(a.length, b.length); i++) if (a[i] !== b[i]) return a[i] - b[i];
    return a.length - b.length;
  });
}

export const HOLE = 16384;                       // bytes of signature room
export const RANGE_MARK = 2147483647;            // a /ByteRange no file has: ten digits, an int to MuPDF
const enc = new TextDecoder('latin1');

/// A tag for one signing, written into the signature's own dictionary by
/// prepareSignature, so its hole is found by structure and never by a
/// look-alike elsewhere in the file
export const nonce = () => [...crypto.getRandomValues(new Uint8Array(16))].map((x) => x.toString(16).padStart(2, '0')).join('');

/// Find the signature's hole in `pdf` (Uint8Array): inside the one object
/// that carries `tag`, its /ByteRange of RANGE_MARKs and its /Contents of
/// HOLE zero bytes. The tag must be in the file exactly once.
function hole(pdf, tag) {
  const s = enc.decode(pdf);
  const at = s.indexOf(tag);
  if (at < 0 || s.indexOf(tag, at + 1) >= 0) throw new Error('the signature could not be found in the file');
  const from = s.lastIndexOf(' obj', at);
  const to = s.indexOf('endobj', at);
  if (from < 0 || to < 0) throw new Error('the signature could not be found in the file');
  const obj = s.slice(from, to);
  const r = obj.search(new RegExp(`/ByteRange\\s*\\[\\s*0\\s+${RANGE_MARK}\\s+${RANGE_MARK}\\s+${RANGE_MARK}\\s*\\]`));
  const c = obj.search(new RegExp(`/Contents\\s*<0{${HOLE * 2}}>`));
  if (r < 0 || c < 0) throw new Error('the signature has no room in the file');
  const rangeAt = from + r;
  const start = s.indexOf('<', from + c);
  return { range: s.slice(rangeAt, s.indexOf(']', rangeAt) + 1), rangeAt, start, end: start + HOLE * 2 + 2 };
}

/// Fill the hole: the byte range, then the signature over everything else.
export async function seal(pdf, id, tag, when) {
  const h = hole(pdf, tag);
  const br = [0, h.start, h.end, pdf.length - h.end];
  const text = `/ByteRange [${br.join(' ')}]`;
  if (text.length > h.range.length) throw new Error('the byte range does not fit');
  const out = new Uint8Array(pdf);
  out.set(new TextEncoder().encode(text.padEnd(h.range.length, ' ')), h.rangeAt);
  const signed = cat(out.subarray(0, br[1]), out.subarray(br[2]));
  const der = await cms(id, signed, when);
  const hex = [...der].map((x) => x.toString(16).padStart(2, '0')).join('');
  if (hex.length > h.end - h.start - 2) throw new Error('the signature is bigger than its room');
  out.set(new TextEncoder().encode(hex), h.start + 1);
  return out;
}

// SHA-1 is not among them: a signature over it can be forged, so it is refused
const HASH = { [OID.sha256]: 'SHA-256', [OID.sha384]: 'SHA-384', [OID.sha512]: 'SHA-512' };
const SIGHASH = {
  [OID.ecdsaSha256]: 'SHA-256', [OID.ecdsaSha384]: 'SHA-384', [OID.ecdsaSha512]: 'SHA-512',
  [OID.rsaSha256]: 'SHA-256', [OID.rsaSha384]: 'SHA-384', [OID.rsaSha512]: 'SHA-512',
};

/// What one signature says and whether it holds: `pdf` is the whole file,
/// `range` its /ByteRange, `contents` the /Contents bytes.
export async function check(pdf, range, contents) {
  const out = { ok: false, whole: false, who: null, when: null, why: '' };
  try {
    const [a, b, c, d] = range;
    // the range must start at the file's first byte and leave out exactly
    // this signature's own <hex> value, nothing more
    if (a !== 0 || !(b > 0) || !(c > b) || !(d >= 0) || c + d > pdf.length) { out.why = 'its byte range does not make sense'; return out; }
    if (pdf[b] !== 0x3c || pdf[c - 1] !== 0x3e) { out.why = 'its byte range leaves out more than the signature'; return out; }
    const hole = new TextDecoder('latin1').decode(pdf.subarray(b + 1, c - 1));
    const want = [...contents].map((x) => x.toString(16).padStart(2, '0')).join('');
    if (!/^[0-9a-fA-F]*$/.test(hole) || !hole.toLowerCase().startsWith(want) || /[^0]/.test(hole.slice(want.length))) { out.why = 'its byte range leaves out more than the signature'; return out; }
    out.whole = c + d === pdf.length;
    const signed = cat(pdf.subarray(0, b), pdf.subarray(c, c + d));
    const ci = parse(contents);
    const sd = ci.kids[1].kids[0];
    const certs = sd.kids.find((k) => k.tag === 0xa0);
    const si = sd.kids.at(-1).kids[0];
    const sid = si.kids[1];
    const digestAlg = readOid(si.kids[2].kids[0]);
    const signedAttrs = si.kids.find((k) => k.tag === 0xa0);
    const sigAlgAt = si.kids.indexOf(signedAttrs) + 1;
    const sigAlg = readOid(si.kids[sigAlgAt].kids[0]);
    const sigValue = si.kids[sigAlgAt + 1].value;
    // the signer's certificate: the one its issuer and serial name, no other
    let cert = null;
    for (const k of certs?.kids || []) {
      const info = certInfo(k.bytes);
      if (sid.tag === 0x30 && bytesEq(info.issuerDer, sid.kids[0].bytes) && bytesEq(info.serial, sid.kids[1].value)) { cert = { der: k.bytes, info }; break; }
    }
    if (!cert) { out.why = 'the certificate it names is not in it'; return out; }
    out.who = cert.info.subject.cn || cert.info.subject.o || cert.info.subject.email || 'someone';
    out.issuer = cert.info.issuer.cn || cert.info.issuer.o || '';
    out.selfSigned = cert.info.selfSigned;
    const hash = HASH[digestAlg];
    if (digestAlg === OID.sha1 || sigAlg === OID.rsaSha1) { out.why = 'it uses SHA-1, which can be forged'; return out; }
    if (!hash) { out.why = 'it uses a kind of signature this page cannot check'; return out; }
    const digest = new Uint8Array(await subtle.digest(hash, signed));
    let toVerify = signed;
    if (signedAttrs) {
      let digestSeen = false;
      for (const at of signedAttrs.kids) {
        const t = readOid(at.kids[0]);
        if (t === OID.messageDigest) {
          digestSeen = true;
          if (!bytesEq(at.kids[1].kids[0].value, digest)) { out.why = 'the file changed after it was signed'; return out; }
        }
        if (t === OID.signingTime) out.when = readTime(at.kids[1].kids[0]);
        if (t === OID.contentType && readOid(at.kids[1].kids[0]) !== OID.data) { out.why = 'it says it signed something other than the file'; return out; }
        if (t === OID.signingCertV2) {
          // which certificate was meant: its hash must be this one's
          const id = at.kids[1].kids[0].kids[0].kids[0];
          const first = id.kids[0];
          const alg = first.tag === 0x30 ? HASH[readOid(first.kids[0])] : 'SHA-256';
          const h = (first.tag === 0x30 ? id.kids[1] : first).value;
          if (!alg || !bytesEq(new Uint8Array(await subtle.digest(alg, cert.der)), h)) { out.why = 'it names a different certificate from the one it carries'; return out; }
        }
      }
      if (!digestSeen) { out.why = 'it does not say what it signed'; return out; }
      toVerify = cat(Uint8Array.of(0x31), signedAttrs.bytes.subarray(1));
    }
    const spki = cert.info.spki;
    const keyAlg = readOid(parse(spki).kids[0].kids[0]);
    let ok = false;
    if (keyAlg === OID.ecPublicKey) {
      const curve = readOid(parse(spki).kids[0].kids[1]);
      const named = curve === OID.p256 ? 'P-256' : curve === OID.p384 ? 'P-384' : null;
      if (!named) { out.why = 'it uses a curve this page cannot check'; return out; }
      const key = await subtle.importKey('spki', spki, { name: 'ECDSA', namedCurve: named }, false, ['verify']);
      const der = parse(sigValue);
      const size = named === 'P-256' ? 32 : 48;
      const fix = (v) => { const x = v[0] === 0 ? v.subarray(1) : v; const o = new Uint8Array(size); o.set(x, size - x.length); return o; };
      ok = await subtle.verify({ name: 'ECDSA', hash: SIGHASH[sigAlg] || hash }, key, cat(fix(der.kids[0].value), fix(der.kids[1].value)), toVerify);
    } else if (keyAlg === OID.rsa) {
      if (sigAlg === OID.rsaPss) { out.why = 'it uses a kind of signature this page cannot check'; return out; }
      const key = await subtle.importKey('spki', spki, { name: 'RSASSA-PKCS1-v1_5', hash: SIGHASH[sigAlg] || hash }, false, ['verify']);
      ok = await subtle.verify('RSASSA-PKCS1-v1_5', key, sigValue, toVerify);
    } else {
      out.why = 'it uses a kind of key this page cannot check';
      return out;
    }
    out.ok = ok;
    if (!ok) out.why = 'the signature does not match the file';
    // whether its certificate was in date when it says it signed (or now)
    const at = out.when || new Date();
    out.from = cert.info.from;
    out.until = cert.info.until;
    out.inDate = at >= cert.info.from && at <= cert.info.until;
    return out;
  } catch (e) {
    out.why = `it could not be read (${e.message})`;
    return out;
  }
}
