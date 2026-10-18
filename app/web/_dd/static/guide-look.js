// What Monty may look up of the member's own: their Git (runs, pull
// requests, a failed job's log), their boxes and their storage - what the
// Git pages and Settings already show them, read with their own session,
// read only, and never what only their devices can open (privacy.md: no
// file, film, device or disk-image names, no passwords). Everything comes
// back as plain words and numbers for guide.js to say; nothing here writes
// to the page.

const REPO = /^[A-Za-z0-9_-][A-Za-z0-9_.-]{0,99}\/[A-Za-z0-9_-][A-Za-z0-9_.-]{0,99}$/;
// a terminal's colour codes: ESC, [, numbers, a letter
const ANSI = new RegExp(`${String.fromCharCode(27)}\\[[0-9;]*[A-Za-z]`, 'g');
const clean = (s, n = 200) => (typeof s === 'string' ? [...s].filter((c) => c >= ' ' && c !== '\x7f').join('').trim().slice(0, n) : '');

async function json(url, init = {}) {
  // no-cache, as the map (guide.js): a kept redirect is asked again
  const r = await fetch(url, { credentials: 'include', cache: 'no-cache', headers: { accept: 'application/json' }, ...init }).catch(() => null);
  if (!r) return { error: 'down' };
  if (r.status === 401) return { error: 'signin' };
  if (r.status === 403 || r.status === 404) return { error: 'missing' };
  if (!r.ok) return { error: 'down' };
  try { return { json: await r.json() }; } catch { return { error: 'down' }; }
}

/// how long ago, in words
export function ago(t, now = Date.now()) {
  const ms = typeof t === 'number' ? t * 1000 : Date.parse(t);
  if (!Number.isFinite(ms)) return '';
  const m = Math.round((now - ms) / 60e3);
  if (m < 1) return 'just now';
  if (m < 60) return `${m} minute${m === 1 ? '' : 's'} ago`;
  const h = Math.round(m / 60);
  if (h < 48) return `${h} hour${h === 1 ? '' : 's'} ago`;
  return `${Math.round(h / 24)} days ago`;
}

// ---- Git

/// The newest run of a repository: its number, title, state and when
export async function latestRun(git, repo) {
  if (!REPO.test(repo)) return { error: 'missing' };
  const { json: j, error } = await json(`${git}/api/v1/repos/${repo}/actions/runs?page=1&limit=10`);
  if (error) return { error };
  const runs = (Array.isArray(j?.workflow_runs) ? j.workflow_runs : []).filter((r) => Number.isInteger(r?.index_in_repo));
  if (!runs.length) return { none: true };
  return { run: shape(runs[0]), failed: runs.map(shape).find((r) => r.state === 'failure') || null };
}

const shape = (r) => ({
  n: r.index_in_repo,
  title: clean(r.title, 120),
  state: clean(r.status, 20),
  when: r.created,
  sha: typeof r.commit_sha === 'string' && /^[0-9a-f]{7,64}$/.test(r.commit_sha) ? r.commit_sha : null,
});

/// The first job of a run that failed, from the commit's statuses (as the
/// Git pages read them, git-actions.js jobsOf)
export async function failedJob(git, repo, run) {
  if (!run.sha) return null;
  const { json: s } = await json(`${git}/api/v1/repos/${repo}/commits/${run.sha}/statuses?page=1&limit=50`);
  const mine = (Array.isArray(s) ? s : [])
    .map((x) => ({ x, m: (x?.target_url || '').match(/\/actions\/runs\/(\d+)\/jobs\/(\d+)/) }))
    .filter(({ m }) => m && Number(m[1]) === run.n)
    .sort((a, b) => b.x.id - a.x.id);
  const bad = mine.find(({ x }) => x.status === 'failure' || x.status === 'error');
  return bad ? { index: Number(bad.m[2]), name: clean(bad.x.context, 120).replace(/ \((pull_request|push|schedule|workflow_dispatch)\)$/, '') } : null;
}

/// The end of a failed job's log: the last lines, colours taken out
export async function logTail(git, repo, run, job, lines = 60) {
  const r = await fetch(`${git}/${repo}/actions/runs/${run.n}/jobs/${job.index}/attempt/1/logs`, { credentials: 'include' }).catch(() => null);
  if (!r?.ok) return '';
  const text = (await r.text()).replace(ANSI, '');
  return text.split('\n').filter((l) => l.trim()).slice(-lines).join('\n').slice(-6000);
}

/// Pull requests waiting on the member's review, and issues given to them
export async function waiting(git) {
  const q = (o) => new URLSearchParams(o).toString();
  const [prs, mine] = await Promise.all([
    json(`${git}/api/v1/repos/issues/search?${q({ type: 'pulls', state: 'open', review_requested: true, limit: 10 })}`),
    json(`${git}/api/v1/repos/issues/search?${q({ type: 'pulls', state: 'open', created: true, limit: 10 })}`),
  ]);
  if (prs.error && mine.error) return { error: prs.error };
  const one = (x) => (REPO.test(x?.repository?.full_name || '') && Number.isInteger(x?.number)
    ? { repo: x.repository.full_name, n: x.number, title: clean(x.title, 120), when: x.updated_at } : null);
  return {
    review: (Array.isArray(prs.json) ? prs.json : []).map(one).filter(Boolean),
    mine: (Array.isArray(mine.json) ? mine.json : []).map(one).filter(Boolean),
  };
}

// ---- boxes and storage

/// The household's boxes: answering or not, and each one's updates and
/// backups (as Settings, Network, shows them)
export async function boxes() {
  const [h, f] = await Promise.all([json('/_dd/house'), json('/_dd/fleet.json')]);
  if (h.error) return { error: h.error };
  const fleet = Object.fromEntries((Array.isArray(f.json) ? f.json : []).filter((x) => typeof x?.name === 'string').map((x) => [x.name, x]));
  const newest = Math.max(0, ...Object.values(fleet).filter((x) => x.up && Number.isInteger(x.release)).map((x) => x.release));
  const list = (Array.isArray(h.json?.boxes) ? h.json.boxes : []).filter((b) => typeof b?.box === 'string').map((b) => {
    const x = fleet[b.box] || {};
    const backed = x.backup?.last_success;
    return {
      name: clean(b.box, 40),
      up: !b.unreachable,
      behind: Number.isInteger(x.release) && x.release < newest,
      failed: typeof x.result === 'string' && x.result !== 'ok' ? clean(x.result, 40) : null,
      backed: typeof backed === 'number' ? backed : null,
    };
  });
  return { list };
}

/// The words for the boxes: what is wrong first, or that all is well
export function boxesSay(list, now = Date.now()) {
  if (!list.length) return 'This home has no boxes yet.';
  const lines = list.map((b) => {
    if (!b.up) return `${b.name} is not answering.`;
    const bad = [];
    if (b.behind) bad.push('behind on updates');
    else if (b.failed) bad.push(`its last update ${b.failed}`);
    if (!b.backed) bad.push('never backed up');
    else if (now / 1000 - b.backed > 2 * 86400) bad.push(`not backed up for ${Math.floor((now / 1000 - b.backed) / 86400)} days`);
    return bad.length ? `${b.name} is up, but ${bad.join(' and ')}.` : `${b.name} is fine: up to date, backed up ${ago(b.backed, now)}.`;
  });
  return lines.join('\n');
}

const size = (b) => (b >= 1e12 ? `${(b / 1e12).toFixed(1)} TB` : b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : b >= 1e6 ? `${Math.round(b / 1e6)} MB` : `${Math.round(b / 1e3)} KB`);

/// The member's storage, in words: used, of what, and what is left
export async function storageSay() {
  const { json: d, error } = await json('/_dd/storage/mine');
  if (error) return { error };
  if (!d?.counted) return { text: 'Your storage has not been counted yet. It is counted every hour.' };
  const used = Number(d.used) || 0;
  const budget = Number(d.budget) || 0;
  const pct = budget ? Math.round((used / budget) * 100) : 0;
  return { text: budget ? `You have used ${size(used)} of ${size(budget)} (${pct}%), so ${size(Math.max(0, budget - used))} is left.${pct >= 90 ? ' That is nearly full.' : ''}` : `You have used ${size(used)}.` };
}

/// The newest release the boxes run, for a bug report
export async function release() {
  const { json: f } = await json('/_dd/fleet.json');
  const r = (Array.isArray(f) ? f : []).filter((x) => x?.up && Number.isInteger(x.release)).map((x) => x.release);
  return r.length ? Math.max(...r) : null;
}

/// The browser, in a few words: name, version and system, nothing more
export function browser(ua = navigator.userAgent) {
  const m = (re) => (re.exec(ua) || [])[1];
  const v = [['Edge', /Edg\/(\d+)/], ['Firefox', /Firefox\/(\d+)/], ['Chrome', /Chrome\/(\d+)/], ['Safari', /Version\/(\d+).*Safari/]].find(([, re]) => m(re));
  const name = v ? `${v[0]} ${m(v[1])}` : 'a browser';
  const os = /Android/.test(ua) ? 'Android' : /iPhone|iPad/.test(ua) ? 'iOS' : /Mac OS X/.test(ua) ? 'macOS' : /Windows/.test(ua) ? 'Windows' : /Linux/.test(ua) ? 'Linux' : '';
  return os ? `${name} on ${os}` : name;
}
