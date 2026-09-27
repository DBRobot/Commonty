// Boxes: what each one is running and when it last heard from the
// release. Every answer comes from that box's own prometheus.

import { ready, failed, section, ago, human } from './panel.js';

async function start() {
  const boxes = await (await fetch('/_dd/fleet.json')).json();
  const body = ready();
  section(body, 'The fleet', boxes.map((b) => [
    b.name,
    b.up ? `release ${b.release ?? '—'}` : 'no answer',
    b.up ? `last run ${b.result ?? '?'} ${ago(b.last_run)}` : '',
    b.model ? `${b.model} · ${b.cores ?? '?'} cores · ${b.memory ? human(b.memory) : '?'}` : '',
    b.kernel ? `linux ${b.kernel}` : '',
    // the box's own backup: fleet health, not somebody's archived disk
    b.backup ? `backed up ${ago(b.backup.last_success)}` : '',
  ].filter(Boolean)), 'No boxes are declared here.');
}

start().catch(failed);
