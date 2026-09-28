// Metrics: how the boxes are doing. Every figure is a Prometheus query
// over the whole fleet, asked of Thanos through /api/v1 (nginx lets these
// few read-only calls through, behind the same sign-in as everything
// else). The fleet, a box, what needs attention, storage and backups, and
// a place to ask anything else.

import './shell.js';

const $ = (id) => document.getElementById(id);
function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props || {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else if (k.startsWith('on')) e[k] = v;
    else e.setAttribute(k, v === true ? '' : v);
  }
  for (const c of kids.flat(Infinity)) if (c !== null && c !== undefined && c !== false) e.append(c);
  return e;
}
const put = (parent, ...kids) => parent.replaceChildren(...kids.flat(Infinity).filter((k) => k !== null && k !== undefined && k !== false));

// ---- asking Prometheus

async function api(path, params) {
  const r = await fetch(`/api/v1/${path}?${new URLSearchParams(params)}`, { credentials: 'same-origin' });
  if (r.status === 401) { location.href = `/_dd/login?rd=${encodeURIComponent(location.pathname)}`; throw new Error('signed out'); }
  const body = await r.json().catch(() => ({}));
  if (!r.ok || body.status !== 'success') throw new Error(body.error || `the query failed (${r.status})`);
  return body.data;
}
const now = () => Math.floor(Date.now() / 1000);
const instant = (query) => api('query', { query }).then((d) => d.result);
const range = (query, secs, points = 120) => {
  const end = now(), step = Math.max(15, Math.round(secs / points));
  return api('query_range', { query, start: end - secs, end, step }).then((d) => d.result);
};
// one number per box
const byBox = (res, key = 'box') => Object.fromEntries(res.map((r) => [r.metric[key], Number(r.value[1])]));

// ---- words and numbers

const pct = (x) => `${Math.round(x)}`;
function bytes(n, per = '') {
  if (!Number.isFinite(n)) return '–';
  const u = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  while (n >= 1000 && i < u.length - 1) { n /= 1024; i++; }
  return `${n >= 100 || i === 0 ? n.toFixed(0) : n.toFixed(1)} ${u[i]}${per}`;
}
function age(secs) {
  if (!Number.isFinite(secs)) return '–';
  const m = secs / 60, h = m / 60, d = h / 24;
  return d >= 2 ? `${Math.floor(d)} days` : h >= 2 ? `${Math.floor(h)} h` : m >= 2 ? `${Math.floor(m)} min` : `${Math.floor(secs)} s`;
}
const ago = (t) => `${age(now() - t)} ago`;
const level = (v, warn, bad) => (v >= bad ? 'bad' : v >= warn ? 'busy' : 'ok');

// ---- the charts

const NS = 'http://www.w3.org/2000/svg';
function sparkline(points, color, max) {
  const s = document.createElementNS(NS, 'svg');
  s.setAttribute('class', 'spark');
  s.setAttribute('viewBox', '0 0 200 28');
  s.setAttribute('preserveAspectRatio', 'none');
  if (points.length > 1) {
    const t0 = points[0][0], t1 = points[points.length - 1][0] || t0 + 1;
    const d = points.map(([t, v], i) => `${i ? 'L' : 'M'}${(((t - t0) / (t1 - t0 || 1)) * 200).toFixed(1)},${(26 - (Math.min(v, max) / max) * 24).toFixed(1)}`).join(' ');
    s.innerHTML = `<path d="${d}" fill="none" stroke="${color}" stroke-width="1.6" vector-effect="non-scaling-stroke"/>`;
  }
  return s;
}

const COLORS = ['var(--c1)', 'var(--c2)', 'var(--c3)', 'var(--c4)', 'var(--c5)', 'var(--c6)'];
// lines over time: [{ name, color, points: [[t, v]] }]
function timeChart(lines, { unit = (v) => String(Math.round(v)), max, warn, secs }) {
  const W = 640, H = 220, L = 52, R = 12, T = 12, B = 24;
  const all = lines.flatMap((l) => l.points);
  const s = document.createElementNS(NS, 'svg');
  s.setAttribute('viewBox', `0 0 ${W} ${H}`);
  s.setAttribute('role', 'img');
  if (!all.length) { s.innerHTML = `<text x="${W / 2}" y="${H / 2}" text-anchor="middle" fill="var(--ink-3)" font-size="13">nothing in this time</text>`; return s; }
  const t1 = now(), t0 = t1 - secs;
  const nice = (v) => { const e = Math.pow(10, Math.floor(Math.log10(v))); const f = v / e; return (f <= 1 ? 1 : f <= 2 ? 2 : f <= 4 ? 4 : f <= 5 ? 5 : 8) * e; };
  const top = max ?? nice(Math.max(1e-9, ...all.map((p) => p[1])) * 1.05);
  const x = (t) => L + ((t - t0) / (t1 - t0)) * (W - L - R), y = (v) => T + (H - T - B) * (1 - Math.min(v, top) / top);
  let g = '';
  for (let k = 0; k <= 4; k++) { const v = (top / 4) * k; g += `<line x1="${L}" x2="${W - R}" y1="${y(v)}" y2="${y(v)}" stroke="var(--line-2)"/><text x="${L - 6}" y="${y(v) + 4}" font-size="11" text-anchor="end" fill="var(--ink-3)">${unit(v)}</text>`; }
  for (let k = 0; k <= 4; k++) {
    const t = t0 + ((t1 - t0) / 4) * k, d = new Date(t * 1000);
    const lab = secs > 3 * 86400 ? d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' }) : d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
    g += `<text x="${x(t)}" y="${H - 6}" font-size="11" text-anchor="${k === 0 ? 'start' : k === 4 ? 'end' : 'middle'}" fill="var(--ink-3)">${lab}</text>`;
  }
  if (warn) g += `<line x1="${L}" x2="${W - R}" y1="${y(warn.at)}" y2="${y(warn.at)}" stroke="var(--bad)" stroke-dasharray="4 4"/><text x="${W - R}" y="${y(warn.at) - 4}" font-size="10.5" text-anchor="end" fill="var(--bad)">${warn.label}</text>`;
  lines.forEach((l, k) => {
    // a gap in the data is a gap in the line, not a slope across it
    let d = '', prev = null;
    const gap = (secs / 120) * 3;
    for (const [t, v] of l.points) { d += `${prev === null || t - prev > gap ? 'M' : 'L'}${x(t).toFixed(1)},${y(v).toFixed(1)} `; prev = t; }
    if (k === 0 && lines.length <= 2) g += `<path d="${d.replace(/M/g, 'L').replace(/^L/, 'M')} L${x(l.points[l.points.length - 1][0])},${y(0)} L${x(l.points[0][0])},${y(0)} Z" fill="${l.color}" opacity=".07"/>`;
    g += `<path d="${d}" fill="none" stroke="${l.color}" stroke-width="1.8"/>`;
  });
  s.innerHTML = g;
  return s;
}
const pts = (res) => res.map((r) => ({ metric: r.metric, points: r.values.map(([t, v]) => [t, Number(v)]) }));
function chartBox(title, now_, lines, opts) {
  return el('div', { class: 'box chart' },
    el('header', {}, el('b', { text: title }), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: now_ })),
    timeChart(lines, opts),
    el('div', { class: 'legend' }, ...lines.map((l) => el('span', {}, el('i', { style: `background:${l.color}` }), l.name))));
}

// ---- the fleet, asked once per drawing

const CPU = '100 - avg by (box) (rate(node_cpu_seconds_total{mode="idle"}[5m])) * 100';
const TEMP = 'max by (box) (node_hwmon_temp_celsius{chip!~"nvme.*|ieee80211.*"})';
const NIC = 'device=~"en.*|eth.*|wl.*"';
const AC = 'power_supply!~"BAT.*|ucsi.*"';
const NAMES = {
  'dd-verify': 'The gate', nginx: 'The web front', forgejo: 'Git', 'llama-swap': 'Chat', 'dd-games': 'Games', 'ente-museum': 'Photos',
  'dd-transcode': 'Movies & TV: playback', garage: 'Storage (Garage)', postgresql: 'Databases', prometheus: 'Metrics', 'thanos-query': 'Metrics across the fleet',
  searx: 'Web search', headscale: 'The network', 'dd-agent': 'Releases', 'cloudflared': 'The public door',
};

async function fleet() {
  const q = (s) => instant(s).catch(() => []);
  const [up, info, cpu, load, mem, memTotal, temp, rx, tx, smart, wear, online, charge, failed, uptime, targets, boxes] = await Promise.all([
    q('up{job="node"}'), q('dd_box_info'), q(CPU), q('node_load1'),
    q('1 - node_memory_MemAvailable_bytes / node_memory_MemTotal_bytes'), q('node_memory_MemTotal_bytes'), q(TEMP),
    q(`sum by (box) (rate(node_network_receive_bytes_total{${NIC}}[5m]))`), q(`sum by (box) (rate(node_network_transmit_bytes_total{${NIC}}[5m]))`),
    q('smartctl_device_smart_status'), q('smartctl_device_percentage_used'),
    q(`node_power_supply_online{${AC}}`), q('node_power_supply_capacity'),
    q('node_systemd_unit_state{state="failed"} == 1'), q('node_time_seconds - node_boot_time_seconds'), q('up{job!="node"}'),
    // each box's release and backup, as its agent and backup report them
    Promise.all(['dd_agent_counter', 'dd_backup_last_success_seconds', 'dd_backup_snapshots', 'dd_backup_oldest_seconds', 'dd_backup_path'].map(q)),
  ]);
  const [release, lastBackup, snapshots, oldest, paths] = boxes;
  const names = [...new Set([...up.map((r) => r.metric.box), ...info.map((r) => r.metric.box)])].filter(Boolean).sort();
  const pick = (res, box) => res.filter((r) => r.metric.box === box);
  return names.map((box) => ({
    box,
    up: pick(up, box).some((r) => r.value[1] === '1'),
    info: pick(info, box)[0]?.metric || {},
    cpu: byBox(cpu)[box], load: byBox(load)[box], mem: byBox(mem)[box], memTotal: byBox(memTotal)[box], temp: byBox(temp)[box],
    rx: byBox(rx)[box], tx: byBox(tx)[box], uptime: byBox(uptime)[box],
    drives: pick(smart, box).map((r) => ({ device: r.metric.device, ok: r.value[1] === '1', wear: Number(pick(wear, box).find((w) => w.metric.device === r.metric.device)?.value[1] ?? NaN) })),
    mains: pick(online, box).length ? pick(online, box).some((r) => r.value[1] === '1') : null,
    battery: Number(pick(charge, box).find((r) => /^BAT/.test(r.metric.power_supply))?.value[1] ?? NaN),
    failed: pick(failed, box).map((r) => r.metric.name),
    targetsDown: pick(targets, box).filter((r) => r.value[1] === '0').map((r) => r.metric.job),
    release: {
      release: byBox(release)[box],
      backup: byBox(lastBackup)[box] === undefined ? null : {
        last_success: byBox(lastBackup)[box], snapshots: byBox(snapshots)[box], oldest: byBox(oldest)[box],
        paths: pick(paths, box).map((r) => r.metric.path).filter(Boolean),
      },
    },
  }));
}

// ---- what needs attention: checked here, from the same figures

async function checks(boxes) {
  const q = (s) => instant(s).catch(() => []);
  const [hot, full] = await Promise.all([
    q(`min_over_time((${TEMP})[15m:1m]) > 90`),
    q('(1 - node_filesystem_avail_bytes{fstype=~"zfs|ext4|xfs|btrfs"} / node_filesystem_size_bytes) > 0.85'),
  ]);
  const out = [];
  for (const b of boxes) {
    if (!b.up) out.push({ level: 'bad', box: b.box, title: `${b.box} is not answering`, words: 'Its metrics stopped coming: it is off, off the network, or its Prometheus stopped. Everything it runs is out until it is back.' });
    for (const u of b.failed) out.push({ level: 'bad', box: b.box, title: `${NAMES[u.replace(/\.service$/, '')] || u} failed on ${b.box}`, words: `systemd gave up restarting ${u}. journalctl -u ${u} on ${b.box} says why.` });
    for (const j of b.targetsDown) out.push({ level: 'busy', box: b.box, title: `${NAMES[j] || j} on ${b.box} is not answering`, words: `Its metrics stopped: the ${j} service there is down or not listening.` });
    if (b.mains === false) out.push({ level: 'bad', box: b.box, title: `${b.box} is on battery`, words: `The power is out where it is; the battery is at ${pct(b.battery)}%. It shuts itself down cleanly before it runs out.` });
    for (const d of b.drives) {
      if (!d.ok) out.push({ level: 'bad', box: b.box, title: `A drive on ${b.box} reports trouble`, words: `${d.device} failed its own health check. Its data is backed up; replace it soon.` });
      else if (d.wear >= 80) out.push({ level: 'busy', box: b.box, title: `A drive on ${b.box} is ${d.wear}% worn`, words: `${d.device} has used most of the writes it is made for. Plan to replace it.` });
    }
    const bk = b.release?.backup?.last_success;
    if (bk && now() - bk > 86400) out.push({ level: 'busy', box: b.box, title: `${b.box}'s backup is ${age(now() - bk)} old`, words: 'Nothing has been copied off it since. The backup service\'s log says why.' });
  }
  for (const r of hot) out.push({ level: 'busy', box: r.metric.box, title: `${r.metric.box} is running hot`, words: `Above 90 °C for the last 15 minutes (${Math.round(Number(r.value[1]))} °C at least). It slows itself past 100 °C. Busy is fine; if it stays hot when idle, check its vents and fan.` });
  const seen = new Set();
  for (const r of full) {
    const k = `${r.metric.box}:${Math.round(Number(r.value[1]) * 1000)}`;
    if (seen.has(k)) continue;
    seen.add(k);
    out.push({ level: 'busy', box: r.metric.box, title: `A disk on ${r.metric.box} is ${Math.round(Number(r.value[1]) * 100)}% full`, words: `${r.metric.mountpoint} and whatever shares its pool. Make room, or add a disk.` });
  }
  return out;
}

// ---- the page's frame: tabs, and where we are

let boxNames = [];
function tabs(current, alerts) {
  const t = (href, label, key, extra) => el('a', { href, 'aria-current': current === key ? 'page' : null }, label, extra);
  put($('tabs'),
    t('/', 'The fleet', 'fleet'),
    boxNames.map((b) => t(`/box/${encodeURIComponent(b)}`, b, `box:${b}`)),
    t('/alerts', 'Alerts', 'alerts', alerts ? [' ', el('span', { class: `mchip ${alerts.some((a) => a.level === 'bad') ? 'bad' : 'busy'}`, text: String(alerts.length) })] : null),
    t('/storage', 'Storage & backups', 'storage'),
    t('/explore', 'Explore', 'explore'));
}
function fleetState(boxes) {
  const answering = boxes.filter((b) => b.up).length;
  const chip = $('fleet-state');
  chip.className = `mchip ${answering === boxes.length ? 'ok' : 'bad'}`;
  chip.textContent = `${boxes.length} box${boxes.length === 1 ? '' : 'es'} · ${answering === boxes.length ? (boxes.length === 1 ? 'answering' : 'all answering') : `${boxes.length - answering} not answering`}`;
}

// ---- views

const main = () => $('app');

async function fleetView() {
  const boxes = await fleet();
  boxNames = boxes.map((b) => b.box);
  const [alerts, cpuHist, tempHist] = await Promise.all([checks(boxes), range(CPU, 6 * 3600, 72).catch(() => []), range(TEMP, 6 * 3600, 72).catch(() => [])]);
  tabs('fleet', alerts);
  fleetState(boxes);
  document.title = 'Metrics';
  const hist = (res, box) => res.find((r) => r.metric.box === box)?.values.map(([t, v]) => [t, Number(v)]) || [];
  const card = (b) => {
    const info = b.info, rel = b.release;
    const mine = alerts.filter((a) => a.box === b.box);
    const state = !b.up ? 'bad' : mine.some((a) => a.level === 'bad') ? 'bad' : mine.length ? 'busy' : 'ok';
    const g = (k, sub, v, extra, cls) => el('div', { class: 'gauge' }, el('div', { class: 'k' }, el('span', { text: k }), el('span', { text: sub })), el('div', { class: `v ${cls || ''}` }, ...v), extra);
    const memLvl = level(b.mem * 100, 80, 92), tLvl = level(b.temp, 85, 95);
    const drivesOk = b.drives.every((d) => d.ok);
    return el('div', { class: `card${b.up ? '' : ' down'}` },
      el('header', {}, el('span', { class: `dot ${state}` }), el('h2', {}, el('a', { href: `/box/${encodeURIComponent(b.box)}`, style: 'color:var(--ink)', text: b.box })),
        el('span', { class: `mchip ${state}`, text: !b.up ? 'not answering' : state === 'ok' ? 'fine' : `${mine.length} to look at` }), el('span', { class: 'spacer' }),
        el('span', { class: 'small muted', text: [rel?.release ? `release ${rel.release}` : null, b.uptime ? `up ${age(b.uptime)}` : null].filter(Boolean).join(' · ') })),
      el('div', { class: 'what', text: [info.model?.replace(/\(R\)|\(TM\)|with Radeon Graphics| @ .*/g, '').trim(), b.memTotal ? `${bytes(b.memTotal)} memory` : null, info.kernel ? `kernel ${info.kernel}` : null].filter(Boolean).join(' · ') }),
      b.up ? el('div', { class: 'gauges' },
        g('CPU', b.load !== undefined ? `load ${b.load.toFixed(1)}` : '', [pct(b.cpu), el('small', { text: '%' })], sparkline(hist(cpuHist, b.box), 'var(--c1)', 100)),
        g('Memory', b.memTotal ? `of ${Math.round(b.memTotal / 2 ** 30)} GB` : '', [pct(b.mem * 100), el('small', { text: '%' })], el('div', { class: 'meter' }, el('i', { class: memLvl === 'ok' ? '' : memLvl, style: `width:${pct(b.mem * 100)}%` })), memLvl === 'ok' ? '' : memLvl),
        g('Temperature', '', [Number.isFinite(b.temp) ? pct(b.temp) : '–', el('small', { text: '°C' })], sparkline(hist(tempHist, b.box), 'var(--c4)', 110), tLvl === 'ok' ? '' : tLvl),
        g('Network', 'in · out', [bytes(b.rx), el('small', { text: '/s' })], el('div', { class: 'small muted', text: `out ${bytes(b.tx, '/s')}` })),
        g('Drives', String(b.drives.length), [b.drives.length ? (drivesOk ? 'Healthy' : 'Trouble') : '–'], el('div', { class: 'small muted', text: b.drives.map((d) => `${d.device}${Number.isFinite(d.wear) ? ` ${d.wear}% worn` : ''}`).join(' · ') }), b.drives.length ? (drivesOk ? 'ok' : 'bad') : ''),
        g('Power', b.mains === null ? '' : b.mains ? 'on mains' : 'ON BATTERY', Number.isFinite(b.battery) ? [pct(b.battery), el('small', { text: '% battery' })] : ['mains'], Number.isFinite(b.battery) ? el('div', { class: 'meter' }, el('i', { class: b.battery < 30 ? 'bad' : '', style: `width:${b.battery}%` })) : null, b.mains === false ? 'bad' : ''))
        : el('div', { class: 'what', text: 'No figures: the box is not answering.' }),
      el('footer', {},
        el('span', {}, el('span', { class: `dot ${b.failed.length ? 'bad' : 'ok'}` }), b.failed.length ? ` ${b.failed.length} failed service${b.failed.length === 1 ? '' : 's'}` : ' no failed services'),
        rel?.backup?.last_success ? el('span', {}, el('span', { class: `dot ${now() - rel.backup.last_success > 86400 ? 'busy' : 'ok'}` }), ` backed up ${ago(rel.backup.last_success)}`) : null));
  };
  put(main(),
    alerts.map((a) => el('div', { class: `banner${a.level === 'bad' ? ' bad' : ''}` }, el('span', { class: `dot ${a.level}`, style: 'margin-top:6px' }), el('div', { style: 'flex:1' }, el('b', { text: a.title }), el('span', { class: 'muted small', text: a.words })))),
    el('div', { class: 'cards' }, boxes.map(card)),
    el('p', { class: 'small muted', style: 'margin-top:14px', text: 'Every box answers here from its own Prometheus, through Thanos. A box that stops answering greys out.' }));
}

const SPANS = [['1h', '1 hour', 3600], ['24h', '24 hours', 86400], ['7d', '7 days', 7 * 86400], ['30d', '30 days', 30 * 86400]];
function spanPicker(key, base) {
  return el('span', { class: 'mseg' }, SPANS.map(([k, t]) => el('a', { class: 'mbtn', href: `${base}?span=${k}`, 'aria-pressed': String(k === key), text: t })));
}

async function boxView(name, params) {
  const [boxes] = await Promise.all([fleet()]);
  boxNames = boxes.map((b) => b.box);
  const b = boxes.find((x) => x.box === name);
  const alerts = await checks(boxes);
  tabs(`box:${name}`, alerts);
  fleetState(boxes);
  document.title = `${name} · Metrics`;
  if (!b) { put(main(), el('div', { class: 'box empty', text: `No box called ${name} in the fleet.` })); return; }
  const key = params.get('span') || '24h';
  const secs = SPANS.find((s) => s[0] === key)?.[2] || 86400;
  const sel = `box="${name.replace(/"/g, '')}"`;
  const r = (query) => range(query, secs).then(pts).catch(() => []);
  const [busy, iowait, cpuT, ssdT, used, arc, rx, tx, units] = await Promise.all([
    r(`100 - avg by (box) (rate(node_cpu_seconds_total{${sel},mode="idle"}[5m])) * 100`),
    r(`avg by (box) (rate(node_cpu_seconds_total{${sel},mode="iowait"}[5m])) * 100`),
    r(`max by (box) (node_hwmon_temp_celsius{${sel},chip!~"nvme.*|ieee80211.*"})`),
    r(`max by (box) (node_hwmon_temp_celsius{${sel},chip=~"nvme.*"})`),
    r(`node_memory_MemTotal_bytes{${sel}} - node_memory_MemAvailable_bytes{${sel}}`),
    r(`node_zfs_arc_size{${sel}}`),
    r(`sum by (box) (rate(node_network_receive_bytes_total{${sel},${NIC}}[5m]))`),
    r(`sum by (box) (rate(node_network_transmit_bytes_total{${sel},${NIC}}[5m]))`),
    instant(`node_systemd_unit_state{${sel},state=~"active|failed"} == 1`).catch(() => []),
  ]);
  const line = (name_, color, s) => ({ name: name_, color, points: s[0]?.points || [] });
  const last = (s) => s[0]?.points.at(-1)?.[1];
  // systemd writes a dash inside a name as \x2d
  const services = units.map((u) => ({ unit: u.metric.name.replace(/\.service$/, '').replace(/\\x([0-9a-f]{2})/g, (_, h) => String.fromCharCode(parseInt(h, 16))), state: u.metric.state }))
    .filter((u) => NAMES[u.unit] || /^gitea-runner-|^dd-game@|^thanos-/.test(u.unit) || u.state === 'failed')
    .map((u) => ({ ...u, name: NAMES[u.unit] || (u.unit.startsWith('gitea-runner') ? 'CI runner' : u.unit.startsWith('dd-game@') ? `Game server ${u.unit.slice(8)}` : u.unit) }))
    .sort((a, b2) => (a.state === 'failed' ? -1 : 0) - (b2.state === 'failed' ? -1 : 0) || a.name.localeCompare(b2.name));
  put(main(),
    el('div', { class: 'mrow gap' }, el('h2', { style: 'margin:0;font-size:20px', text: name }), el('span', { class: `mchip ${b.up ? 'ok' : 'bad'}`, text: b.up ? 'answering' : 'not answering' }),
      el('span', { class: 'muted small', text: [b.info.model, b.memTotal ? bytes(b.memTotal) : null, b.uptime ? `up ${age(b.uptime)}` : null, b.release?.release ? `release ${b.release.release}` : null].filter(Boolean).join(' · ') }),
      el('span', { class: 'spacer' }), spanPicker(key, `/box/${encodeURIComponent(name)}`)),
    el('div', { class: 'charts gap' },
      chartBox('CPU', Number.isFinite(last(busy)) ? `now ${pct(last(busy))}%` : '', [line('busy', 'var(--c1)', busy), line('waiting on disk', 'var(--c3)', iowait)], { max: 100, unit: (v) => `${Math.round(v)}%`, secs }),
      chartBox('Temperature', Number.isFinite(last(cpuT)) ? `now ${pct(last(cpuT))} °C` : '', [line('hottest chip', 'var(--c4)', cpuT), line('SSD', 'var(--c2)', ssdT)], { max: 120, unit: (v) => `${Math.round(v)}°`, warn: { at: 100, label: 'slows itself' }, secs }),
      chartBox('Memory', Number.isFinite(last(used)) ? `now ${bytes(last(used))} of ${bytes(b.memTotal)}` : '', [line('used by programs and cache', 'var(--c1)', used), line('ZFS cache (ARC)', 'var(--c2)', arc)], { max: b.memTotal, unit: (v) => bytes(v), secs }),
      chartBox('Network', Number.isFinite(last(rx)) ? `now ${bytes(last(rx), '/s')} in · ${bytes(last(tx), '/s')} out` : '', [line('in', 'var(--c2)', rx), line('out', 'var(--c3)', tx)], { unit: (v) => bytes(v, '/s'), secs })),
    el('div', { class: 'two' },
      el('div', { class: 'box' }, el('header', {}, el('b', { text: 'What runs here' }), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: `${services.filter((s) => s.state === 'active').length} running` })),
        el('div', { class: 'mlist' }, services.length ? services.map((s) => el('div', {}, el('span', { class: `dot ${s.state === 'failed' ? 'bad' : 'ok'}` }), el('b', { text: s.name }), el('span', { class: 'small muted mono', text: s.unit }), el('span', { class: 'spacer' }), s.state === 'failed' ? el('span', { class: 'mchip bad', text: 'failed' }) : null)) : el('div', { class: 'muted', text: 'Nothing known yet.' }))),
      el('aside', {},
        el('div', { class: 'box gap' }, el('header', {}, el('b', { text: 'Drives' })), el('div', { class: 'mlist' }, b.drives.length ? b.drives.map((d) => el('div', {}, el('span', { class: `dot ${d.ok ? 'ok' : 'bad'}` }), el('div', { style: 'flex:1' }, el('b', { class: 'mono', text: d.device }), el('div', { class: 'small muted', text: Number.isFinite(d.wear) ? `${d.wear}% of its writes used` : 'health checked daily' })), el('span', { class: `mchip ${d.ok ? 'ok' : 'bad'}`, text: d.ok ? 'healthy' : 'trouble' }))) : el('div', { class: 'muted', text: 'No drive health reported.' }))),
        el('div', { class: 'box' }, el('header', {}, el('b', { text: 'Power' })), el('div', { class: 'mlist' },
          el('div', {}, el('span', { class: `dot ${b.mains === false ? 'bad' : 'ok'}` }), b.mains === false ? 'On battery' : 'On mains', el('span', { class: 'spacer' }), Number.isFinite(b.battery) ? el('span', { class: 'small muted', text: `battery ${pct(b.battery)}%` }) : null))))));
}

async function alertsView() {
  const boxes = await fleet();
  boxNames = boxes.map((b) => b.box);
  const q = (s) => instant(s).catch(() => []);
  const [alerts, hotWeek, batteryWeek, downWeek, failedWeek] = await Promise.all([
    checks(boxes),
    q(`max_over_time((${TEMP})[7d:5m]) > 90`),
    q(`min_over_time(node_power_supply_online{${AC}}[7d]) == 0`),
    q('min_over_time(up{job="node"}[7d]) == 0'),
    q('max_over_time((count by (box) (node_systemd_unit_state{state="failed"} == 1))[7d:5m]) > 0'),
  ]);
  tabs('alerts', alerts);
  fleetState(boxes);
  document.title = 'Alerts · Metrics';
  const earlier = [
    ...hotWeek.map((r) => [r.metric.box, `${r.metric.box} ran hot`, `peaked at ${Math.round(Number(r.value[1]))} °C`]),
    ...batteryWeek.map((r) => [r.metric.box, `${r.metric.box} ran on battery`, 'the power went out at least once']),
    ...downWeek.map((r) => [r.metric.box, `${r.metric.box} stopped answering`, 'at least once']),
    ...failedWeek.map((r) => [r.metric.box, `A service failed on ${r.metric.box}`, `${r.value[1]} at once, at worst`]),
  ];
  const rule = (when, means, tells) => el('tr', {}, el('td', { text: when }), el('td', { text: means }), el('td', { text: tells }));
  put(main(), el('div', { style: 'max-width:980px' },
    el('h2', { style: 'margin:0 0 12px;font-size:20px', text: 'Now' }),
    el('div', { class: 'box gap' }, el('div', { class: 'mlist' }, alerts.length ? alerts.map((a) => el('div', { style: 'align-items:flex-start' }, el('span', { class: `dot ${a.level}`, style: 'margin-top:6px' }), el('div', { style: 'flex:1' }, el('b', { text: a.title }), el('div', { class: 'small muted', text: a.words })))) : el('div', {}, el('span', { class: 'dot ok' }), 'Nothing needs looking at.'))),
    el('h2', { style: 'margin:0 0 12px;font-size:20px', text: 'In the last 7 days' }),
    el('div', { class: 'box gap' }, el('div', { class: 'mlist' }, earlier.length ? earlier.map(([, t, w]) => el('div', {}, el('span', { class: 'dot' }), el('b', { text: t }), el('span', { class: 'small muted', text: w }))) : el('div', { class: 'muted', text: 'Nothing happened.' }))),
    el('h2', { style: 'margin:0 0 12px;font-size:20px', text: 'What is watched' }),
    el('div', { class: 'box table-wrap' }, el('table', { class: 't' },
      el('tr', {}, el('th', { text: 'When' }), el('th', { text: 'Means' }), el('th', { text: 'Tells' })),
      rule('A box stops answering', 'off, or off the network', 'here'),
      rule('A service fails', 'systemd gave up restarting it', 'here'),
      rule('A backup is more than a day old', 'nothing copied off since', 'here'),
      rule('A drive fails its health check, or passes 80% worn', 'replace it soon', 'email (smartd) · here'),
      rule('A ZFS pool has trouble', 'a disk or its data is in danger', 'email (zed)'),
      rule('Hotter than 90 °C for 15 minutes', 'busy, or the fan is blocked', 'here'),
      rule('On battery', 'the power is out', 'here'),
      rule('A disk more than 85% full', 'make room, or add a disk', 'here'))),
    el('p', { class: 'small muted', text: 'Only the drive and pool alarms send mail today; the rest show here when this page is open.' })));
}

async function storageView() {
  const boxes = await fleet();
  boxNames = boxes.map((b) => b.box);
  const [fs, arc, alerts] = await Promise.all([
    instant('node_filesystem_size_bytes{fstype=~"zfs|ext4|xfs|btrfs|vfat"}').catch(() => []),
    instant('node_filesystem_avail_bytes{fstype=~"zfs|ext4|xfs|btrfs|vfat"}').catch(() => []),
    checks(boxes),
  ]);
  tabs('storage', alerts);
  fleetState(boxes);
  document.title = 'Storage · Metrics';
  const avail = new Map(arc.map((r) => [`${r.metric.box}|${r.metric.mountpoint}`, Number(r.value[1])]));
  const rows = fs.map((r) => {
    const size = Number(r.value[1]), free = avail.get(`${r.metric.box}|${r.metric.mountpoint}`) ?? NaN;
    return { box: r.metric.box, mount: r.metric.mountpoint, dev: r.metric.device, type: r.metric.fstype, size, used: size - free, free };
  }).filter((x) => x.size > 1e8 && !/^\/(boot|nix\/store|run|proc|sys)/.test(x.mount))
    .sort((a, b) => b.used - a.used);
  const most = Math.max(1, ...rows.map((x) => x.used));
  const backups = boxes.filter((b) => b.release?.backup);
  put(main(), el('div', { class: 'two' },
    el('div', {},
      el('div', { class: 'box gap' }, el('header', {}, el('b', { text: 'Where the space goes' }), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: 'every dataset and disk, biggest first' })),
        el('div', { class: 'table-wrap' }, el('table', { class: 't' },
          el('tr', {}, el('th', { text: 'Where' }), el('th', { text: 'Box' }), el('th', { text: 'Used' }), el('th', { text: 'Free in its pool' }), el('th', { style: 'width:30%' })),
          rows.map((x) => {
            const full = x.size ? x.used / x.size : 0;
            return el('tr', {}, el('td', {}, el('span', { class: 'mono', text: x.mount }), el('div', { class: 'small muted', text: x.dev })), el('td', { text: x.box }), el('td', { text: bytes(x.used) }), el('td', { text: bytes(x.free) }),
              el('td', {}, el('div', { class: 'bar2', title: `${Math.round(full * 100)}% of what it can use` }, el('i', { class: full > 0.9 ? 'bad' : full > 0.85 ? 'busy' : '', style: `width:${Math.max(1, (x.used / most) * 100)}%` }))));
          }))))),
    el('aside', {},
      el('div', { class: 'box gap' }, el('header', {}, el('b', { text: 'Backups' })), el('div', { class: 'mlist' },
        backups.length ? backups.map((b) => {
          const k = b.release.backup;
          return el('div', { style: 'align-items:flex-start' }, el('span', { class: `dot ${k.last_success && now() - k.last_success < 86400 ? 'ok' : 'busy'}`, style: 'margin-top:6px' }), el('div', { style: 'flex:1' }, el('b', { text: b.box }),
            el('div', { class: 'small muted', text: [k.last_success ? `last ${ago(k.last_success)}` : 'never yet', k.snapshots ? `${k.snapshots} snapshots` : null, k.oldest ? `oldest ${ago(k.oldest)}` : null].filter(Boolean).join(' · ') }),
            k.paths?.length ? el('div', { class: 'small muted mono', text: k.paths.join(' ') }) : null));
        }) : el('div', { class: 'muted', text: 'No box reports a backup yet.' }))),
      el('div', { class: 'box' }, el('header', {}, el('b', { text: 'Garage' }), el('span', { class: 'small muted', text: 'the boxes\' object store' })), el('div', { class: 'mlist' },
        boxes.map((b) => el('div', {}, el('span', { class: `dot ${b.targetsDown.includes('garage') ? 'bad' : 'ok'}` }), b.box, el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: b.targetsDown.includes('garage') ? 'not answering' : 'answering' }))))))));
}

const SUGGEST = ['node_hwmon_temp_celsius', 'node_memory_MemAvailable_bytes', 'rate(node_disk_written_bytes_total[5m])', 'smartctl_device_percentage_used', 'node_systemd_unit_state{state="failed"} == 1', 'dd_box_info'];
async function exploreView(params) {
  const boxes = await fleet().catch(() => []);
  boxNames = boxes.map((b) => b.box);
  tabs('explore', null);
  if (boxes.length) fleetState(boxes);
  document.title = 'Explore · Metrics';
  const query = params.get('q') || CPU;
  const key = params.get('span') || '24h';
  const secs = SPANS.find((s) => s[0] === key)?.[2] || 86400;
  const input = el('input', { value: query, 'aria-label': 'Query', spellcheck: 'false' });
  const run = (q2, k) => go(`/explore?${new URLSearchParams({ q: q2 ?? input.value, span: k ?? key })}`);
  input.onkeydown = (e) => { if (e.key === 'Enter') run(); };
  const top = el('div', { class: 'box gap' }, el('div', { style: 'padding:14px 16px' },
    el('div', { class: 'q' }, input, el('span', { class: 'mseg' }, SPANS.map(([k, t]) => el('button', { class: 'mbtn', 'aria-pressed': String(k === key), text: t, onclick: () => run(undefined, k) }))), el('button', { class: 'mbtn go', text: 'Run', onclick: () => run() })),
    el('div', { class: 'suggest' }, el('span', { class: 'small muted', text: 'Try:' }), SUGGEST.map((s) => el('button', { text: s, onclick: () => run(s) })))));
  let series = [], error = null;
  try { series = pts(await range(query, secs)); } catch (e) { error = e.message; }
  const label = (m) => { const { __name__, ...rest } = m; const s = Object.entries(rest).map(([k, v]) => `${k}="${v}"`).join(', '); return `${__name__ || ''}{${s}}`; };
  const shown = series.slice(0, 12);
  const stats = (p) => { const v = p.map((x) => x[1]); return { now: v.at(-1), avg: v.reduce((a, b2) => a + b2, 0) / (v.length || 1), max: Math.max(...v) }; };
  const fmt = (v) => (!Number.isFinite(v) ? '–' : Math.abs(v) >= 1e6 ? bytes(v) : Math.abs(v) >= 100 ? v.toFixed(0) : v.toFixed(2));
  put(main(), top,
    error ? el('div', { class: 'box empty' }, el('p', { class: 'err', text: error })) : [
      el('div', { class: 'box chart gap' }, el('header', {}, el('b', { class: 'mono', text: query.length > 90 ? `${query.slice(0, 90)}…` : query }), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: `${series.length} series${series.length > 12 ? ', the first 12 drawn' : ''}` })),
        timeChart(shown.map((s, i) => ({ name: label(s.metric), color: COLORS[i % COLORS.length], points: s.points })), { unit: fmt, secs }),
        el('div', { class: 'legend' }, shown.map((s, i) => el('span', { class: 'mono' }, el('i', { style: `background:${COLORS[i % COLORS.length]}` }), label(s.metric))))),
      el('div', { class: 'box table-wrap' }, el('table', { class: 't' }, el('tr', {}, el('th', { text: 'Series' }), el('th', { text: 'Now' }), el('th', { text: 'Average' }), el('th', { text: 'Highest' })),
        series.map((s) => { const st = stats(s.points); return el('tr', {}, el('td', { class: 'mono', text: label(s.metric) }), el('td', { text: fmt(st.now) }), el('td', { text: fmt(st.avg) }), el('td', { text: fmt(st.max) })); }))),
    ],
    el('p', { class: 'small muted', style: 'margin-top:12px', text: 'Any Prometheus query over the whole fleet, through Thanos: the label box names the box.' }));
}

// ---- where we are

let drawing = 0;
async function render() {
  const mine = ++drawing;
  const path = decodeURIComponent(location.pathname).replace(/\/+$/, '') || '/';
  const params = new URLSearchParams(location.search);
  try {
    if (path === '/') await fleetView();
    else if (path.startsWith('/box/')) await boxView(path.slice(5), params);
    else if (path === '/alerts') await alertsView();
    else if (path === '/storage') await storageView();
    else if (path === '/explore') await exploreView(params);
    else put(main(), el('div', { class: 'box empty', text: 'Nothing here.' }));
  } catch (e) {
    if (mine === drawing) put(main(), el('div', { class: 'box empty' }, el('p', { class: 'err', text: e.message })));
  }
}
function go(href) {
  history.pushState({}, '', href);
  render();
  scrollTo(0, 0);
}
document.addEventListener('click', (e) => {
  if (e.defaultPrevented || e.button !== 0 || e.metaKey || e.ctrlKey || e.shiftKey) return;
  const a = e.target.closest('a[href]');
  if (!a) return;
  const u = new URL(a.href, location.href);
  if (u.origin !== location.origin || u.pathname.startsWith('/_dd/') || u.pathname.startsWith('/api/')) return;
  e.preventDefault();
  go(u.pathname + u.search);
});
addEventListener('popstate', render);
// the fleet page keeps itself current
setInterval(() => { if (location.pathname === '/' && !document.hidden) render(); }, 30000);
render();
