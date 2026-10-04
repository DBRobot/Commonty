// What every page shares: finding and making elements, and getting data the
// fast way - what was shown before at once, the fresh answer when it lands
// (docs/ui.md). A page imports what it uses; nothing here draws a page.

export const $ = (id) => document.getElementById(id);

/// An element, its properties and its children. `class` and `text` are
/// what they say, `on…` takes a handler, anything else is an attribute; a
/// property or child that is null, undefined or false is left out, and
/// arrays of children are flattened.
export function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else if (k.startsWith('on') && typeof v === 'function') e.addEventListener(k.slice(2), v);
    else e.setAttribute(k, v === true ? '' : v);
  }
  for (const c of kids.flat(Infinity)) if (c !== null && c !== undefined && c !== false) e.append(c);
  return e;
}

// what this browser kept from earlier visits; a page works without it
const store = (() => {
  try { return window.localStorage; } catch { return null; }
})();
function kept(key) {
  try { return JSON.parse(store?.getItem(key) || 'null'); } catch { return null; }
}
function keep(key, value) {
  try { store?.setItem(key, JSON.stringify(value)); } catch { /* full or refused: fetched again next time */ }
}

// one request per address in flight, however many ask
const inflight = new Map();
function fetchOnce(url, as) {
  const id = `${as} ${url}`;
  if (!inflight.has(id)) {
    inflight.set(id, fetch(url).then((r) => {
      if (!r.ok) throw new Error(`the box said ${r.status}`);
      return as === 'text' ? r.text() : r.json();
    }).finally(() => inflight.delete(id)));
  }
  return inflight.get(id);
}

/// Cache first, then fresh: `draw(value)` runs at once with what this
/// browser kept from last time, if anything, and again when the fresh
/// answer differs from it. `source` is an address, or an async function for
/// anything else that answers (the app's own commands) - then `key` names
/// it. Resolves to the fresh value; rejects when there was no answer, and
/// `draw` has then had only the kept one. `as`: 'json' (the default) or
/// 'text', for an address.
export function load(source, draw, { as = 'json', key } = {}) {
  const k = key || `kit:${source}`;
  const old = kept(k);
  if (old !== null) draw(old);
  const fresh = typeof source === 'function' ? source() : fetchOnce(source, as);
  return fresh.then((value) => {
    if (JSON.stringify(value) !== JSON.stringify(old)) {
      keep(k, value);
      draw(value);
    }
    return value;
  });
}

/// The same request, asked early - when the pointer reaches a link - so
/// the click finds it answered. Nothing is drawn; `load` draws.
export function prefetch(url, { as = 'json', key = `kit:${url}` } = {}) {
  fetchOnce(url, as).then((fresh) => keep(key, fresh)).catch(() => {});
}
