// Devices: the keys that are you. Your entry names them; this page only
// reads it, because adding or removing one is a signature your root key
// makes, and your root key is not in a browser.

import { ready, failed, section, when } from './panel.js';

import { me } from './shell.js';
const { user } = await me();

async function start() {
  const e = (await (await fetch('/_dd/directory/' + encodeURIComponent(user))).json()).entry;
  const body = ready();
  section(body, 'Devices', (e.devices || []).map((d) => [
    d.fingerprint.slice(0, 16),
    `added ${when(d.added)}`,
  ]), 'No device holds a key for this account.');
  section(body, 'Passkeys', (e.passkeys || []).map((p) => [
    p.id.slice(0, 16),
    p.library_key ? 'opens your library' : 'signs in only',
  ]), 'No passkey is enrolled; `dd enrol` adds one.');
  section(body, 'Recovery', e.recovery ? [['A recovery key is set', 'it can install a new root']] : [],
    'No recovery key. Without one, losing every device loses the account.');
}

start().catch(failed);
