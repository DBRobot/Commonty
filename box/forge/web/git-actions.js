// Actions: the runs, and one run as a graph of its jobs - read from the
// workflow files' `needs:` - or as its jobs and their logs, both live.

import { put, app, el, ic, api, text, q, when, plural, duration, avatar, route, go, statusDot, setTitle, short, pager, warmers } from './git-core.js';
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

const runsData = (r, page = 1) => api(`/repos/${r.full_name}/actions/runs${q({ page, limit: 25 })}`);
warmers.actions = (r) => runsData(r);

route(/^\/([^/]+)\/([^/]+)\/actions$/, async ({ m, params, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'actions');
  setTitle('Actions', r.full_name);
  const page = Number(params.get('page') || 1);
  const workflow = params.get('workflow') || '';
  const { workflow_runs: runs = [], total_count: total = 0 } = await runsData(r, page);
  if (!current()) return;
  const flows = [...new Set(runs.map((x) => x.workflow_id))].sort();
  const shown = workflow ? runs.filter((x) => x.workflow_id === workflow) : runs;
  const row = (x) => el('a', { class: 'item', href: `/${r.full_name}/actions/runs/${x.index_in_repo}` },
    el('span', { style: 'margin-top:6px' }, statusDot(x.status)),
    el('div', { style: 'flex:1;min-width:0' }, el('div', { class: 't', text: x.title }),
      el('div', { class: 'sub' }, `Run ${x.index_in_repo} · ${x.workflow_id} · ${x.prettyref || ''} · ${x.event} · `, x.trigger_user?.login || '', ' · ', el('span', { class: 'mono', text: short(x.commit_sha) }))),
    el('div', { class: 'small muted', style: 'text-align:right' }, when(x.created), el('br'), x.status === 'running' ? 'running' : x.duration ? duration(ns(x.duration)) : x.status));
  if (!current()) return;
  put(main(), el('div', { class: 'two left' },
    el('aside', { class: 'jobs' },
      el('a', { href: `/${r.full_name}/actions`, 'aria-current': !workflow ? 'page' : null, text: 'All runs' }),
      ...flows.map((f) => el('a', { href: `/${r.full_name}/actions${q({ workflow: f })}`, 'aria-current': workflow === f ? 'page' : null, text: f.replace(/\.ya?ml$/, '') }))),
    el('div', {},
      el('div', { class: 'box' }, el('header', {}, el('b', { text: workflow || 'All runs' }), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: plural(total, 'run') })),
        el('div', { class: 'list' }, ...(shown.length ? shown.map(row) : [el('div', { class: 'empty', text: 'No runs yet.' })]))),
      pager(page, total, 25, (p) => go(`/${r.full_name}/actions${q({ page: p, workflow })}`)))));
});

// ---- one run

// the run by its number: the forge asks by id, which runs a fixed distance
// ahead of the number on this forge; check, and look further if not
async function runByIndex(r, index) {
  const { workflow_runs: first = [] } = await api(`/repos/${r.full_name}/actions/runs${q({ limit: 1 })}`);
  if (first[0]) {
    const guess = await api(`/repos/${r.full_name}/actions/runs/${index + first[0].id - first[0].index_in_repo}`).catch(() => null);
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
  const jobs = new Map();
  for (const s of all.sort((a, b) => a.id - b.id)) {
    const m = (s.target_url || '').match(/\/actions\/runs\/(\d+)\/jobs\/(\d+)/);
    if (!m || Number(m[1]) !== run.index_in_repo) continue;
    const name = s.context.replace(/ \((pull_request|push|schedule|workflow_dispatch)\)$/, '');
    const prev = jobs.get(Number(m[2]));
    // when it began and ended, from the statuses the forge posted as it went
    const started = /started running/i.test(s.description || '') ? s.created_at : prev?.started;
    const ended = s.status !== 'pending' ? s.created_at : null;
    // the forge says pending for waiting, blocked and running alike: its
    // words tell them apart, and only a job a runner has picked up is running
    const state = s.status !== 'pending' ? s.status
      : /started running/i.test(s.description || '') ? 'running'
      : /blocked/i.test(s.description || '') ? 'blocked' : 'waiting';
    jobs.set(Number(m[2]), { index: Number(m[2]), name, leaf: name.split(' / ').pop(), state, description: s.description, started, ended });
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
  // depth: the longest line of waiting before a job
  const depth = new Map();
  const d = (id, seen = new Set()) => {
    if (depth.has(id)) return depth.get(id);
    if (seen.has(id)) return 0;
    seen.add(id);
    const v = Math.max(0, ...edges.filter(([, b]) => b === id).map(([a]) => d(a, seen) + 1));
    depth.set(id, v);
    return v;
  };
  nodes.forEach((n) => d(n.id));
  return { nodes, edges, depth };
}

// a job's instances: "vm_tests (games)" belongs to the node vm_tests
const nodeOf = (leaf) => leaf.replace(/ \(.*\)$/, '');
function rollup(jobs) {
  const s = jobs.map((j) => j.state);
  if (s.some((x) => x === 'failure' || x === 'error')) return 'failure';
  if (s.some((x) => x === 'running')) return 'running';
  if (s.some((x) => x === 'pending' || x === 'waiting' || x === 'blocked')) return 'waiting';
  if (s.length && s.every((x) => x === 'success' || x === 'skipped')) return 'success';
  return s[0] || 'waiting';
}

function drawGraph(r, run, g, jobs) {
  const dag = el('div', { class: 'dag' });
  const edgesSvg = document.createElementNS('http://www.w3.org/2000/svg', 'svg');
  edgesSvg.setAttribute('class', 'edges');
  const byNode = new Map(g.nodes.map((n) => [n.id, []]));
  for (const j of jobs) (byNode.get(nodeOf(j.leaf)) || byNode.get(j.leaf))?.push(j);
  const cols = new Map();
  for (const n of g.nodes) {
    const k = g.depth.get(n.id);
    if (!cols.has(k)) cols.set(k, []);
    cols.get(k).push(n);
  }
  const card = (n) => {
    const js = byNode.get(n.id) || [];
    const state = js.length ? rollup(js) : 'waiting';
    const cls = `job${state === 'running' ? ' busy' : state === 'failure' ? ' bad' : ''}`;
    const one = js.length === 1 && js[0].leaf === n.id;
    const head = el(one ? 'a' : 'div', one ? { href: `/${r.full_name}/actions/runs/${run.index_in_repo}${q({ job: js[0].index })}` } : {},
      statusDot(state), el('span', { class: 't', text: n.id }), js.length > 1 ? el('small', { text: `${js.filter((j) => DONE.includes(j.state)).length} of ${js.length}` }) : el('small', {}, js[0]?.started ? took(js[0]) : js[0] ? (js[0].state === 'blocked' ? 'blocked' : DONE.includes(js[0].state) ? '' : 'waiting') : ''));
    return el('div', { class: cls, 'data-id': n.id }, head,
      ...(js.length > 1 || (js.length === 1 && !one) ? js.map((j) => el('a', { class: 'sub', href: `/${r.full_name}/actions/runs/${run.index_in_repo}${q({ job: j.index })}` }, statusDot(j.state), el('span', { style: 'flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap', text: j.leaf.replace(`${n.id} `, '') || j.leaf }), el('small', {}, took(j))) ) : []));
  };
  const colEls = [...cols.keys()].sort((a, b) => a - b).map((k) => {
    const groups = [...new Set(cols.get(k).map((n) => n.group))];
    return el('div', { class: 'col' }, ...groups.flatMap((gname) => [gname && groups.length > 0 ? el('div', { class: 'grp', text: gname }) : null, ...cols.get(k).filter((n) => n.group === gname).map(card)]));
  });
  dag.append(edgesSvg, el('div', { class: 'cols' }, ...colEls));
  const drawEdges = () => {
    if (!dag.isConnected) return;
    const box = dag.getBoundingClientRect();
    const at = (id) => dag.querySelector(`[data-id="${CSS.escape(id)}"]`)?.getBoundingClientRect();
    edgesSvg.innerHTML = g.edges.map(([a, b]) => {
      const p = at(a), qb = at(b);
      if (!p || !qb) return '';
      const x1 = p.right - box.left + dag.scrollLeft, y1 = p.top + 18 - box.top;
      const x2 = qb.left - box.left + dag.scrollLeft, y2 = qb.top + 18 - box.top, mid = (x1 + x2) / 2;
      const live = dag.querySelector(`[data-id="${CSS.escape(b)}"]`)?.classList.contains('busy');
      return `<path class="${live ? 'live' : ''}" d="M${x1},${y1} C${mid},${y1} ${mid},${y2} ${x2},${y2}"/>`;
    }).join('');
  };
  requestAnimationFrame(drawEdges);
  const onResize = () => (dag.isConnected ? drawEdges() : removeEventListener('resize', onResize));
  addEventListener('resize', onResize);
  return dag;
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

async function attemptOf(r, run, job) {
  let a = 1;
  for (let n = 2; n <= 5; n++) {
    const ok = await fetch(`/${r.full_name}/actions/runs/${run.index_in_repo}/jobs/${job}/attempt/${n}/logs`, { method: 'HEAD', credentials: 'same-origin' }).then((x) => x.ok).catch(() => false);
    if (!ok) break;
    a = n;
  }
  return a;
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
  let jobs = await jobsOf(r, run);
  const g = await graph(r, run).catch(() => null);
  const pane = el('div');
  const summary = el('div', { class: 'box gap' });
  const drawSummary = () => summary.replaceChildren(el('div', { class: 'hrow', style: 'padding:10px 14px;gap:22px' },
    el('span', {}, el('span', { class: 'small muted', text: 'Status' }), el('br'), el('b', { style: `color:var(--${run.status === 'success' ? 'ok' : run.status === 'failure' ? 'bad' : 'busy'})`, text: run.status[0].toUpperCase() + run.status.slice(1) })),
    el('span', {}, el('span', { class: 'small muted', text: 'Started' }), el('br'), when(run.started || run.created), run.trigger_user ? el('span', { class: 'small muted', text: ` by ${run.trigger_user.login}` }) : null),
    el('span', {}, el('span', { class: 'small muted', text: DONE.includes(run.status) ? 'Took' : 'So far' }), el('br'), el('b', { text: duration(DONE.includes(run.status) ? ns(run.duration) : Date.now() - new Date(run.started || run.created)) })),
    el('span', {}, el('span', { class: 'small muted', text: 'Commit' }), el('br'), el('a', { class: 'mono', href: `/${r.full_name}/commit/${run.commit_sha}`, text: short(run.commit_sha) }), ' ', el('span', { class: 'small', text: run.prettyref || '' })),
    el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: `${jobs.filter((j) => DONE.includes(j.state)).length} of ${jobs.length} jobs done` })));
  const seg = el('span', { class: 'gseg', role: 'group', 'aria-label': 'View' },
    el('a', { class: 'btn', href: `/${r.full_name}/actions/runs/${index}`, 'aria-pressed': String(view === 'graph'), text: 'Graph' }),
    el('a', { class: 'btn', href: `/${r.full_name}/actions/runs/${index}${q({ job: jobs.find((j) => j.state === 'failure')?.index ?? jobs.find((j) => j.state === 'running')?.index ?? jobs[0]?.index ?? 0 })}`, 'aria-pressed': String(view === 'log'), text: 'Jobs and logs' }));
  if (!current()) return;
  put(main(), 
    el('div', { class: 'hrow gap' }, el('a', { href: `/${r.full_name}/actions`, text: '← All runs' }), el('h1', { class: 'h1', style: 'font-size:19px', text: `Run ${index} · ${run.title}` }), el('span', { class: 'spacer' }), seg),
    summary, pane);
  drawSummary();

  let timer = null;
  const alive = () => current() && pane.isConnected;
  if (view === 'graph') {
    // redrawn every few seconds while it runs: where it was scrolled to stays
    const draw = () => {
      const was = pane.querySelector('.dag');
      const x = was?.scrollLeft || 0;
      pane.replaceChildren(el('div', { class: 'box' },
      el('header', {}, el('b', { text: run.workflow_id }), el('span', { class: 'small muted', text: `on: ${run.event}` }), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: 'a job starts when every job before it has passed' })),
      g ? drawGraph(r, run, g, jobs) : el('div', { class: 'empty', text: 'The workflow file could not be read; the jobs are under Jobs and logs.' })));
      const now = pane.querySelector('.dag');
      if (now) now.scrollLeft = x;
    };
    draw();
    const tick = async () => {
      if (!alive() || DONE.includes(run.status)) return;
      [run, jobs] = await Promise.all([api(`/repos/${r.full_name}/actions/runs/${run.id}`, { fresh: true }), jobsOf(r, run, true)]);
      if (!alive()) return;
      drawSummary();
      draw();
      timer = setTimeout(tick, 5000);
    };
    timer = setTimeout(tick, 5000);
    return;
  }

  // jobs and a log
  const job = Number(jobParam);
  const current_ = jobs.find((j) => j.index === job);
  const side = el('aside', { class: 'jobs box', style: 'padding:8px' }, ...jobs.map((j) => el('a', { href: `/${r.full_name}/actions/runs/${index}${q({ job: j.index })}`, 'aria-current': j.index === job ? 'page' : null }, statusDot(j.state), el('span', { style: 'flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis;white-space:nowrap', text: j.leaf }), el('small', { class: 'muted' }, took(j)))));
  const log = el('div', { class: 'log' }, el('div', { class: 't', text: 'Loading the log…' }));
  const follow = el('input', { type: 'checkbox', checked: true });
  const attempt = await attemptOf(r, run, job);
  const url = `/${r.full_name}/actions/runs/${index}/jobs/${job}/attempt/${attempt}/logs`;
  pane.replaceChildren(el('div', { class: 'two left', style: 'grid-template-columns:280px minmax(0,1fr)' }, side,
    el('div', { class: 'box', style: 'min-width:0' },
      el('header', {}, statusDot(current_?.state), el('b', { text: current_?.leaf || `Job ${job}` }), el('span', { class: 'muted small' }, current_?.started ? took(current_) : current_?.description || ''), el('span', { class: 'spacer' }), el('label', { class: 'small hrow', style: 'gap:4px' }, follow, 'Follow'), el('a', { class: 'btn plain', href: url, target: '_blank', rel: 'noopener', text: 'Raw' })),
      log)));
  let shown = 0;
  const pull = async () => {
    if (!alive()) return;
    const src = await text(url).catch((e) => (e.status === 404 ? null : ''));
    if (src === null) {
      log.replaceChildren(el('div', { class: 't', text: 'No log yet: the job is waiting for a runner, or for the jobs before it.' }));
    } else if (src.length !== shown) {
      if (!shown) log.replaceChildren();
      log.append(...logLines(src.slice(shown)));
      shown = src.length;
      if (follow.checked) log.scrollTop = log.scrollHeight;
    }
    jobs = await jobsOf(r, run, true);
    const me = jobs.find((j) => j.index === job);
    if (me && !DONE.includes(me.state)) timer = setTimeout(pull, 3000);
  };
  pull();
});
