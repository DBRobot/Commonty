// Actions: the runs, and one run as a graph of its jobs - read from the
// workflow files' `needs:` - or as its jobs and their logs, both live.

import { put, app, el, ic, api, text, q, when, plural, duration, avatar, route, go, statusDot, stateOf, ghost, setTitle, short, pager, warmers, watch } from './git-core.js';
import { repo, header } from './git-repo.js';

const main = () => app();
const DONE = ['success', 'failure', 'cancelled', 'skipped'];

// how long a job took, or has taken so far: the running ones count up
function took(j) {
  if (!j?.started) return el('span', { class: 'took' });
  const t = el('span', { class: 'took', 'data-start': j.started });
  if (j.ended) t.dataset.end = j.ended;
  tickOne(t);
  return t;
}
function tickOne(t) {
  const end = t.dataset.end ? new Date(t.dataset.end) : Date.now();
  t.textContent = duration(Math.max(0, end - new Date(t.dataset.start)));
}
setInterval(() => document.querySelectorAll('.took[data-start]:not([data-end])').forEach(tickOne), 1000);
const ns = (d) => (d > 1e9 ? d / 1e6 : d); // the forge gives nanoseconds

// ---- the runs

const runsData = (r, page = 1, fresh = false) => api(`/repos/${r.full_name}/actions/runs${q({ page, limit: 25 })}`, { fresh });
warmers.actions = (r) => runsData(r);

route(/^\/([^/]+)\/([^/]+)\/actions$/, async ({ m, params, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'actions');
  setTitle('Actions', r.full_name);
  const page = Number(params.get('page') || 1);
  const workflow = params.get('workflow') || '';
  let { workflow_runs: runs = [], total_count: total = 0 } = await runsData(r, page);
  if (!current()) return;
  const row = (x) => el('a', { class: 'item', href: `/${r.full_name}/actions/runs/${x.index_in_repo}` },
    el('span', { style: 'margin-top:6px' }, statusDot(x.status)),
    el('div', { style: 'flex:1;min-width:0' }, el('div', { class: 't', text: x.title }),
      el('div', { class: 'sub' }, `Run ${x.index_in_repo} · ${x.workflow_id} · ${x.prettyref || ''} · ${x.event} · `, x.trigger_user?.login || '', ' · ', el('span', { class: 'mono', text: short(x.commit_sha) }))),
    el('div', { class: 'small muted', style: 'text-align:right' }, when(x.created), el('br'), x.status === 'running' ? 'running' : x.duration ? duration(ns(x.duration)) : x.status));
  const aside = el('aside', { class: 'jobs' });
  const list = el('div', { class: 'list' });
  const count = el('span', { class: 'small muted' });
  const pagerSlot = el('div');
  // drawn again in place as runs start and finish: where you are scrolled stays
  const draw = () => {
    const flows = [...new Set(runs.map((x) => x.workflow_id))].sort();
    const shown = workflow ? runs.filter((x) => x.workflow_id === workflow) : runs;
    aside.replaceChildren(
      el('a', { href: `/${r.full_name}/actions`, 'aria-current': !workflow ? 'page' : null, text: 'All runs' }),
      ...flows.map((f) => el('a', { href: `/${r.full_name}/actions${q({ workflow: f })}`, 'aria-current': workflow === f ? 'page' : null, text: f.replace(/\.ya?ml$/, '') })));
    list.replaceChildren(...(shown.length ? shown.map(row) : [el('div', { class: 'empty', text: 'No runs yet.' })]));
    count.textContent = plural(total, 'run');
    pagerSlot.replaceChildren(pager(page, total, 25, (p) => go(`/${r.full_name}/actions${q({ page: p, workflow })}`)) || '');
  };
  draw();
  put(main(), el('div', { class: 'two left' }, aside,
    el('div', {},
      el('div', { class: 'box' }, el('header', {}, el('b', { text: workflow || 'All runs' }), el('span', { class: 'spacer' }), count), list),
      pagerSlot)));
  // a run starting, a job moving, a run ending: the forge says so, and the
  // list is asked again then, and only then
  watch(r.id, () => current() && list.isConnected, async (changes) => {
    if (!changes.some((c) => c.kind === 'run' || c.kind === 'job')) return;
    try {
      ({ workflow_runs: runs = [], total_count: total = 0 } = await runsData(r, page, true));
      if (current() && list.isConnected) draw();
    } catch { /* the next change asks again */ }
  });
});

// ---- one run

// the run by its number: the forge asks by id, which runs a fixed distance
// ahead of the number on this forge; check, and look further if not. The
// newest run says how far: asked with a page, or the forge ignores the
// limit and sends every run there has ever been
const gap = new Map();
async function runByIndex(r, index) {
  if (!gap.has(r.id)) {
    const { workflow_runs: first = [] } = await api(`/repos/${r.full_name}/actions/runs${q({ page: 1, limit: 1 })}`);
    if (first[0]) gap.set(r.id, first[0].id - first[0].index_in_repo);
  }
  if (gap.has(r.id)) {
    const guess = await api(`/repos/${r.full_name}/actions/runs/${index + gap.get(r.id)}`).catch(() => null);
    if (guess?.index_in_repo === index) return guess;
  }
  for (let page = 1; page <= 20; page++) {
    const { workflow_runs: runs = [] } = await api(`/repos/${r.full_name}/actions/runs${q({ page, limit: 50 })}`);
    const hit = runs.find((x) => x.index_in_repo === index);
    if (hit) return hit;
    if (!runs.length || runs[runs.length - 1].index_in_repo < index) break;
  }
  throw Object.assign(new Error('No such run'), { status: 404 });
}

// the jobs of the run, from the statuses it posted on its commit: each
// names its job and links to it by position
async function jobsOf(r, run, fresh = false) {
  const all = [];
  for (let page = 1; page <= 4; page++) {
    const s = await api(`/repos/${r.full_name}/commits/${run.commit_sha}/statuses${q({ page, limit: 50 })}`, { fresh }).catch(() => []);
    all.push(...s);
    if (s.length < 50) break;
  }
  // by name: a run started again posts every job's statuses again, and the
  // newest says where each job is
  const jobs = new Map();
  for (const s of all.sort((a, b) => a.id - b.id)) {
    const m = (s.target_url || '').match(/\/actions\/runs\/(\d+)\/jobs\/(\d+)/);
    if (!m || Number(m[1]) !== run.index_in_repo) continue;
    const name = s.context.replace(/ \((pull_request|push|schedule|workflow_dispatch)\)$/, '');
    const prev = jobs.get(name);
    // the forge says pending for waiting, blocked and running alike: its
    // words tell them apart, and only a job a runner has picked up is running
    const running = /started running/i.test(s.description || '');
    // and a job skipped is "success, has been skipped": its own state here
    const state = s.status !== 'pending' ? (/been skipped/i.test(s.description || '') ? 'skipped' : s.status)
      : running ? 'running' : /blocked/i.test(s.description || '') ? 'blocked' : 'waiting';
    // when it began and ended, from the statuses the forge posted as it
    // went; waiting again is a new attempt, and the last one's times go
    const started = running ? s.created_at : state === 'waiting' || state === 'blocked' ? null : prev?.started;
    const ended = s.status !== 'pending' ? s.created_at : null;
    jobs.set(name, { index: Number(m[2]), name, leaf: name.split(' / ').pop(), state, description: s.description, started, ended });
  }
  // a run that has stopped leaves nothing running: what never said it
  // finished was cancelled with it
  if (DONE.includes(run.status)) {
    const at = run.stopped && new Date(run.stopped) >= new Date(run.started) ? run.stopped : run.updated;
    for (const j of jobs.values()) {
      if (DONE.includes(j.state) || j.state === 'error') continue;
      j.state = 'cancelled';
      if (j.started) j.ended = at;
    }
  }
  return [...jobs.values()].sort((a, b) => a.index - b.index);
}

// ---- the graph, from the workflow files

// Enough YAML to read a workflow's jobs: each job's id, `needs:` and
// `uses:` (a workflow it calls). Anything else is ignored.
function readJobs(src) {
  const lines = src.split('\n');
  const jobs = [];
  let inJobs = false, cur = null, listKey = null;
  for (const raw of lines) {
    const line = raw.replace(/\s+#.*$/, '');
    if (!line.trim()) continue;
    const indent = line.match(/^ */)[0].length;
    if (indent === 0) { inJobs = /^jobs:\s*$/.test(line); cur = null; continue; }
    if (!inJobs) continue;
    if (indent === 2 && /^\s{2}[\w-]+:\s*$/.test(line)) { cur = { id: line.trim().slice(0, -1), needs: [], uses: null }; jobs.push(cur); listKey = null; continue; }
    if (!cur) continue;
    const kv = line.match(/^\s{4}([\w-]+):\s*(.*)$/);
    if (kv) {
      listKey = null;
      if (kv[1] === 'needs') {
        const v = kv[2].trim();
        if (!v) listKey = 'needs';
        else cur.needs = v.replace(/^\[|\]$/g, '').split(',').map((s) => s.trim().replace(/^['"]|['"]$/g, '')).filter(Boolean);
      } else if (kv[1] === 'uses') cur.uses = kv[2].trim().replace(/^['"]|['"]$/g, '');
      continue;
    }
    const item = line.match(/^\s{6}-\s*(.+)$/);
    if (item && listKey === 'needs') cur.needs.push(item[1].trim().replace(/^['"]|['"]$/g, ''));
  }
  return jobs;
}

async function workflow(r, sha, file) {
  for (const dir of ['.forgejo/workflows', '.gitea/workflows', '.github/workflows']) {
    const src = await text(`/api/v1/repos/${r.full_name}/raw/${dir}/${file}${q({ ref: sha })}`).catch(() => null);
    if (src !== null) return { src, dir };
  }
  return null;
}

// Nodes are job ids, with a called workflow's jobs in its place; a called
// workflow's first jobs wait on what the caller waited on.
async function graph(r, run) {
  const top = await workflow(r, run.commit_sha, run.workflow_id);
  if (!top) return null;
  const nodes = [], edges = [];
  const ends = new Map(); // a job id -> the node ids that finish it
  const expand = async (src, dir, group, before) => {
    const jobs = readJobs(src);
    for (const j of jobs) {
      const waits = j.needs.flatMap((n) => ends.get(`${group}:${n}`) || []);
      const pre = j.needs.length ? waits : before;
      if (j.uses && j.uses.startsWith('./')) {
        const sub = await text(`/api/v1/repos/${r.full_name}/raw/${j.uses.slice(2)}${q({ ref: run.commit_sha })}`).catch(() => null);
        if (sub) {
          const inner = await expand(sub, dir, j.id, pre);
          ends.set(`${group}:${j.id}`, inner);
          continue;
        }
      }
      const id = j.id;
      nodes.push({ id, group: group === '' ? '' : group });
      for (const p of pre) edges.push([p, id]);
      ends.set(`${group}:${j.id}`, [id]);
    }
    // what finishes this workflow: jobs nothing inside it waits on
    const inner = jobs.flatMap((j) => ends.get(`${group}:${j.id}`) || []);
    return inner.filter((id) => !edges.some(([a, b]) => a === id && inner.includes(b)));
  };
  await expand(top.src, top.dir, '', []);
  // a line only where nothing else already says it: a job that waits on
  // every other one (ci_ok) draws from the last ones, not from all of them
  const after = (a) => edges.filter(([x]) => x === a).map(([, y]) => y);
  const reaches = (from, to, skip) => {
    const seen = new Set(), todo = after(from).filter((y) => y !== skip);
    while (todo.length) {
      const y = todo.pop();
      if (y === to) return true;
      if (!seen.has(y)) { seen.add(y); todo.push(...after(y)); }
    }
    return false;
  };
  const direct = edges.filter(([a, b]) => !reaches(a, b, b));
  edges.splice(0, edges.length, ...direct);
  return { nodes, edges };
}

// a job's instances: "vm_tests (games)" belongs to the node vm_tests
const nodeOf = (leaf) => leaf.replace(/ \(.*\)$/, '');
function rollup(jobs) {
  const s = jobs.map((j) => j.state);
  if (s.some((x) => x === 'failure' || x === 'error')) return 'failure';
  if (s.some((x) => x === 'running')) return 'running';
  if (s.some((x) => x === 'pending' || x === 'waiting' || x === 'blocked')) return 'waiting';
  if (s.length && s.every((x) => x === 'skipped')) return 'skipped';
  if (s.length && s.every((x) => x === 'success' || x === 'skipped')) return 'success';
  return s[0] || 'waiting';
}

// a matrix with more jobs than this shows them as dots until asked
const MATRIX_ROWS = 4;
// the matrices opened on this page, kept while it redraws
const shown = new Set();

function drawGraph(r, run, g, jobs) {
  const dag = el('div', { class: 'dag' });
  const edgesSvg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  edgesSvg.setAttribute('class', 'edges');
  const byNode = new Map(g.nodes.map((n) => [n.id, []]));
  for (const j of jobs) (byNode.get(nodeOf(j.leaf)) || byNode.get(j.leaf))?.push(j);
  const card = (n) => {
    const js = byNode.get(n.id) || [];
    const state = js.length ? rollup(js) : 'waiting';
    const cls = `job${state === 'running' ? ' working' : state === 'failure' ? ' broken' : ''}`;
    const one = js.length === 1 && js[0].leaf === n.id;
    const head = el(one ? 'a' : 'div', one ? { href: `/${r.full_name}/actions/runs/${run.index_in_repo}${q({ job: js[0].index })}` } : {},
      statusDot(state), el('span', { class: 't', text: n.id }, n.group ? el('span', { class: 'grp', text: n.group }) : null), js.length > 1 ? el('small', { text: `${js.filter((j) => DONE.includes(j.state)).length} of ${js.length}` }) : el('small', {}, js[0]?.started ? took(js[0]) : js[0] ? (js[0].state === 'blocked' ? 'blocked' : DONE.includes(js[0].state) ? '' : 'waiting') : ''));
    const name = (j) => j.leaf.replace(`${n.id} `, '') || j.leaf;
    const href = (j) => `/${r.full_name}/actions/runs/${run.index_in_repo}${q({ job: j.index })}`;
    const rows = () => js.map((j) => el('a', { class: 'sub', href: href(j) }, statusDot(j.state), el('span', { style: 'flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap', text: name(j) }), el('small', {}, took(j))));
    if (!(js.length > 1 || (js.length === 1 && !one))) return el('div', { class: cls, 'data-id': n.id }, head);
    if (js.length <= MATRIX_ROWS) return el('div', { class: cls, 'data-id': n.id }, head, ...rows());
    // a matrix of many: a dot for each, its name on hover, and the rows on asking
    const c = el('div', { class: cls, 'data-id': n.id });
    const open = shown.has(n.id);
    const toggle = el('button', { class: 'more', type: 'button', text: open ? 'Fewer' : `Show all ${js.length}` });
    toggle.onclick = () => { open ? shown.delete(n.id) : shown.add(n.id); c.replaceWith(card(n)); };
    c.append(head, ...(open ? rows() : [el('div', { class: 'dots' }, ...js.map((j) => el('a', { href: href(j), title: `${name(j)} · ${j.state}${j.started ? ` · ${took(j)}` : ''}` }, statusDot(j.state))))]), toggle);
    return c;
  };

  // ELK lays it out (elk.js in the web bundle, loaded on this page alone):
  // the cards keep their own size, and it says where each goes and where
  // each line bends. Lines going the same way share their path.
  const edges = g.edges;
  const layer = el('div', { class: 'cards' });
  const cards = new Map(g.nodes.map((n) => [n.id, card(n)]));
  layer.append(edgesSvg, ...cards.values());
  dag.append(layer);

  const round = (pts, rad = 6) => {
    let d = `M${pts[0].x},${pts[0].y}`;
    for (let i = 1; i < pts.length - 1; i++) {
      const p = pts[i - 1], c = pts[i], n = pts[i + 1];
      const r1 = Math.min(rad, Math.hypot(c.x - p.x, c.y - p.y) / 2), r2 = Math.min(rad, Math.hypot(n.x - c.x, n.y - c.y) / 2);
      const ux = Math.sign(c.x - p.x), uy = Math.sign(c.y - p.y), vx = Math.sign(n.x - c.x), vy = Math.sign(n.y - c.y);
      d += ` L${c.x - ux * r1},${c.y - uy * r1} Q${c.x},${c.y} ${c.x + vx * r2},${c.y + vy * r2}`;
    }
    const last = pts[pts.length - 1];
    return d + ` L${last.x},${last.y}`;
  };
  const WIDTH = 210;
  let busy = false, again = false;
  const place = async () => {
    if (busy) { again = true; return; }
    busy = true;
    try {
      const ELK = await loadElk();
      for (const c of layer.children) c.style.width = `${WIDTH}px`;
      const graph = {
        id: 'run',
        layoutOptions: {
          'elk.algorithm': 'layered',
          'elk.direction': 'RIGHT',
          'elk.edgeRouting': 'ORTHOGONAL',
          'elk.layered.spacing.nodeNodeBetweenLayers': '56',
          'elk.spacing.nodeNode': '16',
          'elk.spacing.edgeEdge': '8',
          'elk.layered.spacing.edgeNodeBetweenLayers': '16',
          'elk.layered.mergeEdges': 'true',
          'elk.layered.nodePlacement.strategy': 'BRANDES_KOEPF',
          'elk.layered.considerModelOrder.strategy': 'NODES_AND_EDGES',
          'elk.padding': '[top=0,left=0,bottom=0,right=0]',
          // each job as early as what it waits on allows, as the run goes
          'elk.layered.layering.strategy': 'LONGEST_PATH_SOURCE',
          // a job with no lines stays in the picture, not in a corner
          'elk.separateConnectedComponents': 'false',
        },
        children: [...cards].map(([id, c]) => ({ id, width: WIDTH, height: c.offsetHeight })),
        edges: edges.map(([a, b], i) => ({ id: `e${i}`, sources: [a], targets: [b] })),
      };
      const out = await new ELK().layout(graph);
      for (const n of out.children) Object.assign(cards.get(n.id).style, { left: `${n.x}px`, top: `${n.y}px` });
      layer.style.width = `${out.width}px`;
      layer.style.height = `${out.height}px`;
      edgesSvg.setAttribute('width', out.width);
      edgesSvg.setAttribute('height', out.height);
      edgesSvg.innerHTML = out.edges.map((e, i) => {
        const [, b] = edges[i];
        const live = cards.get(b)?.classList.contains('working');
        return (e.sections || []).map((s) => `<path class="${live ? 'live' : ''}" d="${round([s.startPoint, ...(s.bendPoints || []), s.endPoint])}"/>`).join('');
      }).join('');
    } finally {
      busy = false;
      if (again) { again = false; place(); }
    }
  };
  let queued = false;
  const soon = () => { if (!queued) { queued = true; requestAnimationFrame(() => { queued = false; if (dag.isConnected) place(); }); } };
  if (typeof ResizeObserver !== 'undefined') {
    const watcher = new ResizeObserver(() => (dag.isConnected ? soon() : watcher.disconnect()));
    for (const c of cards.values()) watcher.observe(c);
    new MutationObserver((ms) => {
      for (const m of ms) for (const e of m.addedNodes) if (e.classList?.contains('job')) { cards.set(e.dataset.id, e); watcher.observe(e); }
      soon();
    }).observe(layer, { childList: true });
    dag.stop = () => watcher.disconnect();
  }
  soon();
  return dag;
}

// elk.js, once, when a run's graph is first drawn
let elk = null;
function loadElk() {
  elk ||= new Promise((ok, fail) => {
    const s = document.createElement('script');
    s.src = '/_dd/web/elk.js';
    s.onload = () => ok(window.ELK);
    s.onerror = fail;
    document.head.append(s);
  });
  return elk;
}

// ---- a job's log

function logLines(src) {
  return src.replace(/\n$/, '').split('\n').map((l) => {
    const m = l.match(/^(\d{4}-\d\d-\d\dT[\d:.]+Z) (.*)$/);
    const t = m ? m[2] : l;
    const cls = /^::group::|^##\[group\]/.test(t) ? 'g' : /\b(error|failed|panicked)\b/i.test(t) ? 'e' : '';
    return el('div', { class: cls }, m ? el('span', { class: 't', text: `${m[1].slice(11, 19)} ` }) : null, t.replace(/^::group::|^##\[group\]/, '▸ '));
  });
}

route(/^\/([^/]+)\/([^/]+)\/actions\/runs\/(\d+)(?:\/jobs\/(\d+))?$/, async ({ m, params, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'actions');
  const index = Number(m[3]);
  const jobParam = params.get('job') ?? m[4];
  const view = jobParam !== null && jobParam !== undefined ? 'log' : params.get('view') || 'graph';
  let run = await runByIndex(r, index);
  if (!current()) return;
  setTitle(`Run ${index}`, r.full_name);
  // the graph only in its own view, and asked for alongside the jobs
  // the log view asks the forge now: whether to keep reading the log hangs on it
  let [jobs, g] = await Promise.all([jobsOf(r, run, view === 'log'), view === 'graph' ? graph(r, run).catch(() => null) : null]);
  const pane = el('div');
  const summary = el('div', { class: 'box gap' });
  const drawSummary = () => summary.replaceChildren(el('div', { class: 'hrow', style: 'padding:10px 14px;gap:22px' },
    el('span', {}, el('span', { class: 'small muted', text: 'Status' }), el('br'), el('span', { class: 'state-word', 'data-state': stateOf(run.status) || 'off', text: run.status[0].toUpperCase() + run.status.slice(1) })),
    el('span', {}, el('span', { class: 'small muted', text: 'Started' }), el('br'), when(run.started || run.created), run.trigger_user ? el('span', { class: 'small muted', text: ` by ${run.trigger_user.login}` }) : null),
    el('span', {}, el('span', { class: 'small muted', text: DONE.includes(run.status) ? 'Took' : 'So far' }), el('br'), el('b', { text: duration(DONE.includes(run.status) ? ns(run.duration) : Date.now() - new Date(run.started || run.created)) })),
    el('span', {}, el('span', { class: 'small muted', text: 'Commit' }), el('br'), el('a', { class: 'mono', href: `/${r.full_name}/commit/${run.commit_sha}`, text: short(run.commit_sha) }), ' ', el('span', { class: 'small', text: run.prettyref || '' })),
    el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: `${jobs.filter((j) => DONE.includes(j.state)).length} of ${jobs.length} jobs done` })));
  const seg = el('span', { class: 'gseg', role: 'group', 'aria-label': 'View' },
    el('a', { class: 'btn', href: `/${r.full_name}/actions/runs/${index}`, 'aria-current': view === 'graph' ? 'page' : null, text: 'Graph' }),
    el('a', { class: 'btn', href: `/${r.full_name}/actions/runs/${index}${q({ job: jobs.find((j) => j.state === 'failure')?.index ?? jobs.find((j) => j.state === 'running')?.index ?? jobs[0]?.index ?? 0 })}`, 'aria-current': view === 'log' ? 'page' : null, text: 'Jobs and logs' }));
  if (!current()) return;
  put(main(),
    el('div', { class: 'hrow gap' }, el('a', { href: `/${r.full_name}/actions`, text: '← All runs' }), el('h1', { class: 'h1', style: 'font-size:19px', text: `Run ${index} · ${run.title}` }), el('span', { class: 'spacer' }), seg),
    summary, pane);
  drawSummary();

  let timer = null;
  const alive = () => current() && pane.isConnected;
  if (view === 'graph') {
    // drawn again as its jobs move (watch, below): where it was scrolled to stays
    const draw = () => {
      const was = pane.querySelector('.dag');
      const x = was?.scrollLeft || 0;
      was?.stop?.();
      pane.replaceChildren(el('div', { class: 'box' },
      el('header', {}, el('b', { text: run.workflow_id }), el('span', { class: 'small muted', text: `on: ${run.event}` }), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: 'a job starts when every job before it has passed' })),
      g ? drawGraph(r, run, g, jobs) : el('div', { class: 'empty', text: 'The workflow file could not be read; the jobs are under Jobs and logs.' })));
      const now = pane.querySelector('.dag');
      if (now) now.scrollLeft = x;
    };
    draw();
    // this run's jobs moving, as the forge records them
    watch(r.id, alive, async (changes) => {
      if (!changes.some((c) => c.run === run.id || (c.kind === 'status' && c.sha === run.commit_sha))) return;
      try {
        [run, jobs] = await Promise.all([api(`/repos/${r.full_name}/actions/runs/${run.id}`, { fresh: true }), jobsOf(r, run, true)]);
        if (!alive()) return;
        drawSummary();
        draw();
      } catch { /* the next change asks again */ }
    });
    return;
  }

  // jobs and a log
  const job = Number(jobParam);
  const log = el('div', { class: 'log' }, ghost(4));
  const follow = el('input', { type: 'checkbox', checked: true });
  const side = el('aside', { class: 'jobs box', style: 'padding:8px' });
  const head = el('header');
  const raw = el('a', { class: 'btn plain', target: '_blank', rel: 'noopener', text: 'Raw' });
  const drawJobs = () => {
    const me = jobs.find((j) => j.index === job);
    side.replaceChildren(...jobs.map((j) => el('a', { href: `/${r.full_name}/actions/runs/${index}${q({ job: j.index })}`, 'aria-current': j.index === job ? 'page' : null }, statusDot(j.state), el('span', { style: 'flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap', text: j.leaf }), el('small', { class: 'muted' }, took(j)))));
    head.replaceChildren(statusDot(me?.state), el('b', { text: me?.leaf || `Job ${job}` }), el('span', { class: 'muted small' }, me?.started ? took(me) : me?.description || ''), el('span', { class: 'spacer' }), el('label', { class: 'small hrow', style: 'gap:4px' }, follow, 'Follow'), raw);
  };
  drawJobs();
  pane.replaceChildren(el('div', { class: 'two left', style: 'grid-template-columns:280px minmax(0,1fr)' }, side,
    el('div', { class: 'box', style: 'min-width:0' }, head, log)));
  // the forge's changes move the list; the log is asked for while the job runs
  watch(r.id, alive, async (changes) => {
    if (!changes.some((c) => c.run === run.id || (c.kind === 'status' && c.sha === run.commit_sha))) return;
    try {
      [run, jobs] = await Promise.all([api(`/repos/${r.full_name}/actions/runs/${run.id}`, { fresh: true }), jobsOf(r, run, true)]);
      if (!alive()) return;
      drawSummary();
      drawJobs();
      if (!timer && pull) pull();
    } catch { /* the next change asks again */ }
  });
  // the latest attempt: each one the forge has a log for, asked all at once
  const tries = await Promise.all([2, 3, 4, 5].map((n) => fetch(`/${r.full_name}/actions/runs/${index}/jobs/${job}/attempt/${n}/logs`, { method: 'HEAD', credentials: 'same-origin' }).then((x) => x.ok).catch(() => false)));
  if (!alive()) return;
  const attempt = tries.lastIndexOf(true) + 2 || 1;
  const url = `/${r.full_name}/actions/runs/${index}/jobs/${job}/attempt/${attempt}/logs`;
  raw.href = url;
  // only what is new each time: the forge answers a range
  let bytes = 0, pending = '', asking = false;
  const utf8 = new TextDecoder();
  const pull = async () => {
    timer = null;
    if (!alive() || asking) return;
    asking = true;
    const res = await fetch(url, { credentials: 'same-origin', headers: bytes ? { range: `bytes=${bytes}-` } : {} }).catch(() => null);
    asking = false;
    if (!alive()) return;
    if (res?.status === 404 && !bytes) {
      log.replaceChildren(el('div', { class: 't', text: 'No log yet: the job is waiting for a runner, or for the jobs before it.' }));
    } else if (res && (res.status === 200 || res.status === 206)) {
      const chunk = new Uint8Array(await res.arrayBuffer());
      // the whole log again (the forge ignored the range): drawn afresh
      if (res.status === 200 && bytes) { bytes = 0; pending = ''; }
      if (chunk.length) {
        if (!bytes) log.replaceChildren();
        bytes += chunk.length;
        // a line is drawn once it is whole
        pending += utf8.decode(chunk, { stream: true });
        const cut = pending.lastIndexOf('\n') + 1;
        if (cut) {
          log.append(...logLines(pending.slice(0, cut)));
          pending = pending.slice(cut);
          if (follow.checked) log.scrollTop = log.scrollHeight;
        }
      }
    }
    const me = jobs.find((j) => j.index === job);
    if (!me || !DONE.includes(me.state)) timer = setTimeout(pull, 3000);
    else if (pending) { log.append(...logLines(pending)); pending = ''; }
  };
  pull();
});
