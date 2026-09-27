// Network: the machines you have on the fleet's own network, as the
// control server has them.

import { ready, failed, section, ago } from './panel.js';

async function start() {
  const r = await fetch('/_dd/network/mine');
  const d = await r.json();
  const body = ready();
  if (!r.ok) throw new Error(d.error || `the box said ${r.status}`);
  if (!d.here) {
    section(body, 'Your machines', [], 'The control server is not on this box. Open this page on the box that runs it.');
    return;
  }
  section(body, 'Your machines', (d.machines || []).map((m) => [
    m.name || '—',
    (m.addresses || []).join(' '),
    m.online ? 'online' : `last seen ${ago(m.lastSeen ? Date.parse(m.lastSeen) / 1000 : 0)}`,
    m.os || '',
  ].filter(Boolean)), 'No machine of yours is on the network yet. `dd net join` puts one on it.');
}

start().catch(failed);
