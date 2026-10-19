// Photos from Mail: a picture or film in an email saved into Photos, photos
// attached to an email, and a link to some of them put in one. The app
// hands this page Photos' password (/_dd/app/keys) and the account opens in
// the tab (our Rust, dd_web); museum and the store are asked by way of this
// page's own origin, which the app passes on (app/src/keys.rs). Photos'
// master key is in this tab and nowhere else.

import init, { photos_open, photos_list, photos_thumbnail, photos_original, photos_save, photos_share } from '/_dd/web/dd_web.js';
import { appFetch } from './shell.js';
import { el } from './kit.js';

// where something saved from Mail goes in Photos
const ALBUM = 'From Mail';

const post = async (path, body, viaApp) => {
  const init = { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body || {}) };
  const r = await (viaApp ? appFetch(path, init) : fetch(path, init));
  if (!r.ok) throw new Error((await r.text()) || `${path}: ${r.status}`);
  return r.json();
};

let opening = null;
function open() {
  opening ??= (async () => {
    await init();
    const k = await post('/_dd/app/keys', { services: ['photos'] }, true);
    if (k.photos?.error) throw new Error(k.photos.error);
    const cfg = await post('/_dd/photos/config');
    const here = location.origin;
    return photos_open(`${here}/_dd/ente`, `${here}/_dd/ente-store`, cfg.email, k.photos.pw);
  })().catch((e) => { opening = null; throw e; });
  return opening;
}

// The small picture Photos shows in its grid, drawn here as Ente's apps do
// (a JPEG no wider or taller than 720): from the picture, or the film's
// first frame. One Photos can show something in comes back if neither draws.
async function thumbnail(bytes, type) {
  const blob = new Blob([bytes], { type });
  let source = null;
  let w = 0;
  let h = 0;
  const url = URL.createObjectURL(blob);
  try {
    if (type.startsWith('video/')) {
      const v = document.createElement('video');
      v.muted = true;
      v.preload = 'auto';
      v.src = url;
      await new Promise((ok, no) => { v.onloadeddata = ok; v.onerror = no; setTimeout(no, 8000); });
      source = v; w = v.videoWidth; h = v.videoHeight;
    } else {
      source = await createImageBitmap(blob);
      w = source.width; h = source.height;
    }
  } catch { source = null; }
  const scale = source && w && h ? Math.min(1, 720 / Math.max(w, h)) : 1;
  const c = document.createElement('canvas');
  c.width = source ? Math.max(1, Math.round(w * scale)) : 64;
  c.height = source ? Math.max(1, Math.round(h * scale)) : 64;
  const g = c.getContext('2d');
  if (source) g.drawImage(source, 0, 0, c.width, c.height);
  else { g.fillStyle = '#9aa3ad'; g.fillRect(0, 0, c.width, c.height); }
  URL.revokeObjectURL(url);
  const out = await new Promise((ok) => c.toBlob(ok, 'image/jpeg', 0.8));
  return new Uint8Array(await out.arrayBuffer());
}

/// Save one attachment into Photos, in its album From Mail; `when` is the
/// email's date, which Photos files it under
export async function saveToPhotos(name, type, bytes, when) {
  const h = await open();
  const kind = type.startsWith('video/') || /\.(mp4|mov|m4v|webm)$/i.test(name) ? 1 : 0;
  const thumb = await thumbnail(bytes, type || (kind ? 'video/mp4' : 'image/jpeg'));
  await photos_save(h, ALBUM, name, kind, when || Date.now(), bytes, thumb);
  return ALBUM;
}

let cssLoaded = false;
function css() {
  if (cssLoaded) return;
  cssLoaded = true;
  document.head.append(el('link', { rel: 'stylesheet', href: '/_dd/static/mail-photos.css' }));
}

// The member's photos in a dialog, newest first, to choose some of:
// resolves with the chosen ones and the handle, or null
async function choose(title, ok) {
  css();
  const h = await open();
  return new Promise((resolve) => {
    const d = el('dialog', { class: 'dd-photos', 'aria-labelledby': 'dd-photos-t' });
    const grid = el('div', { class: 'grid', role: 'listbox', 'aria-multiselectable': 'true' });
    const count = el('span', { class: 'count', role: 'status' });
    const go = el('button', { type: 'button', text: ok, disabled: '' });
    const picked = new Set();
    let all = [];
    let answer = null;
    const urls = [];
    const mark = () => {
      count.textContent = picked.size ? `${picked.size} chosen` : '';
      go.disabled = !picked.size;
    };
    d.append(
      el('h2', { id: 'dd-photos-t', text: title }),
      grid,
      el('div', { class: 'foot' }, count,
        el('button', { type: 'button', class: 'quiet', text: 'Cancel', onclick: () => d.close() }), go));
    go.onclick = () => { answer = { h, chosen: all.filter((p) => picked.has(p.id)) }; d.close(); };
    d.onclose = () => { for (const u of urls) URL.revokeObjectURL(u); d.remove(); resolve(answer); };
    document.body.append(d);
    d.showModal();
    grid.append(el('p', { class: 'note', text: 'Opening Photos…' }));
    photos_list(h).then(async (json) => {
      const { photos } = JSON.parse(json);
      all = photos;
      grid.replaceChildren();
      if (!photos.length) { grid.append(el('p', { class: 'note', text: 'Nothing in Photos yet.' })); return; }
      const tiles = photos.map((p) => {
        const img = el('img', { alt: p.title || 'A photo', loading: 'lazy' });
        const tile = el('button', { type: 'button', class: 'tile', role: 'option', 'aria-selected': 'false', title: p.title || '' }, img,
          ...(p.kind === 1 ? [el('span', { class: 'film', text: 'Film' })] : []));
        tile.onclick = () => {
          if (picked.has(p.id)) picked.delete(p.id); else picked.add(p.id);
          tile.setAttribute('aria-selected', String(picked.has(p.id)));
          mark();
        };
        grid.append(tile);
        return { p, img };
      });
      // the small pictures a few at a time, the ones at the top first
      let next = 0;
      const one = async () => {
        while (next < tiles.length && d.open) {
          const { p, img } = tiles[next++];
          try {
            const u = URL.createObjectURL(new Blob([await photos_thumbnail(h, p.id)], { type: 'image/jpeg' }));
            urls.push(u);
            img.src = u;
          } catch { img.classList.add('none'); }
        }
      };
      await Promise.all([one(), one(), one(), one()]);
    }).catch((e) => grid.replaceChildren(el('p', { class: 'note', role: 'alert', text: String(e.message || e) })));
  });
}

const TYPES = { jpg: 'image/jpeg', jpeg: 'image/jpeg', png: 'image/png', gif: 'image/gif', webp: 'image/webp', heic: 'image/heic', heif: 'image/heif', avif: 'image/avif', mp4: 'video/mp4', mov: 'video/quicktime', m4v: 'video/mp4', webm: 'video/webm', zip: 'application/zip' };

/// Photos to attach, each as `{name, type, bytes}`, up to `room` bytes in all
export async function attachFromPhotos(room) {
  const c = await choose('Attach from Photos', 'Attach');
  if (!c) return [];
  const out = [];
  let left = room;
  for (const p of c.chosen) {
    const id = p.id;
    const bytes = await photos_original(c.h, id, left);
    left -= bytes.length;
    // a live photo is kept as a zip of its picture and its film
    const name = p.kind === 2 && !/\.zip$/i.test(p.title) ? `${p.title}.zip` : p.title || `photo-${id}.jpg`;
    out.push({ name, type: TYPES[name.split('.').pop().toLowerCase()] || 'application/octet-stream', bytes });
  }
  return out;
}

/// A link anyone can open the chosen photos at, or null
export async function shareFromPhotos() {
  const c = await choose('Share photos by link', 'Make a link');
  if (!c) return null;
  const n = c.chosen.length;
  return photos_share(c.h, `Shared from Mail · ${n} ${n === 1 ? 'photo' : 'photos'}`, c.chosen.map((p) => p.id));
}
