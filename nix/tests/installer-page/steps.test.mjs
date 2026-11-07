// The installer screen's script (nix/installer/web/setup.js) in a made-up
// page: each step asks its question on the way in, the disk step shows the
// disks the installer gives it, and the drive's Wi-Fi joins on its own.
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

const settle = () => new Promise((r) => setTimeout(r, 20));

// a fresh page; `network` is what the installer says of its network, and
// `driveJoins` whether the drive's Wi-Fi connects when asked
async function page({ network, driveJoins = true }) {
  const byId = new Map();
  const bySel = new Map();
  const document = {
    getElementById: (id) => byId.get(id) ?? byId.set(id, element()).get(id),
    querySelector: (s) => bySel.get(s) ?? bySel.set(s, element()).get(s),
    createElement: () => element(),
  };
  const asked = [];
  const posted = [];
  const answers = {
    checks: {
      checks: { uefi: true, tpm: true, setup_mode: true, raid: false, virt: true },
      missing: [],
      firmware: { change: [], already: ['power'] },
      maker: 'LENOVO', model: 'Test',
      drive: { expired: false },
    },
    network,
    wifi: { networks: [{ ssid: 'Cafe', signal: 80 }] },
    disks: { disks: [{ name: '/dev/nvme0n1', size: 256060514304, small: false, model: 'SSD', used: 0, parts: [] }] },
  };
  const fetch = async (url, opts) => {
    const path = url.replace('/api/', '');
    asked.push(path);
    if (opts?.method === 'POST') {
      posted.push([path, JSON.parse(opts.body)]);
      if (!driveJoins) return { ok: false, json: async () => ({ error: 'check the password' }) };
      answers.network = { wired: false, wifi: 'House Wi-Fi' };
      return { ok: true, json: async () => ({ joined: 'House Wi-Fi' }) };
    }
    return { ok: true, json: async () => answers[path] };
  };
  const ctx = vm.createContext({
    document, fetch, setTimeout,
    location: { hash: '#t=' + 'a'.repeat(64), pathname: '/' },
    history: { replaceState() {} },
    sessionStorage: { setItem() {}, getItem: () => 'a'.repeat(64) },
  });
  vm.runInContext(readFileSync(process.argv[2], 'utf8'), ctx);
  await settle();
  return { document, asked, posted };
}

// the cable is in: every step asks on the way in
{
  const { document, asked, posted } = await page({ network: { wired: true } });
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
  assert.deepEqual(posted, [], 'a cable needs no Wi-Fi');
}

// no cable, and the drive carries the Wi-Fi: it joins on its own, once, and
// the step is satisfied without a list or a password
{
  const { document, posted } = await page({ network: { wired: false, wifi: null, drive: 'House Wi-Fi' } });
  document.getElementById('next').onclick();
  await settle();
  assert.deepEqual(posted, [['wifi', { drive: true }]], 'the drive\'s Wi-Fi was joined without a click');
  assert.equal(document.getElementById('net-list').hidden, true, 'no list of networks to pick from');
  assert.equal(document.getElementById('next').disabled, false, 'the network step is satisfied');
}

// the drive's Wi-Fi will not connect: tried once, not in a loop, and the
// picker is shown with the reason
{
  const { document, posted } = await page({
    network: { wired: false, wifi: null, drive: 'Other Wi-Fi' },
    driveJoins: false,
  });
  document.getElementById('next').onclick();
  await settle();
  assert.equal(posted.length, 1, 'tried once, not in a loop');
  assert.equal(document.getElementById('net-list').hidden, false, 'the picker is shown');
  assert.match(document.getElementById('net-lead').textContent, /would not connect/);
  assert.equal(document.getElementById('next').disabled, true, 'not satisfied until it is on a network');
}

// no cable and nothing on the drive: the picker, and nothing is tried
{
  const { document, posted } = await page({ network: { wired: false, wifi: null } });
  document.getElementById('next').onclick();
  await settle();
  assert.deepEqual(posted, []);
  assert.equal(document.getElementById('net-list').hidden, false);
}
console.log('installer page: every step asks on the way in, the drive\'s Wi-Fi joins itself');
