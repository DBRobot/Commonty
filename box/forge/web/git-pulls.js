// Pull requests and issues: the lists, a conversation, a pull request's
// commits, checks and files, the merge box, and opening new ones.

import {
  put, app, el, ic, api, text, q, when, plural, avatar, route, go, statusDot, markdown, toast,
  setTitle, firstLine, short, pop, pager, whoami, $,
} from './git-core.js';
import { repo, header, refs } from './git-repo.js';
import * as diff from './git-diff.js';

const main = () => app();

// ---- lists

function stateIcon(i) {
  if (i.pull_request || i.merged !== undefined) {
    const merged = i.merged || i.pull_request?.merged;
    return ic(merged ? 'merged' : 'pr', 'i');
  }
  return ic('issue');
}
const stateColor = (i) => ((i.merged || i.pull_request?.merged) ? 'var(--merged)' : i.state === 'open' ? 'var(--ok)' : 'var(--bad)');

function labelChip(l) {
  const c = `#${l.color.replace('#', '')}`;
  return el('span', { class: 'chip', style: `background:${c}22;color:var(--ink);box-shadow:inset 0 0 0 1px ${c}88`, text: l.name, title: l.description || '' });
}

async function list({ m, params, current }, kind) {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, kind);
  const state = params.get('state') || 'open';
  const page = Number(params.get('page') || 1);
  const search = params.get('q') || '';
  const pulls = kind === 'pulls';
  setTitle(pulls ? 'Pull requests' : 'Issues', r.full_name);
  const [{ data, total }, open, closed] = await Promise.all([
    api(`/repos/${r.full_name}/issues${q({ type: pulls ? 'pulls' : 'issues', state, page, limit: 25, q: search })}`, { withTotal: true }),
    api(`/repos/${r.full_name}/issues${q({ type: pulls ? 'pulls' : 'issues', state: 'open', limit: 1, q: search })}`, { withTotal: true }),
    api(`/repos/${r.full_name}/issues${q({ type: pulls ? 'pulls' : 'issues', state: 'closed', limit: 1, q: search })}`, { withTotal: true }),
  ]);
  if (!current()) return;
  const base = `/${r.full_name}/${pulls ? 'pulls' : 'issues'}`;
  const find = el('input', { class: 'btn plain', style: 'flex:1;min-width:200px', value: search, placeholder: `Find ${pulls ? 'a pull request' : 'an issue'}`, 'aria-label': 'Find' });
  find.onkeydown = (e) => { if (e.key === 'Enter') go(`${base}${q({ state, q: find.value })}`); };
  const rows = data.map((i) => {
    const dot = el('span');
    if (pulls && i.state === 'open') {
      api(`/repos/${r.full_name}/pulls/${i.number}`).then((p) => api(`/repos/${r.full_name}/commits/${p.head.sha}/status`)).then((s) => { if (s?.total_count) dot.replaceWith(statusDot(s.state)); }).catch(() => {});
    }
    return el('div', { class: 'item' },
      el('span', { style: `color:${stateColor(i)};margin-top:3px` }, stateIcon(i)),
      el('div', { style: 'flex:1;min-width:0' },
        el('div', {}, el('a', { class: 't', href: `/${r.full_name}/${pulls ? 'pull' : 'issues'}/${i.number}`, text: i.title }), ' ', ...(i.labels || []).map(labelChip), i.pull_request?.draft ? el('span', { class: 'chip', text: 'draft' }) : null),
        el('div', { class: 'sub' }, `#${i.number} `, i.state === 'open' ? ['opened ', when(i.created_at)] : ['closed ', when(i.closed_at)], ` by ${i.user.login}`, i.milestone ? ` · ${i.milestone.title}` : '')),
      dot,
      i.comments ? el('span', { class: 'small muted', title: 'comments' }, `💬 ${i.comments}`) : null,
      ...(i.assignees || []).slice(0, 3).map((u) => avatar(u)));
  });
  if (!current()) return;
  put(main(), 
    el('div', { class: 'hrow gap' }, find,
      r.permissions?.pull ? el('a', { class: 'btn go', href: pulls ? `/${r.full_name}/compare` : `/${r.full_name}/issues/new` }, ic('plus'), pulls ? 'New pull request' : 'New issue') : null),
    el('div', { class: 'box' },
      el('header', {},
        el('a', { href: `${base}${q({ q: search })}`, style: state === 'open' ? 'font-weight:700;color:var(--ink)' : 'color:var(--ink-2)' }, stateIcon({ state: 'open', pull_request: pulls ? {} : undefined }), ` ${open.total} open`),
        el('a', { href: `${base}${q({ state: 'closed', q: search })}`, style: state === 'closed' ? 'font-weight:700;color:var(--ink)' : 'color:var(--ink-2)' }, ic('check'), ` ${closed.total} closed`)),
      el('div', { class: 'list' }, ...(rows.length ? rows : [el('div', { class: 'empty', text: state === 'open' ? `No open ${pulls ? 'pull requests' : 'issues'}.` : 'Nothing closed yet.' })]))),
    pager(page, total, 25, (p) => go(`${base}${q({ state, q: search, page: p })}`)));
}
route(/^\/([^/]+)\/([^/]+)\/pulls$/, (c) => list(c, 'pulls'));
route(/^\/([^/]+)\/([^/]+)\/issues$/, (c) => list(c, 'issues'));

// ---- the conversation

async function commentBox(r, n, after) {
  const ta = el('textarea', { class: 'write', placeholder: 'Leave a comment. Markdown works.', 'aria-label': 'Comment' });
  const send = el('button', { class: 'btn go', type: 'submit', text: 'Comment' });
  const form = el('form', {}, ta, el('div', { class: 'hrow', style: 'justify-content:flex-end;margin-top:8px' }, ...(after || []), send));
  form.onsubmit = async (e) => {
    e.preventDefault();
    if (!ta.value.trim()) return;
    send.disabled = true;
    try { await api(`/repos/${r.full_name}/issues/${n}/comments`, { method: 'POST', body: { body: ta.value } }); go(location.pathname, true); }
    catch (err) { toast(err.message); send.disabled = false; }
  };
  const w = await whoami();
  return el('div', { class: 'comment' }, avatar(w.forge, 'avatar l'), el('div', { class: 'box' }, el('header', {}, el('b', { text: 'Add a comment' })), el('div', { class: 'body' }, form)));
}

async function commentView(r, c, first) {
  return el('div', { class: 'comment', id: c.id ? `c${c.id}` : null },
    avatar(c.user, 'avatar l'),
    el('div', { class: 'box' },
      el('header', {}, el('b', { text: c.user.login }), el('span', { class: 'muted' }, first ? 'opened this ' : 'commented ', when(c.created_at)), c.updated_at && c.updated_at !== c.created_at && !first ? el('span', { class: 'small muted', text: '· edited' }) : null),
      el('div', { class: 'body' }, c.body ? await markdown(c.body) : el('p', { class: 'muted', style: 'margin:0', text: 'No description.' }))));
}

function eventLine(r, e) {
  const u = e.user?.login || 'someone';
  const says = {
    close: ['x', `${u} closed this`],
    reopen: ['issue', `${u} reopened this`],
    merge_pull: ['merged', `${u} merged this`],
    pull_push: ['commit', `${u} pushed ${(() => { try { return plural(JSON.parse(e.body).commit_ids.length, 'commit'); } catch { return 'commits'; } })()}`],
    label: ['tag', `${u} ${e.body === '1' ? 'added' : 'removed'} the label ${e.label?.name || ''}`],
    assignees: ['issue', `${u} ${e.removed_assignee ? 'unassigned' : 'assigned'} ${e.assignee?.login || ''}`],
    change_title: ['pulse', `${u} renamed this from “${e.old_title}” to “${e.new_title}”`],
    review_request: ['check', `${u} asked ${e.assignee?.login || 'someone'} to review`],
    commit_ref: ['commit', `${u} mentioned this in a commit`],
    delete_branch: ['branch', `${u} deleted the branch ${e.old_ref || ''}`],
    milestone: ['issue', `${u} set the milestone ${e.milestone?.title || ''}`],
    pull_scheduled_merge: ['clock', `${u} set this to merge when its checks pass`],
    pull_cancel_scheduled_merge: ['clock', `${u} stopped the merge when checks pass`],
  }[e.type];
  if (!says) return null;
  return el('div', { class: 'event' }, ic(says[0]), el('span', { text: says[1] }), when(e.created_at));
}

async function reviewView(r, e) {
  const verdict = { APPROVED: ['check', 'approved these changes', 'ok'], REQUEST_CHANGES: ['x', 'asked for changes', 'bad'], COMMENT: ['pr', 'reviewed', ''] }[e.review?.state || 'COMMENT'] || ['pr', 'reviewed', ''];
  return el('div', {},
    el('div', { class: 'event' }, el('span', { class: `chip ${verdict[2]}` }, ic(verdict[0], 'i s')), el('b', { text: e.user.login }), verdict[1], when(e.created_at)),
    e.body ? el('div', { style: 'margin-left:52px;margin-bottom:12px' }, el('div', { class: 'box' }, el('div', { class: 'body', style: 'padding:10px 14px' }, await markdown(e.body)))) : null);
}

async function timeline(r, n, issue) {
  const events = await api(`/repos/${r.full_name}/issues/${n}/timeline${q({ limit: 200 })}`).catch(() => []);
  const out = [await commentView(r, { ...issue, body: issue.body }, true)];
  for (const e of events) {
    if (e.type === 'comment') out.push(await commentView(r, e));
    else if (e.type === 'review') out.push(await reviewView(r, e));
    else { const l = eventLine(r, e); if (l) out.push(l); }
  }
  return out;
}

// the right-hand column: labels, people, milestone
async function sidebar(r, n, issue, isPull) {
  const can = r.permissions?.push;
  const section = (title, body, edit) => el('section', {}, el('div', { class: 'hrow', style: 'margin-bottom:8px' }, el('h3', { style: 'margin:0', text: title }), el('span', { class: 'spacer' }), edit), body);
  const labels = el('p', {}, ...(issue.labels?.length ? issue.labels.map(labelChip) : [el('span', { class: 'small muted', text: 'None yet' })]));
  const labelEdit = can ? pop(el('button', { class: 'btn plain', style: 'padding:2px 6px', 'aria-label': 'Change labels' }, ic('gear', 'i s')), async (close) => {
    const all = await api(`/repos/${r.full_name}/labels${q({ limit: 100 })}`);
    const on = new Set((issue.labels || []).map((l) => l.id));
    return all.map((l) => el('a', { href: '#', 'aria-current': on.has(l.id) ? 'true' : null, onclick: async (e) => {
      e.preventDefault();
      on.has(l.id) ? on.delete(l.id) : on.add(l.id);
      issue.labels = await api(`/repos/${r.full_name}/issues/${n}/labels`, { method: 'PUT', body: { labels: [...on] } });
      labels.replaceChildren(...(issue.labels.length ? issue.labels.map(labelChip) : [el('span', { class: 'small muted', text: 'None yet' })]));
      close();
    } }, on.has(l.id) ? ic('check', 'i s') : el('span', { style: 'width:13px' }), labelChip(l)));
  }) : null;
  const people = (list) => el('p', {}, ...(list?.length ? list.map((u) => el('span', { class: 'hrow', style: 'gap:6px;margin-bottom:4px' }, avatar(u), u.login)) : [el('span', { class: 'small muted', text: 'Nobody' })]));
  const assignees = people(issue.assignees);
  const assignEdit = can ? pop(el('button', { class: 'btn plain', style: 'padding:2px 6px', 'aria-label': 'Change who it is assigned to' }, ic('gear', 'i s')), async (close) => {
    const all = await api(`/repos/${r.full_name}/assignees`);
    const on = new Set((issue.assignees || []).map((u) => u.login));
    return all.map((u) => el('a', { href: '#', 'aria-current': on.has(u.login) ? 'true' : null, onclick: async (e) => {
      e.preventDefault();
      on.has(u.login) ? on.delete(u.login) : on.add(u.login);
      const x = await api(`/repos/${r.full_name}/issues/${n}`, { method: 'PATCH', body: { assignees: [...on] } });
      issue.assignees = x.assignees;
      assignees.replaceWith(people(x.assignees));
      close();
    } }, avatar(u), u.login));
  }) : null;
  const out = el('aside', { class: 'side' });
  if (isPull) {
    const reviewers = people(isPull.requested_reviewers);
    const ask = can ? pop(el('button', { class: 'btn plain', style: 'padding:2px 6px', 'aria-label': 'Ask for a review' }, ic('gear', 'i s')), async (close) => {
      const all = await api(`/repos/${r.full_name}/collaborators`).catch(() => []);
      return all.filter((u) => u.login !== isPull.user.login).map((u) => el('a', { href: '#', onclick: async (e) => {
        e.preventDefault();
        await api(`/repos/${r.full_name}/pulls/${n}/requested_reviewers`, { method: 'POST', body: { reviewers: [u.login] } });
        toast(`Asked ${u.login} to review`);
        close();
        go(location.pathname, true);
      } }, avatar(u), u.login));
    }) : null;
    out.append(section('Reviewers', reviewers, ask));
  }
  out.append(section('Assignees', assignees, assignEdit), section('Labels', labels, labelEdit),
    section('Milestone', el('p', { class: 'small', text: issue.milestone?.title || 'None' })));
  return out;
}

// ---- an issue

route(/^\/([^/]+)\/([^/]+)\/issues\/new$/, async ({ m, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'issues');
  setTitle('New issue', r.full_name);
  const title = el('input', { class: 'write', placeholder: 'Title', 'aria-label': 'Title', required: true });
  const body = el('textarea', { class: 'write', style: 'min-height:180px', placeholder: 'What happened, or what you would like. Markdown works.', 'aria-label': 'Description' });
  const form = el('form', {}, el('div', { class: 'gap' }, title), body, el('div', { class: 'hrow', style: 'justify-content:flex-end;margin-top:8px' }, el('button', { class: 'btn go', type: 'submit', text: 'Open issue' })));
  form.onsubmit = async (e) => {
    e.preventDefault();
    try { const i = await api(`/repos/${r.full_name}/issues`, { method: 'POST', body: { title: title.value, body: body.value } }); go(`/${r.full_name}/issues/${i.number}`); }
    catch (err) { toast(err.message); }
  };
  const w = await whoami();
  if (!current()) return;
  put(main(), el('div', { style: 'max-width:980px' }, el('div', { class: 'comment' }, avatar(w.forge, 'avatar l'), el('div', { class: 'box' }, el('div', { class: 'body' }, form)))));
  title.focus();
});

route(/^\/([^/]+)\/([^/]+)\/issues\/(\d+)$/, async ({ m, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'issues');
  const n = m[3];
  const issue = await api(`/repos/${r.full_name}/issues/${n}`);
  if (issue.pull_request) { go(`/${r.full_name}/pull/${n}`, true); return; }
  if (!current()) return;
  setTitle(`${issue.title} #${n}`, r.full_name);
  const toggle = r.permissions?.push || issue.user.login === (await whoami()).forge?.login
    ? el('button', { class: 'btn', text: issue.state === 'open' ? 'Close issue' : 'Reopen issue', onclick: async () => { await api(`/repos/${r.full_name}/issues/${n}`, { method: 'PATCH', body: { state: issue.state === 'open' ? 'closed' : 'open' } }); go(location.pathname, true); } })
    : null;
  if (!current()) return;
  put(main(), 
    el('h1', { class: 'h1', style: 'font-size:24px;font-weight:600' }, issue.title, el('span', { class: 'muted', style: 'font-weight:400', text: ` #${n}` })),
    el('div', { class: 'hrow', style: 'margin:8px 0 18px' }, el('span', { class: `state ${issue.state === 'open' ? 'open' : 'closed'}` }, ic('issue'), issue.state === 'open' ? 'Open' : 'Closed'), el('span', { class: 'muted' }, el('b', { style: 'color:var(--ink)', text: issue.user.login }), ' opened this ', when(issue.created_at), ` · ${plural(issue.comments, 'comment')}`)),
    el('div', { class: 'two' }, el('div', {}, ...await timeline(r, n, issue), await commentBox(r, n, toggle ? [toggle] : [])), await sidebar(r, n, issue)));
});

// ---- a pull request

async function pullHead(r, p, tab) {
  const t = (k, label, n) => el('a', { href: `/${r.full_name}/pull/${p.number}${k ? `/${k}` : ''}`, 'aria-current': tab === k ? 'page' : null }, label, n !== undefined ? el('span', { class: 'chip', style: 'margin-left:6px', text: String(n) }) : null);
  const state = p.merged ? 'merged' : p.state === 'open' ? 'open' : 'closed';
  const title = el('h1', { class: 'h1', style: 'font-size:24px;font-weight:600' }, p.title, el('span', { class: 'muted', style: 'font-weight:400', text: ` #${p.number}` }));
  if (r.permissions?.push || p.user.login === (await whoami()).forge?.login) {
    const edit = el('button', { class: 'btn plain', style: 'margin-left:10px;vertical-align:4px', text: 'Edit' });
    edit.onclick = () => {
      const input = el('input', { class: 'write', value: p.title, 'aria-label': 'Title' });
      const save = el('button', { class: 'btn go', text: 'Save', onclick: async () => { await api(`/repos/${r.full_name}/pulls/${p.number}`, { method: 'PATCH', body: { title: input.value } }); go(location.pathname, true); } });
      title.replaceWith(el('div', { class: 'hrow' }, input, save, el('button', { class: 'btn', text: 'Cancel', onclick: () => go(location.pathname, true) })));
    };
    title.append(edit);
  }
  return [title,
    el('div', { class: 'hrow', style: 'margin-top:8px' },
      el('span', { class: `state ${state}` }, ic(p.merged ? 'merged' : 'pr'), state[0].toUpperCase() + state.slice(1)),
      p.draft ? el('span', { class: 'chip', text: 'draft' }) : null,
      el('span', { class: 'muted' }, el('b', { style: 'color:var(--ink)', text: p.user.login }), p.merged ? ` merged ${p.commits ? `${plural(p.commits, 'commit')} ` : ''}into ` : ` wants to merge ${p.commits ? `${plural(p.commits, 'commit')} ` : ''}into `, el('code', { text: p.base.label }), ' from ', el('code', { text: p.head.label }))),
    el('nav', { class: 'subtabs', 'aria-label': 'Pull request' }, t('', 'Conversation', p.comments), t('commits', 'Commits', p.commits || undefined), t('checks', 'Checks'), t('files', 'Files changed', p.changed_files))];
}

// Forgejo's link to a job, drawn by our run page instead
const runLink = (url) => {
  const m = (url || '').match(/\/([^/]+\/[^/]+)\/actions\/runs\/(\d+)(?:\/jobs\/(\d+))?/);
  return m ? `/${m[1]}/actions/runs/${m[2]}${m[3] !== undefined ? q({ job: m[3] }) : ''}` : url;
};
const jobName = (ctx) => ctx.replace(/ \((pull_request|push)\)$/, '');

async function mergeBox(r, p, statuses) {
  const combined = statuses.state;
  const byCtx = new Map();
  for (const s of statuses.statuses || []) byCtx.set(s.context, s);
  const all = [...byCtx.values()];
  const failing = all.filter((s) => ['failure', 'error'].includes(s.status));
  const waiting = all.filter((s) => ['pending', 'running', 'waiting'].includes(s.status));
  const box = el('div', { class: `merge${p.merged ? ' done' : failing.length || waiting.length || !p.mergeable ? ' blocked' : ''}` });
  if (p.merged) {
    box.append(el('div', {}, ic('merged'), el('b', { text: 'Merged' }), el('span', { class: 'muted' }, `by ${p.merged_by?.login || 'someone'} `, when(p.merged_at))));
    return box;
  }
  if (p.state !== 'open') {
    box.append(el('div', {}, ic('x'), el('b', { text: 'Closed without merging' })));
    return box;
  }
  box.append(el('div', {}, all.length ? statusDot(failing.length ? 'failure' : waiting.length ? 'running' : 'success') : el('span', { class: 'dot' }),
    el('b', { text: !all.length ? 'No checks' : failing.length ? `${plural(failing.length, 'check')} failed` : waiting.length ? 'Checks are running' : 'All checks passed' }),
    el('span', { class: 'spacer' }), all.length ? el('a', { href: `/${r.full_name}/pull/${p.number}/checks`, text: `${all.length - waiting.length} of ${all.length} done` }) : null));
  box.append(el('div', {}, el('span', { class: `dot ${p.mergeable ? 'ok' : 'bad'}` }), el('span', { text: p.mergeable ? `No conflicts with ${p.base.ref}` : `Conflicts with ${p.base.ref}, or it cannot be merged as it stands` })));
  if (r.permissions?.push) {
    const styles = [['merge', 'Merge commit', r.allow_merge_commits], ['squash', 'Squash and merge', r.allow_squash_merge], ['rebase', 'Rebase and merge', r.allow_rebase], ['fast-forward-only', 'Fast-forward only', r.allow_fast_forward_only_merge]].filter((s) => s[2]);
    const style = el('select', { class: 'btn plain', 'aria-label': 'How to merge' }, ...styles.map(([v, t]) => el('option', { value: v, text: t, selected: v === r.default_merge_style })));
    const drop = el('label', { class: 'small hrow', style: 'gap:6px' }, el('input', { type: 'checkbox', checked: r.default_delete_branch_after_merge }), 'delete the branch after');
    const merge = el('button', { class: 'btn go', disabled: !p.mergeable || failing.length > 0 || waiting.length > 0, text: 'Merge pull request' });
    const later = el('button', { class: 'btn', text: 'Merge when checks pass' });
    const doMerge = async (auto) => {
      try {
        await api(`/repos/${r.full_name}/pulls/${p.number}/merge`, { method: 'POST', body: { Do: style.value, merge_when_checks_succeed: auto, delete_branch_after_merge: drop.querySelector('input').checked, head_commit_id: p.head.sha } });
        toast(auto ? 'It merges when its checks pass' : 'Merged');
        go(location.pathname, true);
      } catch (err) { toast(err.message); }
    };
    merge.onclick = () => doMerge(false);
    later.onclick = () => doMerge(true);
    box.append(el('div', {}, merge, waiting.length && p.mergeable ? later : null, style, drop, el('span', { class: 'small muted', text: failing.length ? 'A failing check blocks the merge.' : '' })));
  }
  return box;
}

route(/^\/([^/]+)\/([^/]+)\/pulls?\/(\d+)(?:\/(commits|checks|files))?$/, async ({ m, current }) => {
  if (m[0].includes('/pulls/')) { go(m[0].replace('/pulls/', '/pull/'), true); return; }
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'pulls');
  const n = m[3], tab = m[4] || '';
  const p = await api(`/repos/${r.full_name}/pulls/${n}`);
  if (!current()) return;
  setTitle(`${p.title} #${n}`, r.full_name);
  const head = await pullHead(r, p, tab);
  const statuses = await api(`/repos/${r.full_name}/commits/${p.head.sha}/status`).catch(() => ({ statuses: [] }));

  if (tab === '') {
    const issue = await api(`/repos/${r.full_name}/issues/${n}`);
    const close = r.permissions?.push && p.state === 'open' && !p.merged ? el('button', { class: 'btn', text: 'Close pull request', onclick: async () => { await api(`/repos/${r.full_name}/pulls/${n}`, { method: 'PATCH', body: { state: 'closed' } }); go(location.pathname, true); } })
      : r.permissions?.push && p.state === 'closed' && !p.merged ? el('button', { class: 'btn', text: 'Reopen', onclick: async () => { await api(`/repos/${r.full_name}/pulls/${n}`, { method: 'PATCH', body: { state: 'open' } }); go(location.pathname, true); } }) : null;
    if (!current()) return;
    put(main(), ...head, el('div', { class: 'two' },
      el('div', {}, ...await timeline(r, n, { ...issue, body: p.body }), await mergeBox(r, p, statuses), el('div', { style: 'height:20px' }), await commentBox(r, n, close ? [close] : [])),
      await sidebar(r, n, issue, p)));
    return;
  }
  if (tab === 'commits') {
    const cs = await api(`/repos/${r.full_name}/pulls/${n}/commits${q({ limit: 250, stat: false, verification: false, files: false })}`);
    if (!current()) return;
    put(main(), ...head, el('div', { class: 'box list' }, ...cs.map((c) => el('div', { class: 'item' },
      el('div', { style: 'flex:1;min-width:0' }, el('a', { class: 't', href: `/${r.full_name}/commit/${c.sha}`, text: firstLine(c.commit.message) }), el('div', { class: 'sub hrow', style: 'gap:6px' }, avatar(c.author), c.author?.login || c.commit.author.name, ' ', when(c.commit.author.date))),
      el('a', { class: 'btn mono plain', href: `/${r.full_name}/commit/${c.sha}`, text: short(c.sha) })))));
    return;
  }
  if (tab === 'checks') {
    const byCtx = new Map();
    for (const s of statuses.statuses || []) byCtx.set(s.context, s);
    const rows = [...byCtx.values()].sort((a, b) => a.context.localeCompare(b.context));
    const runUrl = rows.map((s) => runLink(s.target_url)).find((u) => u?.includes('/actions/runs/'));
    if (!current()) return;
    put(main(), ...head, el('div', { class: 'box' },
      el('header', {}, statusDot(statuses.state), el('b', { text: rows.length ? `Checks for ${short(p.head.sha)}` : 'No checks ran' }), el('span', { class: 'spacer' }), runUrl ? el('a', { class: 'btn', href: runUrl.split('?')[0], text: 'Open the run' }) : null),
      el('div', { class: 'list' }, ...rows.map((s) => el('a', { href: runLink(s.target_url) || '#' }, statusDot(s.status), el('span', { text: jobName(s.context) }), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: s.description || '' }))))));
    return;
  }
  // files changed
  const patch = await text(`/api/v1/repos/${r.full_name}/pulls/${n}.diff`);
  const files = diff.parse(patch);
  const key = `dd-git-viewed:${r.full_name}#${n}@${p.head.sha}`;
  let seen = new Set();
  try { seen = new Set(JSON.parse(localStorage.getItem(key) || '[]')); } catch { /* private window */ }
  const count = el('span', { class: 'small muted' });
  const countSeen = () => { count.textContent = `${seen.size} of ${files.length} viewed`; };
  countSeen();
  const viewed = (f, box) => {
    const name = f.to || f.from;
    const cb = el('input', { type: 'checkbox', checked: seen.has(name) });
    if (seen.has(name)) box.classList.add('folded');
    cb.onchange = () => {
      cb.checked ? seen.add(name) : seen.delete(name);
      box.classList.toggle('folded', cb.checked);
      try { localStorage.setItem(key, JSON.stringify([...seen])); } catch { /* ok */ }
      countSeen();
    };
    return el('label', { class: 'small hrow', style: 'gap:4px' }, cb, 'Viewed');
  };
  // the line comments of every review so far, by file, side and line
  const reviews = await api(`/repos/${r.full_name}/pulls/${n}/reviews`).catch(() => []);
  const said = (await Promise.all(reviews.filter((v) => v.comments_count).map((v) => api(`/repos/${r.full_name}/pulls/${n}/reviews/${v.id}/comments`).catch(() => [])))).flat();
  const noteRow = async (c) => el('tr', { class: 'note-row' }, el('td', { colspan: 3 }, el('div', { class: 'box linenote' },
    el('header', {}, avatar(c.user), el('b', { text: c.user.login }), el('span', { class: 'muted small' }, when(c.created_at))),
    el('div', { class: 'body' }, await markdown(c.body)))));
  const drawn = new Map();
  for (const c of said) {
    const side = c.position ? 'new' : 'old';
    const line = c.position || c.original_position;
    const k = `${c.path}:${side}:${line}`;
    if (!drawn.has(k)) drawn.set(k, []);
    drawn.get(k).push(await noteRow(c));
  }
  const talk = {
    notes: (path, side, line) => drawn.get(`${path}:${side}:${line}`) || [],
    write: r.permissions?.pull ? (path, side, line, tr) => {
      if (tr.nextElementSibling?.classList.contains('note-form')) return;
      const ta = el('textarea', { class: 'write', style: 'min-height:80px', placeholder: `A comment on line ${line}. Markdown works.`, 'aria-label': 'Comment' });
      const form = el('tr', { class: 'note-form' }, el('td', { colspan: 3 }, el('div', { class: 'box linenote' }, el('div', { class: 'body' }, ta,
        el('div', { class: 'hrow', style: 'justify-content:flex-end;margin-top:8px' },
          el('button', { class: 'btn', type: 'button', text: 'Cancel', onclick: () => form.remove() }),
          el('button', { class: 'btn go', type: 'button', text: 'Comment', onclick: async () => {
            if (!ta.value.trim()) return;
            try {
              await api(`/repos/${r.full_name}/pulls/${n}/reviews`, { method: 'POST', body: { event: 'COMMENT', commit_id: p.head.sha, body: '', comments: [{ path, body: ta.value, [side === 'new' ? 'new_position' : 'old_position']: line }] } });
              const mine = await noteRow({ user: w.forge, created_at: new Date().toISOString(), body: ta.value });
              form.replaceWith(mine);
            } catch (err) { toast(err.message); }
          } }))))));
      tr.after(form);
      ta.focus();
    } : null,
  };
  const w = await whoami();
  const holder = el('div');
  const { seg, mode } = diff.modeSwitch((md) => holder.replaceChildren(...diff.render(files, { mode: md, viewed, talk })));
  holder.replaceChildren(...diff.render(files, { mode: mode(), viewed, talk }));
  const fileList = el('aside', { class: 'box tree', 'aria-label': 'Files' }, el('div', { class: 'small muted', style: 'padding:4px 8px', text: plural(files.length, 'file') }),
    ...files.map((f, i) => el('a', { href: `#diff-${i}`, title: f.to }, ic('file'), el('span', { style: 'overflow:hidden;text-overflow:ellipsis', text: (f.to || f.from).split('/').pop() }), el('span', { class: 'adds small', text: ` +${f.adds}` }), el('span', { class: 'dels small', text: ` −${f.dels}` }))));
  if (!current()) return;
  put(main(), ...head,
    el('div', { class: 'hrow gap' }, diff.summary(files), el('span', { class: 'spacer' }), count, seg),
    el('div', { class: 'withtree' }, fileList, holder));
});

// ---- comparing two branches, and opening a pull request from it

route(/^\/([^/]+)\/([^/]+)\/compare(?:\/(.+?)\.\.\.(.+))?$/, async ({ m, current }) => {
  const r = await repo(m[1], m[2]);
  if (!current()) return;
  header(r, 'pulls');
  setTitle('New pull request', r.full_name);
  const { branches } = await refs(r);
  const base = m[3] || r.default_branch;
  const headRef = m[4] || branches.map((b) => b.name).find((b) => b !== base) || base;
  const picker = (value, which) => {
    const sel = el('select', { class: 'btn plain', 'aria-label': which }, ...branches.map((b) => el('option', { value: b.name, text: b.name, selected: b.name === value })));
    sel.onchange = () => go(`/${r.full_name}/compare/${which === 'into' ? sel.value : base}...${which === 'from' ? sel.value : headRef}`);
    return sel;
  };
  const top = el('div', { class: 'box gap' }, el('div', { class: 'hrow', style: 'padding:10px 14px' }, ic('branch'), 'into', picker(base, 'into'), 'from', picker(headRef, 'from')));
  if (base === headRef) { put(main(), el('h1', { class: 'h1 gap', text: 'New pull request' }), top, el('p', { class: 'muted', text: 'Pick two different branches.' })); return; }
  const [cmp, existing] = await Promise.all([
    api(`/repos/${r.full_name}/compare/${encodeURIComponent(base)}...${encodeURIComponent(headRef)}`),
    api(`/repos/${r.full_name}/pulls/${encodeURIComponent(base)}/${encodeURIComponent(headRef)}`).catch(() => null),
  ]);
  if (!current()) return;
  const commits = cmp.commits || [];
  const title = el('input', { class: 'write', 'aria-label': 'Title', value: commits.length === 1 ? firstLine(commits[0].commit.message) : headRef.replace(/[-_]/g, ' ') });
  const body = el('textarea', { class: 'write', 'aria-label': 'Description', placeholder: 'What this changes and why. Markdown works.' });
  const open = async (draft) => {
    try {
      const p = await api(`/repos/${r.full_name}/pulls`, { method: 'POST', body: { base, head: headRef, title: `${draft ? 'WIP: ' : ''}${title.value}`, body: body.value } });
      go(`/${r.full_name}/pull/${p.number}`);
    } catch (err) { toast(err.message); }
  };
  const w = await whoami();
  const files = cmp.files || [];
  if (!current()) return;
  put(main(), 
    el('h1', { class: 'h1 gap', text: 'New pull request' }), top,
    existing ? el('div', { class: 'box gap', style: 'padding:12px 16px' }, 'There is already one for these branches: ', el('a', { href: `/${r.full_name}/pull/${existing.number}`, text: `#${existing.number} ${existing.title}` }))
      : !commits.length ? el('p', { class: 'muted', text: `${headRef} has nothing ${base} does not.` })
        : el('div', { class: 'comment' }, avatar(w.forge, 'avatar l'), el('div', { class: 'box' }, el('div', { class: 'body' }, el('div', { class: 'gap' }, title), body,
          el('div', { class: 'hrow', style: 'justify-content:flex-end;margin-top:8px' }, el('button', { class: 'btn', text: 'Open as draft', onclick: () => open(true) }), el('button', { class: 'btn go', text: 'Open pull request', onclick: () => open(false) }))))),
    commits.length ? [el('div', { class: 'hrow', style: 'margin:10px 0' }, el('b', { text: `${plural(commits.length, 'commit')}${files.length ? ` · ${plural(files.length, 'file')} changed` : ''}` })),
      el('div', { class: 'box list gap' }, ...commits.map((c) => el('div', {}, ic('commit'), el('a', { href: `/${r.full_name}/commit/${c.sha}`, text: firstLine(c.commit.message) }), el('span', { class: 'spacer' }), el('span', { class: 'mono small muted', text: short(c.sha) })))),
      files.length ? el('div', { class: 'box list' }, ...files.map((f) => el('div', {}, ic('file'), el('span', { class: 'mono small', text: f.filename }), el('span', { class: 'spacer' }), el('span', { class: 'small' }, el('span', { class: 'adds', text: `+${f.additions ?? ''}` }), ' ', el('span', { class: 'dels', text: `−${f.deletions ?? ''}` }))))) : null] : null);
});
