// Reading a scanned page's words, in the tab: Tesseract (WebAssembly, in a
// worker), its English model and its code pinned beside our own wasm in
// flake.nix. Loaded the first time Recognise text is used, and kept for
// the next page. The page's picture never leaves the tab.

let made = null;

async function worker(progress) {
  if (!made) {
    made = (async () => {
      const { createWorker } = (await import('/_dd/web/tesseract.esm.min.js')).default;
      return createWorker('eng', 1, {
        workerPath: '/_dd/web/tesseract-worker.min.js',
        // one build, for every browser with WebAssembly SIMD (all current ones)
        corePath: '/_dd/web/tesseract-core-simd-lstm.wasm.js',
        langPath: '/_dd/web',
        // a worker from our own address, as the content policy asks; not a blob
        workerBlobURL: false,
        gzip: true,
        logger: (m) => made.progress?.(m),
      });
    })();
    made.catch(() => { made = null; });
  }
  made.progress = progress;
  return made;
}

/// The words on a page drawn at `scale` into `canvas`, in page points:
/// [{ text, bbox: [x0, y0, x1, y1], confidence }]. progress(0..1, what).
export async function recognise(canvas, scale, progress) {
  const w = await worker((m) => {
    if (m.status === 'recognizing text') progress?.(0.3 + 0.7 * m.progress, 'Reading the words');
    else progress?.(0.3 * (m.progress || 0), 'Getting the reader ready');
  });
  const { data } = await w.recognize(canvas, {}, { blocks: true, text: false });
  const out = [];
  for (const b of data.blocks || []) for (const p of b.paragraphs) for (const l of p.lines) for (const x of l.words) {
    if (!x.text.trim() || x.confidence < 30) continue;
    out.push({ text: x.text, confidence: x.confidence, bbox: [x.bbox.x0 / scale, x.bbox.y0 / scale, x.bbox.x1 / scale, x.bbox.y1 / scale] });
  }
  return out;
}

export async function stop() {
  if (!made) return;
  const w = await made.catch(() => null);
  made = null;
  await w?.terminate();
}
