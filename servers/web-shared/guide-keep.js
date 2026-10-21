// Monty's conversation, kept for this tab while it moves between the
// site's names (home., git., the docs on the bare name...). Each name has
// its own storage, so the assistant on each page keeps it here, in a frame
// of home.<domain> it never shows: this tab's storage of one name, in the
// browser and nowhere else. Only the pages the assistant runs on may ask:
// the names the model answers (nix/lib/guide-pages.nix) and Chat's (llm.);
// the frame's frame-ancestors says so too (modules/gate/verify.nix).

const base = location.hostname.replace(/^home\./, '');
const ours = new RegExp(`^https://((demo-)?(home|files|git|games|metrics|llm)\\.)?${base.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}$`);
const KEY = 'dd-guide-carry';
const MOST = 256 * 1024;

addEventListener('message', (e) => {
  if (e.source !== parent || !ours.test(e.origin)) return;
  const d = e.data;
  if (!d || typeof d !== 'object') return;
  const id = Number.isInteger(d.id) ? d.id : null;
  let v = null;
  try {
    if (d.op === 'get') v = sessionStorage.getItem(KEY);
    else if (d.op === 'put' && typeof d.v === 'string' && d.v.length <= MOST) sessionStorage.setItem(KEY, d.v);
    else if (d.op === 'clear') sessionStorage.removeItem(KEY);
    else return;
  } catch { /* refused: nothing kept */ }
  parent.postMessage({ op: d.op === 'get' ? 'got' : 'kept', id, v }, e.origin);
});
