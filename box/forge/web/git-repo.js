// One repository: its header and tabs, the code and a file with the tree
// beside it, commits and a commit, branches, releases, and its activity.

import {
  put, $, app, el, ic, api, text, q, enc, ago, when, plural, bytes, avatar, route, go, statusDot,
  markdown, toast, copy, setTitle, firstLine, short, pop, pager, whoami, warmers } from './git-core.js';
import { heatmap, lineChart, languages, dayKey } from './git-charts.js';
import * as diff from './git-diff.js';
import { feedLine } from './git-home.js';

const main = () => app();

// ---- the repository, asked once per visit and kept

const repos = new Map();
export async function repo(owner, name, fresh = false) {
  const k = `${owner}/${name}`.toLowerCase();
  if (fresh || !repos.has(k)) repos.set(k, api(`/repos/${enc(owner)}/${enc(name)}`));
  try { return await repos.get(k); } catch (e) { repos.delete(k); throw e; }
}
export const forget = (r) => repos.delete(r.full_name.toLowerCase());
const refsOf = new Map();
async function refs(r) {
  if (!refsOf.has(r.full_name)) {
    refsOf.set(r.full_name, Promise.all([
      api(`/repos/${r.full_name}/branches${q({ limit: 100 })}`).catch(() => []),
      api(`/repos/${r.full_name}/tags${q({ limit: 100 })}`).catch(() => []),
    ]).then(([branches, tags]) => ({ branches, tags })));
  }
  return refsOf.get(r.full_name);
}

// "main/src/lib.rs" or "feature/x/src/lib.rs": the longest branch or tag
// name the path starts with is the ref
async function split(r, rest) {
  if (!rest) return { ref: r.default_branch, path: '' };
  const { branches, tags } = await refs(r);
  const names = [...branches.map((b) => b.name), ...tags.map((t) => t.name)].sort((a, b) => b.length - a.length);
  const hit = names.find((n) => rest === n || rest.startsWith(`${n}/`));
  if (hit) return { ref: hit, path: rest.slice(hit.length + 1) };
  const [ref, ...p] = rest.split('/');
  return { ref, path: p.join('/') };
}

// ---- the header: name, then the sections as tabs

// the tabs with the most to fetch start fetching when the pointer arrives
const warm = (a, fetchIt) => { a.addEventListener('pointerenter', () => { fetchIt().catch(() => {}); }, { once: true }); return a; };

export function header(r, tab) {
  const [owner, name] = r.full_name.split('/');
  const t = (key, href, icon, label, n) => {
    const a = el('a', { href, 'aria-current': tab === key ? 'page' : null }, ic(icon), label, n ? el('span', { class: 'n', text: String(n) }) : null);
    return warmers[key] ? warm(a, () => warmers[key](r)) : a;
  };
  $('rhead').replaceChildren(el('div', { class: 'rhead' }, el('div', { class: 'in' },
    el('div', { class: 'crumb' }, ic(r.private ? 'lock' : 'repo'), el('a', { href: `/${owner}`, text: owner }), el('span', { class: 'muted', text: '/' }), el('b', {}, el('a', { href: `/${r.full_name}`, text: name })),
      el('span', { class: 'pill', text: r.archived ? 'Archived' : r.private ? 'Private' : 'Public' }),
      r.mirror ? el('span', { class: 'pill', text: 'Mirror' }) : null),
    el('nav', { class: 'rtabs', 'aria-label': 'Repository' },
      t('code', `/${r.full_name}`, 'code', 'Code'),
      r.has_issues ? t('issues', `/${r.full_name}/issues`, 'issue', 'Issues', r.open_issues_count) : null,
      r.has_pull_requests ? t('pulls', `/${r.full_name}/pulls`, 'pr', 'Pull requests', r.open_pr_counter) : null,
      r.has_actions ? t('actions', `/${r.full_name}/actions`, 'play', 'Actions') : null,
      r.has_releases ? t('releases', `/${r.full_name}/releases`, 'tag', 'Releases', r.release_counter) : null,
      t('activity', `/${r.full_name}/activity`, 'pulse', 'Activity'),
      r.permissions?.admin ? t('settings', `/${r.full_name}/settings`, 'gear', 'Settings') : null))));
}

// the branch and tag picker
function refPicker(r, ref, to) {
  const b = el('button', { class: 'btn', type: 'button' }, ic('branch'), ref.length > 24 ? `${ref.slice(0, 11)}…${ref.slice(-10)}` : ref, ic('down', 'i s'));
  return pop(b, async (close) => {
    const { branches, tags } = await refs(r);
    const input = el('input', { placeholder: 'Find a branch or tag', 'aria-label': 'Find a branch or tag' });
    const items = [...branches.map((x) => ['branch', x.name]), ...tags.map((x) => ['tag', x.name])];
    const list = el('div');
    const draw = () => {
      const s = input.value.trim().toLowerCase();
      list.replaceChildren(...items.filter(([, n]) => !s || n.toLowerCase().includes(s)).slice(0, 60)
        .map(([k, n]) => el('a', { href: to(n), 'aria-current': n === ref ? 'true' : null, onclick: close }, ic(k), n, n === r.default_branch ? el('span', { class: 'chip', style: 'margin-left:auto', text: 'default' }) : null)));
    };
    input.oninput = draw;
    draw();
    return [input, list];
  });
}

function cloneBox(r) {
  const ways = [['HTTPS', r.clone_url], ['SSH', r.ssh_url]];
  let pick = 0;
  const input = el('input', { readonly: true, value: ways[0][1], 'aria-label': 'Clone address' });
  const cp = el('button', { 'aria-label': 'Copy' }, ic('copy'));
  cp.onclick = () => copy(input.value, cp);
  const seg = el('span', { class: 'gseg', style: 'margin-bottom:6px' }, ...ways.map(([t], i) => el('button', { class: 'btn', style: 'padding:2px 10px;font-size:12px', 'aria-pressed': String(i === pick), text: t, onclick: (e) => { pick = i; input.value = ways[i][1]; seg.querySelectorAll('button').forEach((x, j) => x.setAttribute('aria-pressed', String(j === i))); } })));
  return el('section', {}, el('h3', { text: 'Clone' }), seg, el('div', { class: 'clone' }, input, cp));
}

// the repository the boxes are built from: only its pages show what they run
let fleetRepo = null;
let fleetBoxes = null;
let fleetAt = 0;
export const fleetRepoName = () => (fleetRepo ??= fetch('/fleet-repo.json').then((x) => (x.ok ? x.json() : {})).then((x) => x.repo || '').catch(() => ''));
const fleet = async (r) => {
  if ((await fleetRepoName()) !== r.full_name) return [];
  if (!fleetBoxes || performance.now() - fleetAt > 60000) { fleetAt = performance.now(); fleetBoxes = null; }
  fleetBoxes ??= fetch('/_dd/fleet.json').then((x) => (x.ok ? x.json() : [])).catch(() => []);
  const boxes = await fleetBoxes;
  // nothing known of any box: say nothing rather than a row of question marks
  return boxes.some((b) => b.release !== null && b.release !== undefined) ? boxes : [];
};

const rawUrl = (r, ref, path) => `/api/v1/repos/${r.full_name}/raw/${enc(path)}${q({ ref })}`;

// ---- the code

const RENDERED = /^readme(\.(md|markdown|txt))?$/i;

route(/^\/([^/]+)\/([^/]+)(?:\/tree\/(.+))?$/, async ({ m, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'code');
  setTitle(r.full_name);
  if (r.empty) { put(main(), emptyRepo(r)); return; }
  const { ref, path } = await split(r, m[3]);
  const at = path ? `/${enc(path)}` : '';
  const listing = await api(`/repos/${r.full_name}/contents${at}${q({ ref })}`);
  if (!current()) return;
  if (!Array.isArray(listing)) { go(`/${r.full_name}/blob/${ref}/${path}`, true); return; }
  listing.sort((a, b) => (b.type === 'dir') - (a.type === 'dir') || a.name.localeCompare(b.name));

  // each entry's last commit, one question per distinct commit
  const msgs = new Map();
  const rows = listing.map((e) => {
    const msg = el('span', { class: 'msg' });
    const href = e.type === 'dir' ? `/${r.full_name}/tree/${ref}/${e.path}` : e.type === 'submodule' ? (e.submodule_git_url || '#') : `/${r.full_name}/blob/${ref}/${e.path}`;
    if (e.last_commit_sha) (msgs.get(e.last_commit_sha) || msgs.set(e.last_commit_sha, []).get(e.last_commit_sha)).push(msg);
    return el('a', { href }, el('span', { class: 'name' }, ic(e.type === 'dir' ? 'dir' : 'file', `i${e.type === 'dir' ? ' dir' : ''}`), e.name), msg, el('span', { class: 'when', text: e.last_commit_when ? ago(e.last_commit_when) : '' }));
  });
  for (const [sha, spans] of msgs) {
    api(`/repos/${r.full_name}/git/commits/${sha}${q({ stat: false, verification: false, files: false })}`)
      .then((c) => spans.forEach((s) => { s.textContent = firstLine(c.commit.message); }))
      .catch(() => {});
  }

  const crumbs = path ? el('span', {}, el('a', { href: `/${r.full_name}/tree/${ref}`, text: r.name }), ...path.split('/').flatMap((p, i, a) => [' / ', i === a.length - 1 ? el('b', { text: p }) : el('a', { href: `/${r.full_name}/tree/${ref}/${a.slice(0, i + 1).join('/')}`, text: p })])) : null;
  const last = el('div', { class: 'lastc' }, el('span', { class: 'muted', text: 'Loading the last commit…' }));
  api(`/repos/${r.full_name}/commits${q({ sha: ref, path, limit: 1, stat: false, verification: false, files: false })}`, { withTotal: true }).then(async ({ data, total }) => {
    const c = data[0];
    if (!c) return;
    const dot = el('span');
    api(`/repos/${r.full_name}/commits/${c.sha}/status`).then((s) => { if (s?.total_count) dot.replaceWith(statusDot(s.state)); }).catch(() => {});
    last.replaceChildren(avatar(c.author), el('b', { text: c.author?.login || c.commit.author.name }),
      el('a', { class: 'msg', href: `/${r.full_name}/commit/${c.sha}`, text: firstLine(c.commit.message) }), dot,
      el('a', { class: 'mono muted', href: `/${r.full_name}/commit/${c.sha}`, text: short(c.sha) }), el('span', { class: 'muted small' }, when(c.commit.author.date)),
      el('a', { class: 'small', href: `/${r.full_name}/commits/${ref}${path ? `/${path}` : ''}` }, ic('clock'), ` ${plural(total, 'commit')}`));
  }).catch(() => last.remove());

  const readme = listing.find((e) => e.type === 'file' && RENDERED.test(e.name));
  const readmeBox = readme ? el('div', { class: 'box', style: 'margin-top:16px' }, el('header', {}, ic('file'), el('b', { text: readme.name }))) : null;
  if (readme) {
    text(rawUrl(r, ref, readme.path)).then(async (src) => {
      const base = { tree: `/${r.full_name}/blob/${ref}${path ? `/${path}` : ''}`, raw: `/api/v1/repos/${r.full_name}/raw${path ? `/${enc(path)}` : ''}` };
      const md = /\.txt$/i.test(readme.name) ? el('pre', { class: 'md pad', text: src }) : await markdown(src, base);
      md.classList.add('pad');
      readmeBox.append(md);
    }).catch(() => {});
  }

  // the branch and tag counts come when they come; the files do not wait
  const nBranches = el('b', { text: '…' });
  const nTags = el('b', { text: '…' });
  const nBranchWord = el('span', { text: ' branches' });
  const nTagWord = el('span', { text: ' tags' });
  refs(r).then(({ branches, tags }) => {
    nBranches.textContent = String(branches.length);
    nBranchWord.textContent = ` branch${branches.length === 1 ? '' : 'es'}`;
    nTags.textContent = String(tags.length);
    nTagWord.textContent = ` tag${tags.length === 1 ? '' : 's'}`;
  }).catch(() => {});
  const side = el('aside', { class: 'side' },
    el('section', {}, el('h3', { text: 'About' }), el('p', { text: r.description || 'No description.' }), r.website ? el('p', {}, el('a', { href: r.website, text: r.website })) : null,
      el('p', { class: 'small muted' }, ic(r.private ? 'lock' : 'repo', 'i s'), r.private ? ' Only the people it is shared with can see it' : ' Anyone can see it')),
    cloneBox(r));
  const langs = el('section', {}, el('h3', { text: 'Languages' }));
  api(`/repos/${r.full_name}/languages`).then((l) => { if (Object.keys(l).length) langs.append(...languages(l)); else langs.remove(); }).catch(() => langs.remove());
  const rel = el('section', {}, el('h3', { text: 'Running now' }));
  fleet(r).then((boxes) => {
    if (!boxes.length) { rel.remove(); return; }
    rel.append(...boxes.map((b) => el('p', {}, el('span', { class: `dot ${b.up ? 'ok' : 'bad'}` }), ' ', el('b', { text: b.name }), ` release ${b.release ?? '?'}`, el('span', { class: 'small muted', text: b.result && b.result !== 'ok' ? ` · ${b.result}` : '' }))));
  }).catch(() => rel.remove());
  side.append(rel, langs);

  if (!current()) return;
  put(main(), el('div', { class: 'two' },
    el('div', {},
      el('div', { class: 'hrow gap' }, refPicker(r, ref, (n) => `/${r.full_name}/tree/${n}${path ? `/${path}` : ''}`),
        crumbs || el('span', { class: 'small' }, el('a', { href: `/${r.full_name}/branches` }, nBranches, nBranchWord), ' · ', el('a', { href: `/${r.full_name}/releases` }, nTags, nTagWord)),
        el('span', { class: 'spacer' }),
        r.permissions?.push ? el('a', { class: 'btn plain', href: `/${r.full_name}/compare` }, ic('pr'), 'Compare') : null),
      el('div', { class: 'box' }, last, el('div', { class: 'list files' }, ...(path ? [el('a', { href: path.includes('/') ? `/${r.full_name}/tree/${ref}/${path.split('/').slice(0, -1).join('/')}` : `/${r.full_name}/tree/${ref}` }, el('span', { class: 'name' }, ic('dir', 'i dir'), '..'))] : []), ...rows)),
      readmeBox),
    side));
});

function emptyRepo(r) {
  return el('div', { class: 'box md pad', style: 'max-width:760px' },
    el('h2', { text: 'Nothing here yet' }),
    el('p', { text: 'Push an existing repository from your device:' }),
    el('pre', { text: `git remote add origin ${r.clone_url}\ngit push -u origin main` }));
}

// ---- a file, with the tree beside it

async function tree(r, ref, path) {
  const box = el('aside', { class: 'box tree', 'aria-label': 'Files' });
  const find = el('input', { placeholder: 'Go to file', 'aria-label': 'Go to file' });
  const nodes = el('div');
  box.append(find, nodes);
  const open = new Set(path.split('/').slice(0, -1).map((_, i, a) => a.slice(0, i + 1).join('/')));
  open.add('');
  const dirs = new Map();
  const load = (dir) => {
    if (!dirs.has(dir)) dirs.set(dir, api(`/repos/${r.full_name}/contents${dir ? `/${enc(dir)}` : ''}${q({ ref })}`).then((l) => l.sort((a, b) => (b.type === 'dir') - (a.type === 'dir') || a.name.localeCompare(b.name))));
    return dirs.get(dir);
  };
  const draw = async (dir, depth, into) => {
    for (const e of await load(dir)) {
      const pad = `padding-left:${6 + depth * 14}px`;
      if (e.type === 'dir') {
        const shut = !open.has(e.path);
        const a = el('a', { href: `/${r.full_name}/tree/${ref}/${e.path}`, style: pad, class: shut ? 'shut' : '' }, ic('down', 'i tw'), ic('dir', 'i dir'), e.name);
        const kids = el('div');
        a.onclick = async (ev) => {
          ev.preventDefault();
          if (open.has(e.path)) { open.delete(e.path); kids.replaceChildren(); a.classList.add('shut'); }
          else { open.add(e.path); a.classList.remove('shut'); await draw(e.path, depth + 1, kids); }
        };
        into.append(a, kids);
        if (!shut) await draw(e.path, depth + 1, kids);
      } else {
        into.append(el('a', { href: `/${r.full_name}/blob/${ref}/${e.path}`, style: `${pad};padding-left:${26 + depth * 14}px`, 'aria-current': e.path === path ? 'page' : null }, ic('file'), e.name));
      }
    }
  };
  await draw('', 0, nodes);
  // "Go to file": every path, asked for once
  let all = null;
  find.oninput = async () => {
    const s = find.value.trim().toLowerCase();
    if (!s) { nodes.hidden = false; box.querySelector('.hits')?.remove(); return; }
    all ??= api(`/repos/${r.full_name}/git/trees/${encodeURIComponent(ref)}${q({ recursive: true, per_page: 10000 })}`).then((t) => t.tree.filter((x) => x.type === 'blob').map((x) => x.path));
    const hits = (await all).filter((p) => p.toLowerCase().includes(s)).slice(0, 40);
    nodes.hidden = true;
    box.querySelector('.hits')?.remove();
    box.append(el('div', { class: 'hits' }, ...hits.map((p) => el('a', { href: `/${r.full_name}/blob/${ref}/${p}`, title: p }, ic('file'), p.split('/').pop(), el('span', { class: 'small muted', text: ` ${p.split('/').slice(0, -1).join('/')}` })))));
  };
  return box;
}

const IMAGE = /\.(png|jpe?g|gif|webp|svg|avif|bmp|ico)$/i;
const MARKDOWN = /\.(md|markdown)$/i;

route(/^\/([^/]+)\/([^/]+)\/blob\/(.+)$/, async ({ m, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'code');
  const { ref, path } = await split(r, m[3]);
  setTitle(path.split('/').pop(), r.full_name);
  const meta = await api(`/repos/${r.full_name}/contents/${enc(path)}${q({ ref })}`);
  if (!current()) return;
  if (Array.isArray(meta)) { go(`/${r.full_name}/tree/${ref}/${path}`, true); return; }
  const raw = rawUrl(r, ref, path);
  const view = el('div');
  let asText = !IMAGE.test(path) && meta.size < 1024 * 1024;
  let body = null;
  if (IMAGE.test(path)) body = el('div', { style: 'padding:16px;text-align:center' }, el('img', { src: raw, alt: path, style: 'max-width:100%' }));
  else if (!asText) body = el('div', { class: 'binary' }, `${bytes(meta.size)}: too big to show here. `, el('a', { href: raw, text: 'Download it' }));
  let src = '';
  if (asText) {
    src = await text(raw);
    if (/\u0000/.test(src.slice(0, 8000))) { asText = false; body = el('div', { class: 'binary' }, 'A binary file. ', el('a', { href: raw, text: 'Download it' })); }
  }
  const lines = asText ? src.replace(/\n$/, '').split('\n') : [];
  const codeView = () => el('div', { class: 'code' }, el('table', {}, el('tbody', {}, ...lines.map((l, i) => el('tr', { id: `L${i + 1}` }, el('td', { class: 'ln' }, el('a', { href: `#L${i + 1}`, text: String(i + 1) })), el('td', { text: l }))))));
  let showing = MARKDOWN.test(path) ? 'preview' : 'code';
  const draw = async () => view.replaceChildren(!asText ? body : showing === 'preview' ? Object.assign(await markdown(src, { tree: `/${r.full_name}/blob/${ref}/${path.split('/').slice(0, -1).join('/')}`, raw: `/api/v1/repos/${r.full_name}/raw/${enc(path.split('/').slice(0, -1).join('/'))}` }), { className: 'md pad' }) : codeView());
  const seg = MARKDOWN.test(path) ? el('span', { class: 'gseg' }, ...['preview', 'code'].map((k) => el('button', { class: 'btn', 'aria-pressed': String(showing === k), text: k === 'preview' ? 'Preview' : 'Code', onclick: (e) => { showing = k; seg.querySelectorAll('button').forEach((b) => b.setAttribute('aria-pressed', String(b === e.target))); draw(); } }))) : null;
  await draw();

  const last = el('div', { class: 'lastc' });
  api(`/repos/${r.full_name}/commits${q({ sha: ref, path, limit: 1, stat: false, verification: false, files: false })}`).then(([c]) => {
    if (!c) return;
    last.replaceChildren(avatar(c.author), el('b', { text: c.author?.login || c.commit.author.name }), el('a', { class: 'msg', href: `/${r.full_name}/commit/${c.sha}`, text: firstLine(c.commit.message) }), el('a', { class: 'mono muted', href: `/${r.full_name}/commit/${c.sha}`, text: short(c.sha) }), el('span', { class: 'muted small' }, when(c.commit.author.date)), el('a', { class: 'small', href: `/${r.full_name}/commits/${ref}/${path}` }, ic('clock'), ' History'));
  }).catch(() => {});
  const cp = el('button', { class: 'btn plain', 'aria-label': 'Copy the file' }, ic('copy'));
  cp.onclick = () => copy(src, cp);
  const crumbs = el('span', {}, el('a', { href: `/${r.full_name}/tree/${ref}`, text: r.name }), ...path.split('/').flatMap((p, i, a) => [' / ', i === a.length - 1 ? el('b', { text: p }) : el('a', { href: `/${r.full_name}/tree/${ref}/${a.slice(0, i + 1).join('/')}`, text: p })]));
  if (!current()) return;
  put(main(), el('div', { class: 'withtree' },
    await tree(r, ref, path),
    el('div', { style: 'min-width:0' },
      el('div', { class: 'hrow gap' }, refPicker(r, ref, (n) => `/${r.full_name}/blob/${n}/${path}`), crumbs),
      el('div', { class: 'box' }, last,
        el('header', { style: 'border-radius:0' }, el('span', { class: 'small muted', text: asText ? `${lines.length.toLocaleString()} lines · ${bytes(meta.size)}` : bytes(meta.size) }), el('span', { class: 'spacer' }), seg, el('a', { class: 'btn plain', href: raw, text: 'Raw' }), asText ? cp : null),
        view))));
});

// ---- commits

route(/^\/([^/]+)\/([^/]+)\/commits(?:\/(.+))?$/, async ({ m, params, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'code');
  const { ref, path } = await split(r, m[3]);
  const page = Number(params.get('page') || 1);
  setTitle('Commits', r.full_name);
  const { data, total } = await api(`/repos/${r.full_name}/commits${q({ sha: ref, path, page, limit: 30, stat: false, verification: false, files: false })}`, { withTotal: true });
  if (!current()) return;
  const days = new Map();
  for (const c of data) {
    const d = new Date(c.commit.committer.date).toLocaleDateString(undefined, { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' });
    if (!days.has(d)) days.set(d, []);
    days.get(d).push(c);
  }
  const row = (c) => {
    const dot = el('span');
    api(`/repos/${r.full_name}/commits/${c.sha}/status`).then((s) => { if (s?.total_count) dot.replaceWith(statusDot(s.state)); }).catch(() => {});
    const cp = el('button', { class: 'btn plain', style: 'padding:4px 8px', 'aria-label': 'Copy the full hash' }, ic('copy', 'i s'));
    cp.onclick = () => copy(c.sha, cp);
    return el('div', { class: 'item' },
      el('div', { style: 'flex:1;min-width:0' }, el('a', { class: 't', href: `/${r.full_name}/commit/${c.sha}`, text: firstLine(c.commit.message) }),
        el('div', { class: 'sub hrow', style: 'gap:6px' }, avatar(c.author), el('b', { text: c.author?.login || c.commit.author.name }), 'committed ', when(c.commit.committer.date))),
      dot, el('a', { class: 'btn mono plain', href: `/${r.full_name}/commit/${c.sha}`, text: short(c.sha) }), cp, el('a', { class: 'btn plain', style: 'padding:4px 8px', href: `/${r.full_name}/tree/${c.sha}`, title: 'Browse the files at this commit' }, ic('code', 'i s')));
  };
  if (!current()) return;
  put(main(), 
    el('div', { class: 'hrow gap' }, refPicker(r, ref, (n) => `/${r.full_name}/commits/${n}${path ? `/${path}` : ''}`), el('h1', { class: 'h1', style: 'font-size:18px', text: path ? `History of ${path}` : 'Commits' }), el('span', { class: 'small muted', text: plural(total, 'commit') })),
    ...[...days].flatMap(([d, cs]) => [el('h3', { class: 'small muted', style: 'margin:0 0 8px' }, ic('commit'), ` ${d}`), el('div', { class: 'box list gap' }, ...cs.map(row))]),
    pager(page, total, 30, (p) => go(`${location.pathname}${q({ page: p })}`)));
});

// ---- a commit

route(/^\/([^/]+)\/([^/]+)\/commit\/([0-9a-f]{4,64})$/, async ({ m, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'code');
  const sha = m[3];
  const [c, patch] = await Promise.all([
    api(`/repos/${r.full_name}/git/commits/${sha}${q({ stat: false, files: false })}`),
    text(`/api/v1/repos/${r.full_name}/git/commits/${sha}.diff`),
  ]);
  if (!current()) return;
  setTitle(firstLine(c.commit.message), r.full_name);
  const files = diff.parse(patch);
  const [title, ...rest] = c.commit.message.split('\n');
  const holder = el('div');
  const { seg, mode } = diff.modeSwitch((md) => holder.replaceChildren(...diff.render(files, { mode: md })));
  holder.replaceChildren(...diff.render(files, { mode: mode() }));
  const status = el('span');
  api(`/repos/${r.full_name}/commits/${sha}/status`).then((s) => { if (s?.total_count) status.replaceWith(el('span', { class: 'hrow small', style: 'gap:6px' }, statusDot(s.state), `checks ${s.state}`)); }).catch(() => {});
  if (!current()) return;
  put(main(), 
    el('div', { class: 'box gap' },
      el('div', { style: 'padding:14px 16px' }, el('h1', { class: 'h1', style: 'font-size:19px', text: title }), rest.join('\n').trim() ? el('pre', { class: 'muted', style: 'white-space:pre-wrap;font:inherit;margin:8px 0 0', text: rest.join('\n').trim() }) : null),
      el('header', { style: 'border-radius:0 0 4px 4px;border-bottom:0;border-top:1px solid var(--line)' },
        avatar(c.author), el('b', { text: c.author?.login || c.commit.author.name }), el('span', { class: 'muted' }, 'committed ', when(c.commit.committer.date)), status, el('span', { class: 'spacer' }),
        el('span', { class: 'small muted' }, `${plural(c.parents.length, 'parent')} `, ...c.parents.map((p) => el('a', { class: 'mono', href: `/${r.full_name}/commit/${p.sha}`, text: `${short(p.sha)} ` })), ' · commit ', el('span', { class: 'mono', text: short(sha) })),
        el('a', { class: 'btn plain', href: `/${r.full_name}/tree/${sha}`, text: 'Browse files' }))),
    el('div', { class: 'hrow gap' }, diff.summary(files), el('span', { class: 'spacer' }), seg),
    holder);
});

// ---- branches

route(/^\/([^/]+)\/([^/]+)\/branches$/, async ({ m, current }) => {
  const r = await repo(m[1], m[2], true);
  if (!current()) return;
  header(r, 'code');
  setTitle('Branches', r.full_name);
  refsOf.delete(r.full_name);
  const [{ branches }, pulls, rules] = await Promise.all([refs(r), api(`/repos/${r.full_name}/pulls${q({ state: 'open', limit: 50 })}`).catch(() => []), api(`/repos/${r.full_name}/branch_protections`).catch(() => [])]);
  if (!current()) return;
  branches.sort((a, b) => (b.name === r.default_branch) - (a.name === r.default_branch) || new Date(b.commit.timestamp) - new Date(a.commit.timestamp));
  const rows = branches.map((b) => {
    const pr = pulls.find((p) => p.head.ref === b.name && p.head.repo?.full_name === r.full_name);
    const counts = el('span', { class: 'small muted' });
    if (b.name !== r.default_branch) {
      Promise.all([
        api(`/repos/${r.full_name}/compare/${encodeURIComponent(r.default_branch)}...${encodeURIComponent(b.name)}`),
        api(`/repos/${r.full_name}/compare/${encodeURIComponent(b.name)}...${encodeURIComponent(r.default_branch)}`),
      ]).then(([ahead, behind]) => { counts.textContent = `${ahead.total_commits} ahead · ${behind.total_commits} behind`; }).catch(() => {});
    }
    const del = r.permissions?.push && b.name !== r.default_branch && !b.protected ? el('button', { class: 'btn plain', 'aria-label': `Delete ${b.name}` }, ic('x')) : null;
    const row = el('div', {}, ic('branch'), el('a', { href: `/${r.full_name}/tree/${b.name}`, text: b.name }),
      b.name === r.default_branch ? el('span', { class: 'chip', text: 'default' }) : null,
      b.protected || rules.some((x) => x.rule_name === b.name) ? el('span', { class: 'chip ok' }, ic('lock', 'i s'), ' protected') : null,
      counts, el('span', { class: 'spacer' }), el('span', { class: 'small muted' }, 'updated ', when(b.commit.timestamp)),
      pr ? el('a', { class: 'chip', href: `/${r.full_name}/pull/${pr.number}`, text: `#${pr.number}` }) : b.name !== r.default_branch && r.permissions?.push ? el('a', { class: 'btn plain', href: `/${r.full_name}/compare/${r.default_branch}...${b.name}`, text: 'New pull request' }) : null,
      del);
    if (del) del.onclick = async () => {
      if (!confirm(`Delete the branch ${b.name}? Its commits stay only if another branch or a pull request has them.`)) return;
      await api(`/repos/${r.full_name}/branches/${encodeURIComponent(b.name)}`, { method: 'DELETE' });
      row.remove();
      refsOf.delete(r.full_name);
    };
    return row;
  });
  if (!current()) return;
  put(main(), el('div', { class: 'hrow gap' }, el('h1', { class: 'h1', text: 'Branches' }), el('span', { class: 'small muted', text: plural(branches.length, 'branch', 'branches') })), el('div', { class: 'box list' }, ...rows));
});

// ---- releases: the forge's, its tags, and what the boxes run

route(/^\/([^/]+)\/([^/]+)\/releases(?:\/tag\/(.+))?$/, async ({ m, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'releases');
  setTitle('Releases', r.full_name);
  const [rels, tags] = await releaseData(r);
  if (!current()) return;
  // what the boxes run is asked of every box: it comes when it comes
  const running = el('div');
  fleet(r).then((boxes) => {
    if (!boxes.length || !running.isConnected) return;
    running.replaceWith(el('div', { class: 'box gap' }, el('header', {}, el('b', { text: 'Running now' }), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: 'a release is signed on your laptop and pulled by each box' })),
      el('div', { class: 'hrow', style: 'padding:12px 14px' }, ...boxes.map((b) => el('span', { class: 'pill', style: 'font-size:13px;padding:4px 12px' }, el('span', { class: `dot ${b.up ? 'ok' : 'bad'}` }), ` ${b.name}: release ${b.release ?? '?'}${b.result && b.result !== 'ok' ? ` (${b.result})` : ''}`)))));
  }).catch(() => {});
  if (m[3]) {
    const one = rels.find((x) => x.tag_name === m[3]) || await api(`/repos/${r.full_name}/releases/tags/${encodeURIComponent(m[3])}`);
    if (!current()) return;
    put(main(), el('p', {}, el('a', { href: `/${r.full_name}/releases`, text: '← Releases' })),
      el('div', { class: 'two' },
        el('div', { class: 'box' }, el('div', { style: 'padding:18px 22px 6px' }, el('h1', { class: 'h1', style: 'font-size:24px', text: one.name || one.tag_name }), el('p', { class: 'muted', style: 'margin:6px 0 0' }, ic('tag'), ` ${one.tag_name} · `, when(one.published_at || one.created_at))), el('div', { class: 'md pad' }, await markdown(one.body || '_No notes._'))),
        el('aside', { class: 'side' },
          el('section', {}, el('h3', { text: 'Commit' }), el('p', {}, el('a', { class: 'mono', href: `/${r.full_name}/tree/${one.tag_name}`, text: one.target_commitish }))),
          one.assets?.length ? el('section', {}, el('h3', { text: 'Files' }), ...one.assets.map((a) => el('p', {}, el('a', { href: a.browser_download_url, text: a.name }), el('span', { class: 'small muted', text: ` ${bytes(a.size)}` })))) : null,
          el('section', {}, el('h3', { text: 'Download the code' }), el('p', {}, el('a', { href: one.zipball_url, text: 'zip' }), ' · ', el('a', { href: one.tarball_url, text: 'tar.gz' }))))));
    return;
  }
  const relRows = rels.map((x, i) => el('a', { class: 'item', href: `/${r.full_name}/releases/tag/${x.tag_name}` }, ic('tag'), el('div', { style: 'flex:1;min-width:0' }, el('div', { class: 't' }, x.name || x.tag_name, i === 0 ? el('span', { class: 'chip ok', style: 'margin-left:8px', text: 'Latest' }) : null, x.prerelease ? el('span', { class: 'chip busy', style: 'margin-left:8px', text: 'pre-release' }) : null), el('div', { class: 'sub', text: firstLine(x.body) })), el('span', { class: 'small muted' }, when(x.published_at || x.created_at))));
  const tagRows = tags.map((t) => el('div', {}, ic('tag'), el('a', { href: `/${r.full_name}/tree/${t.name}`, class: 'mono', text: t.name }), el('span', { class: 'spacer' }), el('a', { class: 'mono small muted', href: `/${r.full_name}/commit/${t.commit.sha}`, text: short(t.commit.sha) }), el('a', { class: 'small', href: t.zipball_url, text: 'zip' })));
  if (!current()) return;
  put(main(), el('div', { class: 'hrow gap' }, el('h1', { class: 'h1', text: 'Releases' })), running,
    rels.length ? el('div', { class: 'box list gap' }, ...relRows) : el('p', { class: 'muted', text: 'No releases published on the forge yet.' }),
    tags.length ? [el('h2', { class: 'ph', style: 'font-size:16px', text: 'Tags' }), el('div', { class: 'box list' }, ...tagRows)] : null);
});

// ---- activity: the week's lines, the year's squares, what happened

warmers.releases = (r) => releaseData(r);
warmers.activity = (r) => yearOf(r);
const releaseData = (r) => Promise.all([
  api(`/repos/${r.full_name}/releases${q({ limit: 50 })}`).catch(() => []),
  api(`/repos/${r.full_name}/tags${q({ limit: 50 })}`).catch(() => []),
]);

// A year of commits on the default branch, by day: fetched eight pages at
// a time, and kept in this browser until the branch moves.
const years = new Map();
function yearOf(r) {
  if (years.has(r.full_name)) return years.get(r.full_name);
  const p = (async () => {
    const since = Date.now() - 365 * 86400e3;
    const url = (page) => `/repos/${r.full_name}/commits${q({ sha: r.default_branch, page, limit: 50, stat: false, verification: false, files: false })}`;
    const first = await api(url(1), { withTotal: true });
    const key = `git-year:${r.full_name}`;
    const head = first.data[0]?.sha;
    try {
      const c = JSON.parse(localStorage.getItem(key) || 'null');
      if (c && c.head === head) return c.byDay;
    } catch { /* counted again */ }
    const byDay = {};
    const add = (cs) => {
      let older = false;
      for (const c of cs) {
        const t = new Date(c.commit.committer.date).getTime();
        if (t >= since) byDay[dayKey(t)] = (byDay[dayKey(t)] || 0) + 1;
        else older = true;
      }
      return older;
    };
    // the forge sends no total for commits: pages until one reaches back a year
    let done = add(first.data) || first.data.length < 50;
    for (let n = 2; n <= 40 && !done; n += 8) {
      const batch = await Promise.all([...Array(Math.min(8, 41 - n))].map((_, i) => api(url(n + i)).catch(() => [])));
      for (const cs of batch) if (add(cs) || cs.length < 50) done = true;
    }
    try { localStorage.setItem(key, JSON.stringify({ head, byDay })); } catch { /* kept for this visit only */ }
    return byDay;
  })();
  years.set(r.full_name, p);
  p.catch(() => years.delete(r.full_name));
  return p;
}

route(/^\/([^/]+)\/([^/]+)\/activity$/, async ({ m, params, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'activity');
  setTitle('Activity', r.full_name);
  const span = params.get('span') === 'month' ? 30 : 7;
  const start = new Date();
  start.setHours(0, 0, 0, 0);
  start.setDate(start.getDate() - (span - 1));
  const days = [...Array(span)].map((_, i) => { const d = new Date(start); d.setDate(d.getDate() + i); return d; });
  const inSpan = (t) => new Date(t) >= start;
  const label = (d) => (span === 7 ? d.toLocaleDateString(undefined, { weekday: 'short', day: 'numeric' }) : d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' }));
  const waiting = (t) => el('div', { class: 'empty muted', text: t });

  // the page at once; each part fills in as its answer comes
  const chart = el('div', {}, waiting('Counting commits…'));
  const stat = (t) => { const b = el('b', { text: '…' }); return [b, el('div', {}, b, el('span', { class: 'small muted', text: t }))]; };
  const [sCommits, sCommitsBox] = stat(`commits on ${r.default_branch}`);
  const [sMerged, sMergedBox] = stat('pull requests merged');
  const [sTags, sTagsBox] = stat('tags');
  const [sRuns, sRunsBox] = stat('runs passed');
  const yearHead = el('b', { text: 'The last year' });
  const heat = el('div', {}, waiting('Counting commits…'));
  const feedBox = el('div', { class: 'box list' }, waiting('Loading…'));
  put(main(),
    el('div', { class: 'hrow gap' }, el('h1', { class: 'h1', text: 'Activity' }), el('span', { class: 'spacer' }),
      el('span', { class: 'gseg' }, el('a', { class: 'btn', href: `/${r.full_name}/activity`, 'aria-pressed': String(span === 7), text: 'This week' }), el('a', { class: 'btn', href: `/${r.full_name}/activity?span=month`, 'aria-pressed': String(span === 30), text: 'This month' }))),
    el('div', { class: 'box gap' }, chart, el('div', { class: 'stats' }, sCommitsBox, sMergedBox, sTagsBox, sRunsBox)),
    el('div', { class: 'box gap' }, el('header', {}, yearHead, el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: `every day with a commit on ${r.default_branch}` })), heat),
    el('div', { class: 'hrow', style: 'margin-bottom:10px' }, el('b', { text: 'Everything that happened' })),
    feedBox);
  const live = () => current() && chart.isConnected;

  // the issues list says when each pull request merged, and answers in a
  // blink where the pull request list takes seconds
  const pullsP = Promise.all([1, 2].map((page) => api(`/repos/${r.full_name}/issues${q({ type: 'pulls', state: 'closed', since: start.toISOString(), page, limit: 50 })}`).catch(() => []))).then((x) => x.flat());
  const mergedByP = pullsP.then((pulls) => {
    const merged = pulls.filter((p) => p.pull_request?.merged && inSpan(p.pull_request.merged_at)).map((p) => ({ merged_at: p.pull_request.merged_at }));
    const by = Object.fromEntries(days.map((d) => [dayKey(d), 0]));
    for (const p of merged) by[dayKey(p.merged_at)] = (by[dayKey(p.merged_at)] || 0) + 1;
    if (live()) sMerged.textContent = String(merged.length);
    return by;
  });
  let byDayNow = null;
  let mergedNow = null;
  const drawChart = () => {
    if (!live() || !byDayNow) return;
    chart.replaceChildren(lineChart(days.map(label), [
      { name: 'commits', color: 'var(--accent)', values: days.map((d) => byDayNow[dayKey(d)] || 0) },
      ...(mergedNow ? [{ name: 'pull requests merged', color: 'var(--merged)', dashed: true, values: days.map((d) => mergedNow[dayKey(d)] || 0) }] : []),
    ]));
  };
  yearOf(r).then((byDay) => {
    byDayNow = byDay;
    if (!live()) return;
    drawChart();
    sCommits.textContent = days.reduce((t, d) => t + (byDay[dayKey(d)] || 0), 0).toLocaleString();
    yearHead.textContent = `${plural(Object.values(byDay).reduce((a, b) => a + b, 0), 'commit')} in the last year`;
    heat.replaceChildren(heatmap(byDay));
  }).catch(() => { if (live()) { chart.replaceChildren(waiting('The commits could not be counted.')); heat.replaceChildren(); } });
  mergedByP.then((by) => { mergedNow = by; drawChart(); });
  api(`/repos/${r.full_name}/tags${q({ limit: 50 })}`).then((tags) => {
    if (live()) sTags.textContent = String(tags.filter((t) => t.commit?.created && inSpan(t.commit.created)).length || '—');
  }).catch(() => { sTags.textContent = '—'; });
  (r.has_actions ? api(`/repos/${r.full_name}/actions/runs${q({ limit: 50 })}`).then((x) => x.workflow_runs || []) : Promise.resolve([])).then((runs) => {
    if (!live()) return;
    const done = runs.filter((x) => inSpan(x.created) && ['success', 'failure', 'cancelled'].includes(x.status));
    sRuns.textContent = done.length ? `${Math.round((done.filter((x) => x.status === 'success').length / done.length) * 100)}%` : '—';
    sRuns.nextSibling.textContent = `of ${done.length} runs passed`;
  }).catch(() => { sRuns.textContent = '—'; });
  api(`/repos/${r.full_name}/activities/feeds${q({ limit: 40 })}`).then((feed) => {
    if (live()) feedBox.replaceChildren(...(feed.length ? feed.map(feedLine) : [waiting('Nothing yet.')]));
  }).catch(() => feedBox.replaceChildren(waiting('Could not be loaded.')));
});

// the forge's own addresses, from before these pages, lead to the same place
route(/^\/([^/]+)\/([^/]+)\/src\/(?:branch|tag|commit)\/(.+)$/, ({ m }) => go(`/${m[1]}/${m[2]}/tree/${m[3]}`, true));
route(/^\/([^/]+)\/([^/]+)\/(?:commits\/branch)\/(.+)$/, ({ m }) => go(`/${m[1]}/${m[2]}/commits/${m[3]}`, true));

export { refPicker, split, refs };
