// Scan to PDF, from Files: pages from the phone's camera or pictures on the
// device, each cropped and turned as it should be, then one PDF of them
// sealed into the folder open. "Make the words searchable" reads each page
// with Tesseract in the tab (pdf-ocr.js) and lays the words over it unseen.
// Loaded when Scan is pressed.

import { el } from './kit.js';
import { saveInto } from './pick.js';

let cssLoaded = false;
const MAX_PAGES = 40;

/// Open the scanner over the page; `into` is the library folder the PDF
/// goes in. done(path) once it is saved.
export function openScan({ lib, into, done }) {
  if (!cssLoaded) {
    cssLoaded = true;
    document.head.append(el('link', { rel: 'stylesheet', href: '/_dd/static/scan.css' }));
  }
  const pages = [];             // { canvas, crop: [x0, y0, x1, y1] in 0..1, turn }
  const d = el('dialog', { class: 'dd-scan', 'aria-labelledby': 'dd-scan-t' });
  const camera = el('input', { type: 'file', accept: 'image/*', capture: 'environment', multiple: true, class: 'sr-only' });
  const files = el('input', { type: 'file', accept: 'image/*', multiple: true, class: 'sr-only' });
  const strip = el('div', { class: 'strip', role: 'list', 'aria-label': 'Pages' });
  const stage = el('div', { class: 'stage' });
  const stamp = () => {
    const t = new Date();
    const p = (n) => String(n).padStart(2, '0');
    return `Scan ${t.getFullYear()}-${p(t.getMonth() + 1)}-${p(t.getDate())} ${p(t.getHours())}.${p(t.getMinutes())}`;
  };
  const name = el('input', { type: 'text', value: stamp(), 'aria-label': 'Name of the PDF' });
  const words = el('input', { type: 'checkbox', checked: true });
  const state = el('p', { class: 'state', role: 'status' });
  const saveBtn = el('button', { type: 'button', text: 'Save PDF', disabled: true });
  let at = -1;

  const add = async (list) => {
    for (const f of list) {
      // each page is kept as a picture until the PDF is made
      if (pages.length >= MAX_PAGES) { state.textContent = `A scan holds up to ${MAX_PAGES} pages. Save this one and start another.`; break; }
      const bmp = await createImageBitmap(f, { imageOrientation: 'from-image' });
      // big enough to read, small enough to keep a scan a few hundred KB a page
      const s = Math.min(1, 2200 / Math.max(bmp.width, bmp.height));
      const c = document.createElement('canvas');
      c.width = Math.round(bmp.width * s);
      c.height = Math.round(bmp.height * s);
      c.getContext('2d').drawImage(bmp, 0, 0, c.width, c.height);
      pages.push({ canvas: c, crop: [0, 0, 1, 1], turn: 0 });
    }
    at = pages.length - 1;
    draw();
  };
  camera.onchange = () => { add([...camera.files]); camera.value = ''; };
  files.onchange = () => { add([...files.files]); files.value = ''; };

  // the page as it will be: turned, then cropped
  const finished = (p) => {
    const turned = document.createElement('canvas');
    const side = p.turn % 180 !== 0;
    turned.width = side ? p.canvas.height : p.canvas.width;
    turned.height = side ? p.canvas.width : p.canvas.height;
    const x = turned.getContext('2d');
    x.translate(turned.width / 2, turned.height / 2);
    x.rotate((p.turn * Math.PI) / 180);
    x.drawImage(p.canvas, -p.canvas.width / 2, -p.canvas.height / 2);
    const [a, b, c2, e] = p.crop;
    const out = document.createElement('canvas');
    out.width = Math.max(1, Math.round((c2 - a) * turned.width));
    out.height = Math.max(1, Math.round((e - b) * turned.height));
    out.getContext('2d').drawImage(turned, a * turned.width, b * turned.height, out.width, out.height, 0, 0, out.width, out.height);
    return { turned, out };
  };

  function draw() {
    saveBtn.disabled = !pages.length;
    strip.replaceChildren(...pages.map((p, i) => {
      const { out } = finished(p);
      const thumb = el('button', { type: 'button', class: 'pg', role: 'listitem', 'aria-label': `Page ${i + 1}`, 'aria-current': String(i === at) },
        out, el('span', { text: String(i + 1) }));
      thumb.onclick = () => { at = i; draw(); };
      return thumb;
    }));
    stage.replaceChildren();
    if (at < 0) {
      stage.append(el('div', { class: 'empty' },
        el('p', { text: 'Take a picture of each page, or choose pictures from this device.' }),
        el('div', { class: 'row' },
          el('button', { type: 'button', text: 'Take a picture', onclick: () => camera.click() }),
          el('button', { type: 'button', class: 'quiet', text: 'Choose pictures', onclick: () => files.click() }))));
      return;
    }
    const p = pages[at];
    const { turned } = finished(p);
    // the dimmed part outside the crop stays inside the page's frame
    const view = el('div', { class: 'view' });
    const frame = el('div', { class: 'frame' });
    turned.className = 'img';
    const box = el('div', { class: 'crop', 'aria-label': 'What is kept of the page' });
    for (const h of ['nw', 'ne', 'sw', 'se']) box.append(el('span', { class: `h ${h}`, 'data-h': h }));
    const place = () => {
      const [a, b, c2, e] = p.crop;
      Object.assign(box.style, { left: `${a * 100}%`, top: `${b * 100}%`, width: `${(c2 - a) * 100}%`, height: `${(e - b) * 100}%` });
    };
    place();
    box.onpointerdown = (ev) => {
      const h = ev.target.dataset.h;
      const r = frame.getBoundingClientRect();
      const start = [...p.crop];
      const p0 = [(ev.clientX - r.left) / r.width, (ev.clientY - r.top) / r.height];
      box.setPointerCapture(ev.pointerId);
      const move = (e2) => {
        const q = [(e2.clientX - r.left) / r.width - p0[0], (e2.clientY - r.top) / r.height - p0[1]];
        const c = [...start];
        const clamp = (v) => Math.min(1, Math.max(0, v));
        if (!h) {
          const w = start[2] - start[0], hh = start[3] - start[1];
          c[0] = clamp(start[0] + q[0]); c[1] = clamp(start[1] + q[1]);
          c[0] = Math.min(c[0], 1 - w); c[1] = Math.min(c[1], 1 - hh);
          c[2] = c[0] + w; c[3] = c[1] + hh;
        } else {
          if (h.includes('w')) c[0] = clamp(Math.min(start[0] + q[0], start[2] - 0.05));
          if (h.includes('e')) c[2] = clamp(Math.max(start[2] + q[0], start[0] + 0.05));
          if (h.includes('n')) c[1] = clamp(Math.min(start[1] + q[1], start[3] - 0.05));
          if (h.includes('s')) c[3] = clamp(Math.max(start[3] + q[1], start[1] + 0.05));
        }
        p.crop = c;
        place();
      };
      const up = () => { box.removeEventListener('pointermove', move); box.removeEventListener('pointerup', up); draw(); };
      box.addEventListener('pointermove', move);
      box.addEventListener('pointerup', up);
      ev.preventDefault();
    };
    frame.append(turned, box);
    view.append(frame);
    stage.append(view, el('div', { class: 'row tools' },
      el('button', { type: 'button', class: 'quiet', text: '↺ Turn', onclick: () => { p.turn = (p.turn + 270) % 360; p.crop = [0, 0, 1, 1]; draw(); } }),
      el('button', { type: 'button', class: 'quiet', text: '↻ Turn', onclick: () => { p.turn = (p.turn + 90) % 360; p.crop = [0, 0, 1, 1]; draw(); } }),
      el('button', { type: 'button', class: 'quiet', text: 'Whole page', onclick: () => { p.crop = [0, 0, 1, 1]; draw(); } }),
      el('button', { type: 'button', class: 'quiet', text: 'Earlier', disabled: at === 0, onclick: () => { pages.splice(at - 1, 0, pages.splice(at, 1)[0]); at--; draw(); } }),
      el('button', { type: 'button', class: 'quiet', text: 'Later', disabled: at === pages.length - 1, onclick: () => { pages.splice(at + 1, 0, pages.splice(at, 1)[0]); at++; draw(); } }),
      el('button', { type: 'button', class: 'quiet danger', text: 'Remove', onclick: () => { pages.splice(at, 1); at = Math.min(at, pages.length - 1); draw(); } })));
  }

  saveBtn.onclick = async () => {
    saveBtn.disabled = true;
    try {
      const { Pdf } = await import('./pdf-doc.js');
      const shots = pages.map((p) => finished(p).out);
      state.textContent = 'Making the PDF…';
      const images = [];
      for (const c of shots) {
        const blob = await new Promise((ok) => c.toBlob(ok, 'image/jpeg', 0.82));
        images.push({ bytes: new Uint8Array(await blob.arrayBuffer()), width: c.width, height: c.height });
      }
      const pdf = await Pdf.fromImages(images);
      if (words.checked) {
        const { recognise } = await import('./pdf-ocr.js');
        for (let i = 0; i < shots.length; i++) {
          state.textContent = `Reading the words on page ${i + 1} of ${shots.length}…`;
          const scale = shots[i].width / 595.28;
          const found = await recognise(shots[i], scale);
          if (found.length) await pdf.addHiddenText(i, found);
        }
      }
      state.textContent = 'Locking and saving…';
      const file = `${(name.value.trim() || stamp()).replace(/\.pdf$/i, '')}.pdf`;
      const path = await saveInto(lib, into, file, pdf.save());
      pdf.destroy();
      d.close();
      done?.(path);
    } catch (e) {
      state.textContent = `Not saved: ${e.message}`;
      saveBtn.disabled = false;
    }
  };

  d.append(
    el('div', { class: 'head' }, el('h2', { id: 'dd-scan-t', text: 'Scan to PDF' }),
      el('button', { type: 'button', class: 'quiet', text: '+ Page', onclick: () => camera.click(), 'aria-label': 'Add a page from the camera' }),
      el('button', { type: 'button', class: 'quiet', text: 'From pictures', onclick: () => files.click() })),
    el('div', { class: 'body' }, stage, strip),
    el('div', { class: 'foot' },
      el('label', { class: 'nm' }, el('span', { text: 'Name' }), name),
      el('label', { class: 'chk' }, words, el('span', { text: 'Make the words searchable' })),
      state,
      el('div', { class: 'row' }, el('button', { type: 'button', class: 'quiet', text: 'Cancel', onclick: () => d.close() }), saveBtn)),
    camera, files);
  d.onclose = () => d.remove();
  document.body.append(d);
  d.showModal();
  draw();
}
