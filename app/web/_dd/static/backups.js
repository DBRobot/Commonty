// Backups: the disks people have put here. `dd image` writes an archive
// of an old computer into their own folder on the box, in restic's
// format, encrypted with a password that never leaves their machine. So
// this page can say an archive is here and when it last grew, and cannot
// say what is in it - which is the point of it.

import { ready, failed, section, ago, when } from './panel.js';

const IMAGES = 'https://files.' + location.hostname.split('.').slice(-2).join('.') + '/images/';

async function listing(at) {
  const r = await fetch(at, { credentials: 'include' });
  if (r.status === 404) return [];
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  return r.json();
}

/// restic's own shape: a repository is these, and nothing else is
const RESTIC = ['config', 'data', 'index', 'keys', 'snapshots'];

function stamp(entries, name) {
  const e = entries.find((x) => x.name === name);
  return e ? Math.round(Date.parse(e.mtime) / 1000) : 0;
}

async function start() {
  const body = ready();
  const top = await listing(IMAGES);
  const isRepo = RESTIC.every((n) => top.some((e) => e.name === n));

  if (isRepo) {
    // one archive, written straight into this person's folder
    const grew = stamp(top, 'data') || stamp(top, 'snapshots');
    const made = stamp(top, 'keys') || stamp(top, 'config');
    section(body, 'Your archived disks', [
      ['An archive is here', `last written ${ago(grew)}`, when(grew)],
      ['First written', when(made)],
    ], '');
  } else {
    const folders = top.filter((e) => e.type === 'directory');
    section(body, 'Your archived disks', folders.map((f) => [
      f.name,
      `last written ${ago(Math.round(Date.parse(f.mtime) / 1000))}`,
    ]), 'Nothing here yet. `dd image` puts a disk in.');
  }

  const p = document.createElement('p');
  p.className = 'note';
  p.innerHTML = 'What is in these is encrypted with a password that never left the machine that made them, so this box cannot list it and neither can this page. <code>dd image list</code> on that machine can.';
  body.append(p);
}

start().catch(failed);
