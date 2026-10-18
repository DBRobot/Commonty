// Reading an email (RFC 5322 + MIME): headers with their encoded words, the
// parts of a multipart, base64 and quoted-printable, any charset the
// platform's TextDecoder knows. The Worker reads only what a summary needs;
// the mail page reads all of it. Nothing here runs a message's own code.

const td8 = new TextDecoder('utf-8');
const latin1 = new TextDecoder('latin1');

export function decoder(charset) {
  try { return new TextDecoder((charset || 'utf-8').trim().replace(/^"|"$/g, '').toLowerCase(), { fatal: false }); } catch { return td8; }
}

// the byte offset just past the header block (the first empty line)
function headerEnd(b) {
  for (let i = 0; i + 1 < b.length; i++) {
    if (b[i] === 10 && b[i + 1] === 10) return [i, i + 2];
    if (b[i] === 13 && b[i + 1] === 10 && b[i + 2] === 13 && b[i + 3] === 10) return [i, i + 4];
    if (i === 0 && (b[0] === 10 || (b[0] === 13 && b[1] === 10))) return [0, b[0] === 10 ? 1 : 2];
  }
  return [b.length, b.length];
}

export function words(s) {
  // =?charset?B|Q?text?= ; whitespace between two encoded words goes
  return String(s).replace(/(=\?[^?\s]+\?[bBqQ]\?[^?\s]*\?=)\s+(?==\?)/g, '$1').replace(/=\?([^?\s]+)\?([bBqQ])\?([^?\s]*)\?=/g, (all, cs, enc, text) => {
    try {
      const bytes = enc.toUpperCase() === 'B'
        ? Uint8Array.from(atob(text.replace(/[^A-Za-z0-9+/=]/g, '')), (c) => c.charCodeAt(0))
        : qp(text.replace(/_/g, ' '), true);
      return decoder(cs.split('*')[0]).decode(bytes);
    } catch { return all; }
  });
}

export function headers(text) {
  const out = [];
  for (const line of text.split(/\r?\n/)) {
    if (/^[ \t]/.test(line) && out.length) out[out.length - 1][1] += ' ' + line.trim();
    else {
      const i = line.indexOf(':');
      if (i > 0) out.push([line.slice(0, i).trim().toLowerCase(), line.slice(i + 1).trim()]);
    }
  }
  return out;
}
const first = (h, name) => (h.find((x) => x[0] === name) || [])[1];

// a header value's parameters: type/sub; a=b; c="d e"; RFC 2231 name*=utf-8''x
export function params(v) {
  const [main, ...rest] = String(v || '').split(/;(?=(?:[^"]*"[^"]*")*[^"]*$)/);
  const p = { value: main.trim().toLowerCase() };
  const ext = {};
  for (const r of rest) {
    const i = r.indexOf('=');
    if (i < 0) continue;
    let k = r.slice(0, i).trim().toLowerCase();
    let val = r.slice(i + 1).trim().replace(/^"(.*)"$/s, '$1').replace(/\\(.)/g, '$1');
    const m = k.match(/^([^*]+)\*(\d+)?(\*)?$/);
    if (m) {
      if (!ext[m[1]]) ext[m[1]] = [];
      ext[m[1]].push([Number(m[2] || 0), val, !!m[3] || (!m[2] && k.endsWith('*'))]);
      continue;
    }
    p[k] = words(val);
  }
  for (const [k, parts] of Object.entries(ext)) {
    parts.sort((a, b) => a[0] - b[0]);
    let cs = 'utf-8';
    const bytes = [];
    parts.forEach(([n, val, enc], idx) => {
      if (enc && idx === 0 && /^[^']*'[^']*'/.test(val)) { cs = val.split("'")[0] || 'utf-8'; val = val.split("'").slice(2).join("'"); }
      if (enc) for (let i = 0; i < val.length; i++) {
        if (val[i] === '%' && /^[0-9a-f]{2}$/i.test(val.slice(i + 1, i + 3))) { bytes.push(parseInt(val.slice(i + 1, i + 3), 16)); i += 2; } else bytes.push(val.charCodeAt(i) & 255);
      } else for (const c of new TextEncoder().encode(val)) bytes.push(c);
    });
    p[k] = decoder(cs).decode(Uint8Array.from(bytes));
  }
  return p;
}

export function qp(s, inWord = false) {
  const out = [];
  const t = inWord ? s : s.replace(/=\r?\n/g, '');
  for (let i = 0; i < t.length; i++) {
    const c = t[i];
    if (c === '=' && /^[0-9A-Fa-f]{2}$/.test(t.slice(i + 1, i + 3))) { out.push(parseInt(t.slice(i + 1, i + 3), 16)); i += 2; } else out.push(t.charCodeAt(i) & 255);
  }
  return Uint8Array.from(out);
}
function unb64(s) {
  const clean = s.replace(/[^A-Za-z0-9+/]/g, '');
  const pad = clean.length % 4 === 1 ? clean.slice(0, -1) : clean;
  try { return Uint8Array.from(atob(pad), (c) => c.charCodeAt(0)); } catch { return new Uint8Array(0); }
}
export function decodeBody(bytes, cte) {
  const e = String(cte || '').trim().toLowerCase();
  if (e === 'base64') return unb64(latin1.decode(bytes));
  if (e === 'quoted-printable') return qp(latin1.decode(bytes));
  return bytes;
}

function indexOfBytes(hay, needle, from) {
  outer: for (let i = from; i <= hay.length - needle.length; i++) {
    for (let j = 0; j < needle.length; j++) if (hay[i + j] !== needle[j]) continue outer;
    return i;
  }
  return -1;
}
function splitParts(body, boundary) {
  const b = new TextEncoder().encode('--' + boundary);
  const parts = [];
  let i = indexOfBytes(body, b, 0);
  while (i >= 0) {
    let start = i + b.length;
    if (body[start] === 45 && body[start + 1] === 45) break;
    while (start < body.length && body[start] !== 10) start++;
    start++;
    const next = indexOfBytes(body, b, start);
    let end = next < 0 ? body.length : next;
    if (body[end - 1] === 10) end--;
    if (body[end - 1] === 13) end--;
    parts.push(body.subarray(start, end));
    if (next < 0) break;
    i = next;
  }
  return parts;
}

// one part, and the parts inside it: {headers, type, params, disposition,
// filename, cid, body (decoded bytes), parts}
export function parse(raw, depth = 0) {
  const bytes = raw instanceof Uint8Array ? raw : new TextEncoder().encode(String(raw));
  const [he, bs] = headerEnd(bytes);
  const h = headers(latin1.decode(bytes.subarray(0, he)).replace(/[\u0080-ÿ]+/g, (m) => td8.decode(Uint8Array.from(m, (c) => c.charCodeAt(0)))));
  const ct = params(first(h, 'content-type') || 'text/plain; charset=us-ascii');
  const cd = params(first(h, 'content-disposition') || '');
  const body = bytes.subarray(bs);
  const node = {
    headers: h,
    type: ct.value || 'text/plain',
    params: ct,
    disposition: cd.value || '',
    filename: cd.filename || ct.name || '',
    cid: (first(h, 'content-id') || '').replace(/^<|>$/g, ''),
    parts: [],
    size: body.length,
  };
  if (node.type.startsWith('multipart/') && ct.boundary && depth < 12) {
    node.parts = splitParts(body, ct.boundary).map((p) => parse(p, depth + 1));
    node.body = new Uint8Array(0);
  } else if (node.type === 'message/rfc822' && depth < 12) {
    node.body = decodeBody(body, first(h, 'content-transfer-encoding'));
    node.parts = [parse(node.body, depth + 1)];
  } else {
    node.body = decodeBody(body, first(h, 'content-transfer-encoding'));
  }
  return node;
}

export const text = (part) => decoder(part.params.charset).decode(part.body);

function* walk(p) { yield p; for (const c of p.parts) yield* walk(c); }

// what a reader shows: the html (or text) body, the attachments, the parts a
// body names by cid:
export function readable(root) {
  let html = null, plain = null;
  const att = [];
  const inline = {};
  const pick = (p) => {
    if (p.type === 'multipart/alternative') {
      const kids = p.parts;
      const h = [...kids].reverse().find((k) => k.type === 'text/html' || k.type.startsWith('multipart/'));
      const t = kids.find((k) => k.type === 'text/plain');
      if (t && plain === null) plain = text(t);
      if (h) pick(h);
      return;
    }
    if (p.type.startsWith('multipart/')) { for (const k of p.parts) pick(k); return; }
    const isAtt = p.disposition === 'attachment' || (p.filename && !p.type.startsWith('text/'));
    if (p.cid && p.type.startsWith('image/') && p.disposition !== 'attachment') { inline[p.cid] = p; return; }
    if (isAtt) { att.push(p); return; }
    if (p.type === 'text/html' && html === null) { html = text(p); return; }
    if (p.type === 'text/plain' && plain === null && p.disposition !== 'attachment') { plain = text(p); return; }
    if (p.type === 'message/rfc822') att.push(p);
    else if (!p.type.startsWith('text/')) att.push(p);
  };
  pick(root);
  return { html, plain, attachments: att, inline };
}

// "Name" <a@b>, c@d  ->  [{name, address}]
export function addresses(v) {
  const s = words(v || '');
  const out = [];
  let cur = '', q = false, a = 0;
  for (const ch of s) {
    if (ch === '"') q = !q;
    if (ch === '<') a++;
    if (ch === '>') a--;
    if (ch === ',' && !q && a <= 0) { out.push(cur); cur = ''; continue; }
    cur += ch;
  }
  out.push(cur);
  return out.map((x) => x.trim()).filter(Boolean).map((x) => {
    const m = x.match(/^(.*?)<\s*([^>\s]+)\s*>$/);
    if (m) return { name: m[1].trim().replace(/^"(.*)"$/, '$1').replace(/\\(.)/g, '$1'), address: m[2].toLowerCase() };
    return { name: '', address: x.replace(/[<>]/g, '').toLowerCase() };
  });
}

export function plainOf(html) {
  return String(html)
    .replace(/<(script|style|head|title)[^>]*>[\s\S]*?<\/\1>/gi, ' ')
    .replace(/<br\s*\/?>|<\/(p|div|tr|li|h\d)>/gi, '\n')
    .replace(/<[^>]+>/g, ' ')
    .replace(/&nbsp;/g, ' ').replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;|&apos;/g, "'")
    .replace(/&#(\d+);/g, (_, n) => String.fromCodePoint(Number(n)))
    .replace(/[ \t ]+/g, ' ').replace(/\n\s*\n+/g, '\n').trim();
}

// what the list shows without opening the message, from at most `limit`
// bytes of it (the Worker's CPU is short)
export function summary(raw, limit = 256 * 1024) {
  const bytes = raw.length > limit ? raw.subarray(0, limit) : raw;
  const root = parse(bytes);
  const h = root.headers;
  const r = readable(root);
  const body = r.plain ?? (r.html ? plainOf(r.html) : '');
  const g = (n) => words(first(h, n) || '');
  return {
    from: addresses(first(h, 'from'))[0] || { name: '', address: '' },
    to: addresses(first(h, 'to')),
    cc: addresses(first(h, 'cc')),
    replyTo: addresses(first(h, 'reply-to'))[0] || null,
    subject: g('subject').slice(0, 500),
    date: Date.parse(first(h, 'date') || '') || null,
    mid: (first(h, 'message-id') || '').trim().slice(0, 300),
    irt: (first(h, 'in-reply-to') || '').trim().slice(0, 300),
    refs: (first(h, 'references') || '').trim().split(/\s+/).filter(Boolean).slice(-20),
    snippet: body.replace(/\s+/g, ' ').trim().slice(0, 240),
    attachments: r.attachments.map((a) => a.filename || a.type).slice(0, 20),
    list: !!first(h, 'list-unsubscribe') || !!first(h, 'list-id'),
    size: raw.length,
  };
}
