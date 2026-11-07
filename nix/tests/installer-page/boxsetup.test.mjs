// The app's "set up a box" page (app/web/boxsetup.js) in a made-up page: a
// drive is written again for a second box. The app keeps the last run's
// `written` until it restarts, and the page once took that for this run's
// and sent no write at all: the button did nothing, but once.
import { readFileSync } from 'node:fs';
import vm from 'node:vm';
import assert from 'node:assert/strict';

function element() {
  const e = {
    hidden: false, disabled: false, checked: false, value: '', textContent: '',
    children: [], dataset: {}, style: {}, attrs: {},
    classList: { toggle() {}, add() {}, remove() {} },
    replaceChildren(...c) { e.children = c; },
    append(...c) { e.children.push(...c); },
    setAttribute(k, v) { e.attrs[k] = v; },
    removeAttribute(k) { delete e.attrs[k]; },
  };
  return e;
}
const settle = () => new Promise((r) => setTimeout(r, 20));

// a fresh page, with the app already at `stage` from before
async function page(stage) {
  const byId = new Map();
  const get = (id) => byId.get(id) ?? byId.set(id, element()).get(id);
  const document = {
    getElementById: get,
    querySelector: (s) => (s.startsWith('meta') ? { content: 'secret' } : element()),
    querySelectorAll: () => [],
  };
  const posted = [];
  const fetch = async (url, init = {}) => {
    const path = url.replace('/_dd/app/boxsetup', '');
    if (init.method === 'POST') {
      posted.push([path, JSON.parse(init.body)]);
      return { ok: true, status: 202, text: async () => '', json: async () => ({}) };
    }
    if (path === '/progress') return { ok: true, json: async () => ({ stage }) };
    if (path === '/disks') return { ok: true, json: async () => [] };
    return { ok: true, json: async () => ({}) };
  };
  // the page's one import is the kit: $ is getElementById, el builds an element
  const src = readFileSync(process.argv[2], 'utf8')
    .replace(/^import .*kit\.js';$/m, 'const { $, el } = kit;');
  const ctx = vm.createContext({
    document, fetch, kit: { $: get, el: () => element() },
    setInterval: () => 0, clearInterval() {}, setTimeout,
  });
  vm.runInContext(src, ctx);
  await settle();
  vm.runInContext('chosen = { id: "usb1", name: "/dev/sda" }; wifi = "none";', ctx);
  vm.runInContext('write()', ctx);
  await settle();
  return posted;
}

// the first box: nothing has been written yet
assert.equal((await page(undefined)).length, 1, 'a fresh app writes');

// the second box: the app still says the last drive was written
const second = await page('written');
assert.equal(second.length, 1, 'a drive written before does not stop the next write');
assert.equal(second[0][0], '/write');
assert.equal(second[0][1].device, 'usb1');

// the last one failed: write again
assert.equal((await page('failed')).length, 1, 'a failed write is tried again');

// a write really running (the window was reloaded under it): joined, not doubled
for (const stage of ['manifest', 'download', 'write', 'key']) {
  assert.equal((await page(stage)).length, 0, `a running write (${stage}) is followed, not started again`);
}
console.log('box setup page: a second box is written, a running write is not doubled');
