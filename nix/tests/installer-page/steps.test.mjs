// The installer screen's script (nix/installer/web/setup.js) in a made-up
// page: each step asks its question on the way in, and the disk step shows
// the disks the installer gives it.
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
    querySelector() { return e.children[0]?.children?.[0]?.children?.[0] ?? element(); },
    closest() { return element(); },
    focus() {},
    requestSubmit() {},
  };
  return e;
}
const byId = new Map();
const bySel = new Map();
const document = {
  getElementById: (id) => byId.get(id) ?? byId.set(id, element()).get(id),
  querySelector: (s) => bySel.get(s) ?? bySel.set(s, element()).get(s),
  createElement: () => element(),
};
const asked = [];
const answers = {
  checks: {
    checks: { uefi: true, tpm: true, setup_mode: true, raid: false, virt: true },
    missing: [],
    firmware: { change: [], already: ['power'] },
    maker: 'LENOVO', model: 'Test',
    drive: { expired: false },
  },
  network: { wired: true },
  disks: { disks: [{ name: '/dev/nvme0n1', size: 256060514304, small: false, model: 'SSD', used: 0, parts: [] }] },
};
const fetch = async (url) => {
  const path = url.replace('/api/', '');
  asked.push(path);
  return { ok: true, json: async () => answers[path] };
};
const ctx = vm.createContext({
  document, fetch, setTimeout,
  location: { hash: '#t=' + 'a'.repeat(64), pathname: '/' },
  history: { replaceState() {} },
  sessionStorage: { setItem() {}, getItem: () => 'a'.repeat(64) },
});
const settle = () => new Promise((r) => setTimeout(r, 20));
vm.runInContext(readFileSync(process.argv[2], 'utf8'), ctx);
await settle();
assert.deepEqual(asked, ['checks']);
assert.equal(document.getElementById('next').disabled, false, 'the firmware step is satisfied');

document.getElementById('next').onclick();
await settle();
assert.deepEqual(asked, ['checks', 'network']);

document.getElementById('next').onclick();
await settle();
assert.deepEqual(asked, ['checks', 'network', 'disks'], 'the disk step asks for the disks');
const list = document.getElementById('disk-list').children;
assert.equal(list.length, 1, 'one disk listed, not the placeholder');
assert.equal(list[0].className, 'hi-disk');
assert.equal(document.getElementById('next').disabled, false, 'the only disk is chosen');
console.log('installer page: every step asks on the way in');
