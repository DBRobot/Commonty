// Leave Commonty, a tab of Settings: everything taken out first, then the
// button. Leaving is a statement the account's root signs, agreed by every
// box (servers/gate/src/leaving.rs): their passkey for an account made in a
// browser, the app for one whose root is on a device. Within 30 days, the
// same tab brings them back with nothing lost.
//
// Taking things out happens here, where the app serves this page (its
// window, or a browser it let in): the library is zipped in this page with
// the key the app hands over, Photos is zipped by the app with the password
// it holds, the disk images are Backups' own downloads, and Passwords opens
// on the vault's own Export screen. A zip never sits whole in memory: it
// goes out a piece at a time, through the app to a download (or to the
// Downloads folder from the app's window), to a file the person picks, or,
// where neither can be had, into a Blob of pieces a browser keeps on disk.
import { requestOptions, assertion, post } from './webauthn.js';
import { docsUrl, inApp, servedByApp, appFetch } from './shell.js';
import { unlock, walk, pieces, human, save } from './library.js';
import { images } from './backups.js';
import { ZipWriter } from './zip.js';

const $ = (id) => document.getElementById(id);
const day = (t) => new Date(t * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric' });

let state = null;

export async function where() {
  if (!state) {
    const r = await fetch('/_dd/leave');
    if (!r.ok) throw new Error(`the box said ${r.status}`);
    state = await r.json();
  }
  return state;
}

// where the app serves this page, the app signs with the root it holds
const byApp = inApp || servedByApp;
// the app's routes: the window's carry its launch secret
const ask = (url, init) => (inApp ? appFetch(url, init) : fetch(url, init));

// the root signs the statement; the box puts it in the fleet's log
async function say(back) {
  if (!state.passkey) {
    const init = {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ back }),
    };
    const r = await ask('/_dd/app/leaving', init);
    if (!r.ok) throw new Error(await r.text());
    return;
  }
  const start = await post('/_dd/leave/start', { back });
  const { publicKey, ceremony } = await start.json();
  const cred = await navigator.credentials.get({ publicKey: requestOptions(publicKey) });
  await post('/_dd/leave/sign', assertion(cred), { 'x-dd-ceremony': ceremony });
}

function show(s) {
  // anyone may leave: a member, a guest, or an account still waiting
  const role = s.role || (s.member ? 'member' : 'waiting');
  $('lv-member').hidden = !!s.left;
  $('lv-take-box').hidden = role !== 'member' || !!s.left;
  for (const r of ['member', 'guest', 'waiting']) $(`lv-as-${r}`).hidden = role !== r;
  $('lv-left').hidden = !s.left;
  if (s.left) {
    $('lv-since').textContent = day(s.left);
    $('lv-until').textContent = day(s.until);
    // the owner's removal is undone by the owner, not here
    $('lv-back').hidden = s.removed;
    $('lv-removed').hidden = !s.removed;
  }
  if (!s.passkey) {
    $('lv-ask-how').hidden = true;
    $('lv-ask-yes').textContent = 'Leave';
  }
  // a root on a device signs only through the app that holds it, and a
  // passkey only on the website, where it was made
  const elsewhere = s.passkey ? byApp : !byApp;
  if (elsewhere) {
    $('lv-go').hidden = true;
    $('lv-back').hidden = true;
    $(s.passkey ? 'lv-passkey' : 'lv-device').hidden = false;
  }
  if (role === 'member' && !s.left) take();
}

async function act(back, button) {
  button.disabled = true;
  const msg = $(back ? 'lv-msg' : 'lv-ask-msg');
  msg.textContent = '';
  try {
    await say(back);
    if (back) {
      location.href = '/_dd/settings';
      return;
    }
    // this browser is signed out now: what it shows is the last it knew
    const t = Math.floor(Date.now() / 1000);
    $('lv-ask').close();
    show({ ...state, left: t, until: t + state.window });
    $('lv-back').hidden = true;
    $('lv-left-h').focus();
  } catch (e) {
    button.disabled = false;
    msg.textContent = e.name === 'NotAllowedError' ? 'Your passkey was not used.' : String(e.message || e);
  }
}

// ---- taking everything out

const count = (n) => n.toLocaleString();

function left(secs) {
  if (!Number.isFinite(secs) || secs <= 0) return '';
  if (secs < 90) return ' · under 2 min left';
  if (secs < 5400) return ` · about ${Math.round(secs / 60)} min left`;
  return ` · about ${(secs / 3600).toFixed(1)} h left`;
}

/// One download's row: its button, its Cancel, its bar and its words.
/// What changes every second is shown; a screen reader is told each tenth
/// of the way, and the end.
function row(name) {
  const go = $(`lv-${name}-go`);
  const stop = $(`lv-${name}-stop`);
  const bar = $(`lv-${name}-bar`);
  const status = $(`lv-${name}-status`);
  const said = $(`lv-${name}-said`);
  let tenth = -1;
  let since = 0;
  return {
    go,
    stop,
    begin(words) {
      go.hidden = true;
      stop.hidden = false;
      stop.disabled = false;
      stop.focus();
      $(`lv-${name}-progress`).hidden = false;
      bar.removeAttribute('value');
      status.className = 'lv-status';
      status.textContent = words;
      said.textContent = words;
      tenth = -1;
      since = performance.now();
    },
    /// files and bytes so far, of how many if known
    step(files, total, bytes, totalBytes) {
      const frac = totalBytes ? bytes / totalBytes : total ? files / total : null;
      if (frac === null) bar.removeAttribute('value');
      else bar.value = Math.min(frac, 1);
      const secs = (performance.now() - since) / 1000;
      const eta = frac && secs > 5 ? left(secs * (1 - frac) / frac) : '';
      const what = total ? `${count(files)} of ${count(total)} files` : `${count(files)} files`;
      const size = totalBytes ? `${human(bytes)} of ${human(totalBytes)}` : human(bytes);
      status.textContent = `${what} · ${size}${eta}`;
      const t = frac === null ? -1 : Math.floor(frac * 10);
      if (t > tenth) {
        tenth = t;
        said.textContent = `${t * 10}% downloaded`;
      }
    },
    end(words, bad) {
      go.hidden = false;
      stop.hidden = true;
      if (bad) bar.removeAttribute('value');
      else bar.value = 1;
      status.className = bad ? 'lv-status bad' : 'lv-status';
      status.textContent = words;
      said.textContent = words;
      go.disabled = false;
      go.focus();
    },
  };
}

const android = /Android/.test(navigator.userAgent);
// through the app: a browser it let in, or its window where a request
// carries a body
const relayed = servedByApp || (inApp && !android);

/// Where a zip made in this page goes, a piece at a time: through the app
/// (to a download in a browser it let in, or to the Downloads folder from
/// its window); else to a file the person picks; else into a Blob of
/// pieces. The app's window on Android takes no body on a request, so
/// there the page keeps the pieces itself.
async function sinkFor(kind) {
  if (relayed) {
    const r = await ask('/_dd/app/export', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ kind }),
    });
    if (!r.ok) throw new Error(await r.text());
    const { id, name } = await r.json();
    const q = `?id=${encodeURIComponent(id)}`;
    if (servedByApp) {
      const a = document.createElement('a');
      a.href = `/_dd/app/export/zip${q}`;
      a.download = name;
      document.body.append(a);
      a.click();
      a.remove();
    }
    const send = async (path, body) => {
      const r = await ask(`/_dd/app/export/${path}${q}`, { method: 'POST', body });
      if (!r.ok) throw new Error(r.status === 410 ? `the download stopped: ${await r.text()}` : await r.text());
      return r;
    };
    return {
      write: (bytes) => send('chunk', bytes),
      async close() {
        const s = await (await send('end')).json();
        return s.saved;
      },
      abort: () => send('cancel').catch(() => {}),
    };
  }
  if (window.showSaveFilePicker) {
    const w = await (await window.showSaveFilePicker({ suggestedName: `commonty-${kind}.zip` })).createWritable();
    return { write: (b) => w.write(b), close: () => w.close(), abort: () => w.abort().catch(() => {}) };
  }
  const kept = [];
  return {
    write: async (b) => { kept.push(new Blob([b])); },
    async close() { save(new Blob(kept, { type: 'application/zip' }), `commonty-${kind}.zip`); },
    async abort() { kept.length = 0; },
  };
}

/// pieces go out in batches of about 8 MiB: fewer requests to the app
function batched(sink) {
  const MAX = 8 << 20;
  let parts = [];
  let size = 0;
  const flush = async () => {
    if (!size) return;
    const all = new Uint8Array(size);
    let at = 0;
    for (const p of parts) { all.set(p, at); at += p.length; }
    parts = [];
    size = 0;
    await sink.write(all);
  };
  return {
    async write(bytes) {
      if (bytes.length >= MAX) {
        await flush();
        await sink.write(bytes);
        return;
      }
      parts.push(bytes);
      size += bytes.length;
      if (size >= MAX) await flush();
    },
    flush,
  };
}

const when = (s) => (s ? new Date(s) : new Date());

/// Files & Movies: the whole library as one zip, folders and names as they
/// are, opened here with the key the app hands over
async function filesZip(ui) {
  const stop = new AbortController();
  let sink = null;
  ui.stop.onclick = () => {
    ui.stop.disabled = true;
    stop.abort();
  };
  ui.begin('Counting your files…');
  try {
    const got = await unlock();
    if (got.none) {
      ui.end('There is no library to take: nothing to download.');
      return;
    }
    if (!got.ok) throw new Error('your library opens in the Commonty app');
    const lib = got.ok;
    // a file picked by the person is asked for while the click still counts
    if (!relayed) sink = await sinkFor('files');
    const all = await walk(lib, '', (n) => {
      if (stop.signal.aborted) throw new DOMException('stopped', 'AbortError');
      $('lv-files-status').textContent = `Counting your files… ${count(n)} so far`;
    });
    const files = all.filter((e) => !e.dir);
    const totalBytes = files.reduce((n, e) => n + e.size, 0);
    sink ??= await sinkFor('files');
    const out = batched(sink);
    const zip = new ZipWriter((b) => out.write(b));
    let done = 0;
    let bytes = 0;
    let shown = 0;
    const tick = () => {
      const now = performance.now();
      if (now - shown < 500) return;
      shown = now;
      ui.step(done, files.length, bytes, totalBytes);
    };
    ui.step(0, files.length, 0, totalBytes);
    async function* counted(item) {
      for await (const p of pieces(lib, item, stop.signal)) {
        bytes += p.length;
        tick();
        yield p;
      }
    }
    for (const e of all) {
      if (stop.signal.aborted) throw new DOMException('stopped', 'AbortError');
      if (e.dir) await zip.folder(e.path, when(e.modified));
      else {
        await zip.file(e.path, when(e.modified), counted(e));
        done++;
        tick();
      }
    }
    await zip.end();
    await out.flush();
    const saved = await sink.close();
    ui.step(done, files.length, bytes, totalBytes);
    ui.end(saved
      ? `Done: ${count(done)} files, ${human(bytes)}, saved in your Downloads folder (${saved.split(/[\\/]/).pop()}).`
      : `Done: ${count(done)} files, ${human(bytes)}.`);
  } catch (e) {
    await sink?.abort();
    if (stop.signal.aborted || e.name === 'AbortError') ui.end('Cancelled. Nothing was kept.', true);
    else ui.end(`Could not download your files: ${e.message || e}`, true);
  }
}

/// Photos: the app signs in with the password it holds, and makes the zip
/// itself, a folder for each album; this page only follows it
async function photosZip(ui) {
  let id = null;
  let stopped = false;
  ui.stop.onclick = async () => {
    ui.stop.disabled = true;
    stopped = true;
    if (id) await ask(`/_dd/app/export/cancel?id=${encodeURIComponent(id)}`, { method: 'POST' }).catch(() => {});
  };
  ui.begin('Opening your photos…');
  try {
    const r = await ask('/_dd/app/export', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ kind: 'photos' }),
    });
    if (!r.ok) throw new Error(await r.text());
    const job = await r.json();
    id = job.id;
    const q = `?id=${encodeURIComponent(id)}`;
    if (servedByApp) {
      const a = document.createElement('a');
      a.href = `/_dd/app/export/zip${q}`;
      a.download = job.name;
      document.body.append(a);
      a.click();
      a.remove();
    }
    for (;;) {
      await new Promise((ok) => setTimeout(ok, 1000));
      const s = await (await ask(`/_dd/app/export/state${q}`)).json();
      if (s.total !== null && s.total !== undefined) ui.step(s.files, s.total, s.bytes, s.totalBytes);
      if (s.done) {
        ui.end(s.saved
          ? `Done: ${count(s.files)} photos and videos, ${human(s.bytes)}, saved in your Downloads folder (${s.saved.split(/[\\/]/).pop()}).`
          : `Done: ${count(s.files)} photos and videos, ${human(s.bytes)}.`);
        return;
      }
      if (s.cancelled || stopped) {
        ui.end('Cancelled. Nothing was kept.', true);
        return;
      }
      if (s.error) throw new Error(s.error);
    }
  } catch (e) {
    ui.end(`Could not download your photos: ${e.message || e}`, true);
  }
}

/// Passwords: the vault's own Export screen, where the person types their
/// master password, which no one but them ever holds
function vaultExport() {
  const to = '/#/tools/export';
  if (inApp) {
    location.href = `/_dd/app/vault?to=${encodeURIComponent(to)}`;
    return;
  }
  window.open(`/_dd/app/go?service=vault&path=${encodeURIComponent(to)}`, '_blank', 'noopener');
}

let taken = false;
function take() {
  if (taken) return;
  taken = true;
  if (!byApp) {
    $('lv-not-app').hidden = false;
    for (const id of ['lv-files-go', 'lv-photos-go', 'lv-vault']) $(id).disabled = true;
    $('lv-images-msg').textContent = 'From the Commonty app, or a browser you let in from it.';
    return;
  }
  const files = row('files');
  const photos = row('photos');
  files.go.onclick = () => filesZip(files);
  photos.go.onclick = () => photosZip(photos);
  $('lv-vault').onclick = vaultExport;
  images({ msg: $('lv-images-msg'), list: $('lv-images-list'), total: null, delete: false });
}

let wired = false;
export async function start() {
  $('lv-docs').href = docsUrl('/dev/leaving');
  if (!wired) {
    wired = true;
    $('lv-go').onclick = () => { $('lv-ask-msg').textContent = ''; $('lv-ask').showModal(); };
    $('lv-ask-no').onclick = () => $('lv-ask').close();
    $('lv-ask-yes').onclick = (e) => act(false, e.currentTarget);
    $('lv-back').onclick = (e) => act(true, e.currentTarget);
  }
  try {
    show(await where());
  } catch (e) {
    $('lv-msg').textContent = String(e.message || e);
  }
}
