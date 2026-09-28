// A Send, opened by anyone with its link: https://vault.<domain>/#/send/<id>/<key>.
// The key is after the #, so it never reaches a server; the text or file
// is fetched as ciphertext and decrypted here with Bitwarden's Send scheme
// (HKDF of the link's key; AES-256-CBC with HMAC-SHA256).
const $ = (id) => document.getElementById(id);
const enc = new TextEncoder();
const dec = new TextDecoder();

function b64(s) {
  s = s.replace(/-/g, '+').replace(/_/g, '/');
  while (s.length % 4) s += '=';
  return Uint8Array.from(atob(s), (c) => c.charCodeAt(0));
}
const tob64 = (u) => btoa(String.fromCharCode(...new Uint8Array(u)));

function say(text, bad = false) {
  $('note').textContent = text;
  $('note').classList.toggle('bad', bad);
}

// the two keys a Send's parts are sealed with, from the 16 bytes in the link
async function keys(raw) {
  const ikm = await crypto.subtle.importKey('raw', raw, 'HKDF', false, ['deriveBits']);
  const bits = new Uint8Array(await crypto.subtle.deriveBits(
    { name: 'HKDF', hash: 'SHA-256', salt: enc.encode('bitwarden-send'), info: enc.encode('send') }, ikm, 512));
  return {
    enc: await crypto.subtle.importKey('raw', bits.slice(0, 32), 'AES-CBC', false, ['decrypt']),
    mac: await crypto.subtle.importKey('raw', bits.slice(32), { name: 'HMAC', hash: 'SHA-256' }, false, ['verify']),
  };
}

async function open(k, iv, data, mac) {
  const signed = new Uint8Array(iv.length + data.length);
  signed.set(iv);
  signed.set(data, iv.length);
  if (!await crypto.subtle.verify('HMAC', k.mac, mac, signed)) throw new Error('this Send has been tampered with');
  return crypto.subtle.decrypt({ name: 'AES-CBC', iv }, k.enc, data);
}

// "2.<iv>|<data>|<mac>"
async function openString(k, s) {
  if (!s) return '';
  const [type, rest] = s.split('.', 2);
  if (type !== '2') throw new Error('unknown encryption');
  const [iv, data, mac] = rest.split('|').map(b64);
  return dec.decode(await open(k, iv, data, mac));
}

// a file: one type byte, then iv, mac and data
async function openFile(k, buf) {
  const u = new Uint8Array(buf);
  if (u[0] !== 2) throw new Error('unknown encryption');
  return open(k, u.slice(1, 17), u.slice(49), u.slice(17, 49));
}

const pick = (o, ...names) => names.map((n) => o?.[n]).find((v) => v !== undefined);

async function main() {
  const m = location.hash.match(/^#\/send\/([^/]+)\/([^/?#]+)/);
  if (!m) {
    $('name').textContent = 'Nothing to open';
    $('why').textContent = 'This link is missing the part that opens a Send. Ask whoever sent it for the whole link.';
    return;
  }
  const [, id, keyText] = m;
  const raw = b64(keyText);
  const k = await keys(raw);
  let password;

  const access = async () => {
    const r = await fetch(`/api/sends/access/${encodeURIComponent(id)}`, {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(password ? { password } : {}),
    });
    if (r.status === 401) return 'password';
    if (r.status === 400 && password) return 'wrong';
    if (r.status === 404 || r.status === 400) return 'gone';
    if (!r.ok) throw new Error(`the server said ${r.status}`);
    return r.json();
  };

  const show = async (send) => {
    $('pw').hidden = true;
    $('name').textContent = await openString(k, pick(send, 'name', 'Name')) || 'A Send';
    const type = pick(send, 'type', 'Type');
    if (type === 0) {
      const t = pick(send, 'text', 'Text');
      $('body').textContent = await openString(k, pick(t, 'text', 'Text'));
      if (pick(t, 'hidden', 'Hidden')) {
        $('body').classList.add('hidden');
        $('reveal').hidden = false;
        $('reveal').onclick = () => {
          $('body').classList.toggle('hidden');
          $('reveal').textContent = $('body').classList.contains('hidden') ? 'Show' : 'Hide';
        };
      }
      $('copy').className = 'quiet';
      $('copy').onclick = async () => {
        await navigator.clipboard.writeText($('body').textContent);
        say('Copied.');
      };
      $('text').hidden = false;
    } else {
      const f = pick(send, 'file', 'File');
      const fileId = pick(f, 'id', 'Id');
      const name = await openString(k, pick(f, 'fileName', 'FileName'));
      $('filename').textContent = name;
      $('filesize').textContent = pick(f, 'sizeName', 'SizeName') || '';
      $('file').hidden = false;
      $('download').onclick = async () => {
        say('Fetching and decrypting…');
        try {
          const r = await fetch(`/api/sends/${pick(send, 'id', 'Id')}/access/file/${fileId}`, {
            method: 'POST',
            headers: { 'content-type': 'application/json' },
            body: JSON.stringify(password ? { password } : {}),
          });
          if (!r.ok) throw new Error(`the server said ${r.status}`);
          const { url } = await r.json();
          // only ever from this host, whatever the answer says
          const u = new URL(url, location.origin);
          const blob = await fetch(u.pathname + u.search).then((x) => {
            if (!x.ok) throw new Error(`the server said ${x.status}`);
            return x.arrayBuffer();
          });
          const plain = await openFile(k, blob);
          const a = document.createElement('a');
          a.href = URL.createObjectURL(new Blob([plain]));
          a.download = name || 'download';
          a.click();
          setTimeout(() => URL.revokeObjectURL(a.href), 60000);
          say('Saved.');
        } catch (e) {
          say(`Could not download it: ${e.message}`, true);
        }
      };
    }
    const exp = pick(send, 'expirationDate', 'ExpirationDate');
    if (exp) say(`Expires ${new Date(exp).toLocaleString()}.`);
  };

  const attempt = async () => {
    const got = await access();
    if (got === 'gone') {
      $('name').textContent = 'This Send is gone';
      $('why').textContent = 'It has expired, been opened as many times as it allows, or been deleted.';
      $('pw').hidden = true;
    } else if (got === 'password' || got === 'wrong') {
      $('name').textContent = 'A Send';
      $('pw').hidden = false;
      $('password').focus();
      if (got === 'wrong') say('That password is not right.', true);
    } else {
      say('');
      await show(got);
    }
  };

  $('pw').addEventListener('submit', async (e) => {
    e.preventDefault();
    const base = await crypto.subtle.importKey('raw', enc.encode($('password').value), 'PBKDF2', false, ['deriveBits']);
    password = tob64(await crypto.subtle.deriveBits(
      { name: 'PBKDF2', hash: 'SHA-256', salt: raw, iterations: 100000 }, base, 256));
    await attempt().catch((err) => say(err.message, true));
  });

  await attempt();
}

main().catch((e) => {
  $('name').textContent = 'This Send could not be opened';
  say(e.message, true);
});
