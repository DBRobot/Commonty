// An email from what the editor holds (docs/dev/mail.md): plain text beside
// the HTML, attachments after, headers in encoded words where they need
// them, every line CRLF and none longer than mail allows. No Bcc header:
// those addresses are the envelope's alone.

const te = new TextEncoder();
const CRLF = '\r\n';

const ascii = (s) => /^[\x20-\x7e]*$/.test(s);
function b64(bytes) {
  let s = '';
  for (let i = 0; i < bytes.length; i += 0x8000) s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
}
const wrap = (s, n = 76) => (s.match(new RegExp(`.{1,${n}}`, 'g')) || ['']).join(CRLF);

// a header's words, encoded only where they are not plain ASCII (RFC 2047)
export function encodeWords(s) {
  s = String(s).replace(/[\r\n]+/g, ' ');
  if (ascii(s)) return s;
  const out = [];
  let chunk = '';
  for (const ch of s) {
    if (te.encode(chunk + ch).length > 45) { out.push(chunk); chunk = ''; }
    chunk += ch;
  }
  if (chunk) out.push(chunk);
  return out.map((c) => `=?UTF-8?B?${b64(te.encode(c))}?=`).join(`${CRLF} `);
}

export function address(a) {
  const addr = String(a.address || a).trim();
  if (!/^[^\s<>@,;"]+@[^\s<>@,;"]+$/.test(addr)) throw new Error(`not an address: ${addr}`);
  const name = String(a.name || '').replace(/[\r\n"\\]/g, '').trim();
  if (!name) return addr;
  return ascii(name) ? `"${name}" <${addr}>` : `${encodeWords(name)} <${addr}>`;
}

// a header line folded at commas and spaces to stay under 78
function header(name, value) {
  const line = `${name}: ${value}`;
  if (line.length <= 78) return line;
  const parts = line.split(/(?<=,) /);
  let out = '', cur = '';
  for (const p of parts) {
    if (cur && (cur + ' ' + p).length > 76) { out += cur + CRLF + ' '; cur = p; } else cur = cur ? `${cur} ${p}` : p;
  }
  return out + cur;
}

function qpText(text) {
  // quoted-printable, soft breaks at 76
  const bytes = te.encode(text.replace(/\r?\n/g, '\n'));
  let line = '', out = '';
  const flush = (soft) => { out += line + (soft ? '=' : '') + CRLF; line = ''; };
  for (let i = 0; i < bytes.length; i++) {
    const b = bytes[i];
    if (b === 10) { if (/[ \t]$/.test(line)) line = line.slice(0, -1) + `=${line.charCodeAt(line.length - 1).toString(16).toUpperCase().padStart(2, '0')}`; flush(false); continue; }
    const enc = (b === 61 || b < 32 || b > 126) && b !== 9 ? `=${b.toString(16).toUpperCase().padStart(2, '0')}` : String.fromCharCode(b);
    if (line.length + enc.length > 75) flush(true);
    line += enc;
  }
  if (line) out += line;
  return out;
}

const boundary = () => `=_cm_${[...crypto.getRandomValues(new Uint8Array(12))].map((b) => b.toString(16).padStart(2, '0')).join('')}`;

export function messageId(domain) {
  return `<${[...crypto.getRandomValues(new Uint8Array(16))].map((b) => b.toString(16).padStart(2, '0')).join('')}@${domain}>`;
}

// {from, to, cc, subject, text, html, attachments: [{name, type, bytes}],
//  inReplyTo, references, domain, date} -> {raw: Uint8Array, mid}
export function compose(m) {
  const mid = m.mid || messageId(m.domain);
  const head = [
    header('From', address(m.from)),
    ...(m.to?.length ? [header('To', m.to.map(address).join(', '))] : []),
    ...(m.cc?.length ? [header('Cc', m.cc.map(address).join(', '))] : []),
    header('Subject', encodeWords(m.subject || '')),
    `Date: ${(m.date || new Date()).toUTCString().replace('GMT', '+0000')}`,
    `Message-ID: ${mid}`,
    ...(m.inReplyTo ? [`In-Reply-To: ${m.inReplyTo}`] : []),
    ...(m.references?.length ? [header('References', m.references.join(' '))] : []),
    'MIME-Version: 1.0',
  ];
  const text = m.text ?? '';
  const textPart = ['Content-Type: text/plain; charset=utf-8', 'Content-Transfer-Encoding: quoted-printable', '', qpText(text)].join(CRLF);
  let body;
  if (m.html) {
    const alt = boundary();
    const htmlPart = ['Content-Type: text/html; charset=utf-8', 'Content-Transfer-Encoding: quoted-printable', '', qpText(m.html)].join(CRLF);
    body = [`Content-Type: multipart/alternative; boundary="${alt}"`, '', `--${alt}`, textPart, `--${alt}`, htmlPart, `--${alt}--`].join(CRLF);
  } else body = textPart;
  if (m.attachments?.length) {
    const mix = boundary();
    const parts = [`Content-Type: multipart/mixed; boundary="${mix}"`, '', `--${mix}`, body];
    for (const a of m.attachments) {
      const name = String(a.name || 'file').replace(/[\r\n"\\]/g, '_');
      const fname = ascii(name) ? `filename="${name}"` : `filename*=UTF-8''${encodeURIComponent(name)}`;
      parts.push(`--${mix}`, `Content-Type: ${/^[\w.+-]+\/[\w.+-]+$/.test(a.type || '') ? a.type : 'application/octet-stream'}`, `Content-Disposition: attachment; ${fname}`, 'Content-Transfer-Encoding: base64', '', wrap(b64(a.bytes)));
    }
    parts.push(`--${mix}--`);
    body = parts.join(CRLF);
  }
  const raw = te.encode(head.join(CRLF) + CRLF + body + CRLF);
  return { raw, mid };
}

// the editor's HTML as plain text, for the part beside it
export function textOf(html) {
  return String(html)
    .replace(/<br\s*\/?>/gi, '\n')
    .replace(/<\/(p|div|h\d|li|blockquote|pre|tr)>/gi, '\n')
    .replace(/<li[^>]*>/gi, '• ')
    .replace(/<[^>]+>/g, '')
    .replace(/&nbsp;/g, ' ').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&amp;/g, '&')
    .replace(/\n{3,}/g, '\n\n')
    .trim();
}
