// Git, beyond any one repository: your repositories and your year, a
// person's page, a new repository, notifications, search, and your own
// settings (API tokens, keys, profile).

import {
  $, app, el, ic, api, q, ago, when, plural, avatar, route, go, who, whoami, statusDot,
  markdown, toast, copy, setTitle, firstLine, short, fail, bell,
} from './git-core.js';
import { heatmap, dayKey } from './git-charts.js';

const main = () => app();
const noHead = () => $('rhead').replaceChildren();
const vis = (r) => (r.private ? 'Private' : r.internal ? 'Members' : 'Public');

// ---- a repository in a list

async function repoRow(r) {
  const dot = el('span', { class: 'dot', title: 'no checks' });
  const row = el('div', { class: 'item' },
    ic(r.private ? 'lock' : 'repo'),
    el('div', { style: 'flex:1;min-width:0' },
      el('div', {}, el('a', { class: 't', href: `/${r.full_name}`, text: r.full_name }), ' ', el('span', { class: 'pill', text: r.archived ? 'Archived' : vis(r) })),
      r.description ? el('div', { class: 'sub', text: r.description }) : null,
      el('div', { class: 'sub hrow', style: 'gap:6px' }, dot, r.language ? el('span', { text: r.language }) : null, el('span', {}, 'updated ', when(r.updated_at)))),
    r.open_pr_counter ? el('a', { class: 'chip', href: `/${r.full_name}/pulls`, text: plural(r.open_pr_counter, 'open pull request') }) : null);
  api(`/repos/${r.full_name}/commits/${encodeURIComponent(r.default_branch)}/status`)
    .then((s) => { if (s?.total_count) dot.replaceWith(statusDot(s.state)); })
    .catch(() => {});
  return row;
}

// contributions by day, from the forge's own count
async function year(user) {
  const rows = await api(`/users/${encodeURIComponent(user)}/heatmap`).catch(() => []);
  const days = {};
  let total = 0;
  for (const { timestamp, contributions } of rows || []) {
    const k = dayKey(timestamp * 1000);
    days[k] = (days[k] || 0) + contributions;
    total += contributions;
  }
  return el('div', { class: 'box gap' },
    el('header', {}, el('b', { text: `${plural(total, 'contribution')} in the last year` })),
    heatmap(days));
}

// what happened, in words
export function feedLine(a) {
  const u = a.act_user?.login || 'someone';
  const repo = a.repo?.full_name;
  const R = repo ? el('a', { href: `/${repo}`, text: repo }) : '';
  let content = {};
  try { content = JSON.parse(a.content || '{}'); } catch { /* plain text */ }
  const ref = (a.ref_name || '').replace(/^refs\/(heads|tags)\//, '');
  // an issue or pull request is ["number","title"]; a comment "number|text"
  const n = Array.isArray(content) ? content[0] : (a.content || '').split('|')[0];
  const pick = {
    create_repo: ['repo', [u, ' made ', R]],
    commit_repo: ['commit', [u, ' pushed ', plural(content.Len || content.Commits?.length || 1, 'commit'), ' to ', el('code', { text: ref }), ' in ', R]],
    push_tag: ['tag', [u, ' tagged ', el('code', { text: ref }), ' in ', R]],
    delete_branch: ['branch', [u, ' deleted ', el('code', { text: ref }), ' in ', R]],
    create_pull_request: ['pr', [u, ' opened ', el('a', { href: `/${repo}/pull/${n}`, text: `#${n}` }), ' in ', R]],
    merge_pull_request: ['merged', [u, ' merged ', el('a', { href: `/${repo}/pull/${n}`, text: `#${n}` }), ' in ', R]],
    auto_merge_pull_request: ['merged', ['#', el('a', { href: `/${repo}/pull/${n}`, text: n }), ' merged by itself when its checks passed']],
    close_pull_request: ['pr', [u, ' closed ', el('a', { href: `/${repo}/pull/${n}`, text: `#${n}` })]],
    comment_pull: ['pr', [u, ' commented on ', el('a', { href: `/${repo}/pull/${n}`, text: `#${n}` })]],
    approve_pull_request: ['check', [u, ' approved ', el('a', { href: `/${repo}/pull/${n}`, text: `#${n}` })]],
    create_issue: ['issue', [u, ' opened ', el('a', { href: `/${repo}/issues/${n}`, text: `#${n}` }), ' in ', R]],
    close_issue: ['issue', [u, ' closed ', el('a', { href: `/${repo}/issues/${n}`, text: `#${n}` })]],
    comment_issue: ['issue', [u, ' commented on ', el('a', { href: `/${repo}/issues/${n}`, text: `#${n}` })]],
    publish_release: ['tag', [u, ' published a release in ', R]],
  }[a.op_type] || ['pulse', [u, ` ${a.op_type.replace(/_/g, ' ')} `, R]];
  return el('div', {}, ic(pick[0]), el('span', { style: 'flex:1;min-width:0' }, ...pick[1]), el('span', { class: 'small muted' }, when(a.created)));
}

// ---- your repositories

route(/^\/$/, async ({ current }) => {
  noHead();
  setTitle('Your repositories');
  const w = await whoami();
  const user = w.forge?.login || w.name;
  const [repos, reviews, assigned, feed] = await Promise.all([
    api(`/user/repos${q({ limit: 50 })}`),
    api(`/repos/issues/search${q({ type: 'pulls', state: 'open', review_requested: true, limit: 10 })}`).catch(() => []),
    api(`/repos/issues/search${q({ state: 'open', assigned: true, limit: 10 })}`).catch(() => []),
    api(`/users/${encodeURIComponent(user)}/activities/feeds${q({ limit: 12 })}`).catch(() => []),
  ]);
  if (!current()) return;
  repos.sort((a, b) => new Date(b.updated_at) - new Date(a.updated_at));
  const list = el('div', { class: 'box list gap' }, ...(repos.length ? await Promise.all(repos.map(repoRow)) : [el('div', { class: 'empty' }, 'No repositories yet. ', el('a', { href: '/new', text: 'Make one' }))]));
  const filter = el('input', { class: 'btn plain', style: 'width:220px', placeholder: 'Find a repository', 'aria-label': 'Find a repository' });
  filter.oninput = () => {
    const s = filter.value.trim().toLowerCase();
    for (const row of list.children) row.hidden = !!s && !row.textContent.toLowerCase().includes(s);
  };
  const waiting = [...reviews, ...assigned.filter((i) => !reviews.some((r) => r.id === i.id))];
  main().replaceChildren(el('div', { class: 'two' },
    el('div', {},
      el('div', { class: 'hrow gap' }, el('h1', { class: 'h1', text: 'Your repositories' }), el('span', { class: 'spacer' }), filter, el('a', { class: 'btn go', href: '/new' }, ic('plus'), 'New')),
      list,
      await year(user)),
    el('aside', { class: 'side' },
      el('section', {}, el('h3', { text: 'Waiting on you' }),
        waiting.length ? waiting.map((i) => el('p', {},
          el('a', { href: `/${i.repository.full_name}/${i.pull_request ? 'pull' : 'issues'}/${i.number}` }, ic(i.pull_request ? 'pr' : 'issue'), ` #${i.number} ${i.title}`),
          el('br'), el('span', { class: 'small muted', text: `${i.repository.full_name} · ${reviews.includes(i) ? 'review asked' : 'assigned to you'}` })))
          : el('p', { class: 'small muted', text: 'Nothing: no reviews asked of you, nothing assigned.' })),
      el('section', {}, el('h3', { text: 'Your settings' }), el('p', { class: 'small' }, el('a', { href: '/settings/tokens', text: 'API tokens' }), ' · ', el('a', { href: '/settings/keys', text: 'SSH keys' }), ' · ', el('a', { href: '/settings/profile', text: 'Profile' }))),
      el('section', {}, el('h3', { text: 'Recent activity' }),
        feed.length ? el('div', { class: 'list small', style: 'margin:0 -14px' }, ...feed.map(feedLine)) : el('p', { class: 'small muted', text: 'Quiet so far.' })))));
});

// ---- a new repository

route(/^\/new$/, async () => {
  noHead();
  setTitle('A new repository');
  const w = await whoami();
  const owner = w.forge?.login || w.name;
  const name = el('input', { id: 'n-name', required: true, pattern: '[A-Za-z0-9._-]+', placeholder: 'garden-sensors', autocomplete: 'off' });
  const desc = el('input', { id: 'n-desc' });
  const choice = (value, title, words, checked) => el('label', { class: 'choice' }, el('input', { type: 'radio', name: 'vis', value, checked }), el('span', {}, el('b', { text: title }), el('br'), el('small', { class: 'muted', text: words })));
  const readme = el('input', { type: 'checkbox', checked: true });
  const ignore = el('select', { 'aria-label': 'Language for .gitignore' }, ...['None', 'Rust', 'C', 'Python', 'Go', 'Node', 'Nix'].map((l) => el('option', { text: l })));
  const out = el('div');
  const form = el('form', { style: 'max-width:760px' },
    el('h1', { class: 'h1', text: 'A new repository' }),
    el('p', { class: 'muted', text: "A place for a project's files and their history." }),
    el('div', { class: 'field' }, el('label', { for: 'n-name', text: 'Name' }), el('div', { class: 'hrow' }, el('span', { class: 'btn', style: 'cursor:default', text: owner }), el('span', { class: 'muted', text: '/' }), name), el('small', { text: 'Letters, numbers, dashes: it becomes part of the address.' })),
    el('div', { class: 'field' }, el('label', { for: 'n-desc' }, 'What it is ', el('span', { class: 'muted', style: 'font-weight:400', text: '(optional)' })), desc),
    el('div', { class: 'field' }, el('span', { class: 'lbl', text: 'Who can see it' }),
      choice('public', 'Everyone', 'Anyone can read it; only you and the people you add can change it.', false),
      choice('private', 'Only me', 'And the people you add in its settings.', true),
      choice('sealed', 'Only me, encrypted', 'Sealed on your devices; the box stores it but cannot read it. No pull requests or Actions: those need the box to read the code.', false)),
    el('div', { class: 'field', id: 'n-start' }, el('span', { class: 'lbl', text: 'Start with' }),
      el('label', { class: 'choice' }, readme, el('span', { text: 'A README' })),
      el('label', { class: 'choice' }, el('span', {}, 'A .gitignore for '), ignore)),
    el('button', { class: 'btn go', type: 'submit', text: 'Make repository' }),
    out);
  form.onchange = () => { $('n-start').hidden = form.vis.value === 'sealed'; };
  form.onsubmit = async (e) => {
    e.preventDefault();
    out.replaceChildren();
    const n = name.value.trim();
    if (form.vis.value === 'sealed') {
      const url = `dd::${location.origin}/${owner}/${n}.git`;
      out.append(el('div', { class: 'box', style: 'margin-top:16px' },
        el('header', {}, el('b', { text: 'From a checkout on your device' })),
        el('div', { class: 'md pad' }, el('pre', { text: `git remote add origin ${url}\ngit push -u origin main` }),
          el('p', { class: 'small muted', text: 'The push makes it, sealed. dd repo share gives another of your devices the key.' }))));
      return;
    }
    try {
      const r = await api('/user/repos', { method: 'POST', body: { name: n, description: desc.value, private: form.vis.value === 'private', auto_init: readme.checked, readme: 'Default', default_branch: 'main', gitignores: ignore.value === 'None' ? '' : ignore.value } });
      go(`/${r.full_name}`);
    } catch (err) {
      out.append(el('p', { class: 'err', text: err.message }));
    }
  };
  main().replaceChildren(form);
  name.focus();
});

// ---- notifications

route(/^\/notifications$/, async ({ params }) => {
  noHead();
  setTitle('Notifications');
  const all = params.get('all') === '1';
  const items = await api(`/notifications${q({ all, limit: 50 })}`);
  const markAll = el('button', { class: 'btn plain', text: 'Mark all read' });
  markAll.onclick = async () => { await api('/notifications', { method: 'PUT' }); bell(); go('/notifications', true); };
  const link = (n) => {
    const s = n.subject;
    const m = (s.html_url || s.url || '').match(/\/([^/]+\/[^/]+)\/(pulls|issues)\/(\d+)/);
    if (m) return `/${m[1]}/${m[2] === 'pulls' ? 'pull' : 'issues'}/${m[3]}`;
    return `/${n.repository.full_name}`;
  };
  const icon = (t) => ({ Pull: 'pr', Issue: 'issue', Commit: 'commit', Repository: 'repo' }[t] || 'bell');
  main().replaceChildren(el('div', { style: 'max-width:980px' },
    el('div', { class: 'hrow gap' }, el('h1', { class: 'h1', text: 'Notifications' }), el('span', { class: 'spacer' }),
      el('span', { class: 'gseg' }, el('a', { class: 'btn', href: '/notifications', 'aria-pressed': String(!all), text: 'Unread' }), el('a', { class: 'btn', href: '/notifications?all=1', 'aria-pressed': String(all), text: 'All' })),
      all ? null : markAll),
    el('div', { class: 'box list' }, ...(items.length ? items.map((n) => {
      const a = el('a', { class: 'item', href: link(n) },
        ic(icon(n.subject.type)),
        el('div', { style: 'flex:1;min-width:0' }, el('div', { class: 't', text: n.subject.title }), el('div', { class: 'sub', text: `${n.repository.full_name}${n.unread ? '' : ' · read'}` })),
        el('span', { class: 'small muted' }, when(n.updated_at)));
      a.addEventListener('click', () => { if (n.unread) api(`/notifications/threads/${n.id}`, { method: 'PATCH' }).then(bell).catch(() => {}); });
      return a;
    }) : [el('div', { class: 'empty', text: all ? 'Nothing yet.' : 'All caught up.' })]))));
});

// ---- search

route(/^\/search$/, async ({ params, current }) => {
  noHead();
  const text = params.get('q') || '';
  const kind = params.get('type') || 'repositories';
  setTitle(`Search: ${text}`);
  const input = document.querySelector('#git-search input');
  if (input) input.value = text;
  const [repos, pulls, issues] = await Promise.all([
    api(`/repos/search${q({ q: text, limit: 30 })}`).then((r) => r.data || []),
    api(`/repos/issues/search${q({ q: text, type: 'pulls', state: 'all', limit: 30 })}`).catch(() => []),
    api(`/repos/issues/search${q({ q: text, type: 'issues', state: 'all', limit: 30 })}`).catch(() => []),
  ]);
  if (!current()) return;
  const tab = (k, label, n) => el('a', { href: `/search${q({ q: text, type: k })}`, 'aria-current': kind === k ? 'page' : null }, label, el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: String(n) }));
  const issueRow = (i) => el('a', { class: 'item', href: `/${i.repository.full_name}/${i.pull_request ? 'pull' : 'issues'}/${i.number}` },
    ic(i.pull_request ? (i.pull_request.merged ? 'merged' : 'pr') : 'issue'),
    el('div', { style: 'flex:1' }, el('div', { class: 't', text: i.title }), el('div', { class: 'sub', text: `${i.repository.full_name} #${i.number} · ${i.state}` })));
  const body = kind === 'pulls' ? pulls.map(issueRow) : kind === 'issues' ? issues.map(issueRow) : await Promise.all(repos.map(repoRow));
  main().replaceChildren(el('div', { class: 'two left' },
    el('aside', { class: 'jobs' }, tab('repositories', 'Repositories', repos.length), tab('pulls', 'Pull requests', pulls.length), tab('issues', 'Issues', issues.length)),
    el('div', {},
      el('p', { class: 'muted', style: 'margin-top:0' }, `${body.length} result${body.length === 1 ? '' : 's'} for `, el('b', { style: 'color:var(--ink)', text: text })),
      el('div', { class: 'box list' }, ...(body.length ? body : [el('div', { class: 'empty', text: 'Nothing matches.' })])),
      el('p', { class: 'gnote', text: 'Searching inside files is not switched on for this forge; this looks at names, titles and descriptions.' }))));
});

// ---- your settings: tokens, keys, profile

const SCOPES = [
  ['repository', 'Code', ['None', 'Read', 'Read and write']],
  ['issue', 'Issues and pull requests', ['None', 'Read', 'Read and write']],
  ['user', 'Your account', ['None', 'Read', 'Read and write']],
  ['notification', 'Notifications', ['None', 'Read', 'Read and write']],
  ['organization', 'Organisations', ['None', 'Read', 'Read and write']],
  ['package', 'Packages', ['None', 'Read', 'Read and write']],
];

route(/^\/settings(?:\/(tokens|keys|profile))?$/, async ({ m }) => {
  noHead();
  const pane = m[1] || 'tokens';
  const w = await whoami();
  const user = w.forge?.login || w.name;
  setTitle('Your settings');
  const nav = el('nav', {}, ...[['tokens', 'API tokens'], ['keys', 'SSH & signing keys'], ['profile', 'Profile']].map(([k, t]) => el('a', { href: `/settings/${k}`, 'aria-current': pane === k ? 'page' : null, text: t })));
  const body = el('div');
  main().replaceChildren(
    el('div', { class: 'hrow gap' }, avatar(w.forge, 'avatar l'), el('div', {}, el('h1', { class: 'h1', text: user }), el('span', { class: 'small muted', text: 'Your settings for Git. Your name, passkeys and devices are Commonty\'s, under your name in the bar.' }))),
    el('div', { class: 'settings' }, nav, body));

  if (pane === 'tokens') {
    const tokens = await api(`/users/${encodeURIComponent(user)}/tokens`);
    const shown = el('div');
    const name = el('input', { id: 't-n', placeholder: 'renovate on node2', required: true });
    const repos = el('input', { id: 't-r', placeholder: 'david/commonty, david/dbcan' });
    const picks = SCOPES.map(([k, label, opts]) => [k, el('select', { 'aria-label': label }, ...opts.map((o, i) => el('option', { value: ['', 'read', 'write'][i], text: o })))]);
    picks[0][1].value = 'read';
    const form = el('form', { class: 'box gap' },
      el('header', {}, el('b', { text: 'A new token' })),
      el('div', { style: 'padding:14px 16px' },
        el('div', { class: 'field' }, el('label', { for: 't-n', text: 'Name' }), name, el('small', { text: 'What it is for, so you know what breaks if you delete it.' })),
        el('div', { class: 'field' }, el('label', { for: 't-r' }, 'Only these repositories ', el('span', { class: 'muted', style: 'font-weight:400', text: '(optional)' })), repos, el('small', { text: 'Leave empty for all of yours.' })),
        el('div', { class: 'field', style: 'max-width:640px' }, el('span', { class: 'lbl', text: 'What it may do' }),
          el('div', { class: 'box list' }, ...picks.map(([k, sel], i) => el('div', {}, el('b', { text: SCOPES[i][1] }), el('span', { class: 'spacer' }), sel)))),
        el('button', { class: 'btn go', type: 'submit', text: 'Make token' })));
    form.onsubmit = async (e) => {
      e.preventDefault();
      const scopes = picks.filter(([, s]) => s.value).map(([k, s]) => `${s.value}:${k}`);
      if (!scopes.length) { toast('Give it something to do'); return; }
      try {
        const t = await api(`/users/${encodeURIComponent(user)}/tokens`, { method: 'POST', body: { name: name.value.trim(), scopes, repositories: repos.value.split(',').map((s) => s.trim()).filter(Boolean) } });
        const val = el('input', { readonly: true, value: t.sha1, 'aria-label': 'The new token' });
        const cp = el('button', { 'aria-label': 'Copy' }, ic('copy'));
        cp.onclick = () => copy(t.sha1, cp);
        shown.replaceChildren(el('div', { class: 'box gap', style: 'border-color:var(--accent)' }, el('div', { style: 'padding:12px 16px' }, el('b', { text: 'Copy it now: it is shown once.' }), el('div', { class: 'clone', style: 'margin-top:8px;max-width:560px' }, val, cp))));
        list.prepend(tokenRow(t));
        form.reset();
      } catch (err) { toast(err.message); }
    };
    const tokenRow = (t) => {
      const del = el('button', { class: 'btn bad plain', text: 'Delete' });
      const row = el('div', {}, ic('key'), el('div', { style: 'flex:1;min-width:0' }, el('b', { text: t.name }), el('div', { class: 'sub mono', text: (t.scopes || []).join(' · ') || 'no scopes' })), el('span', { class: 'small muted', text: `ends in ${t.token_last_eight ? '…' + t.token_last_eight : ''}` }), del);
      del.onclick = async () => {
        if (!confirm(`Delete the token "${t.name}"? Whatever uses it stops working.`)) return;
        await api(`/users/${encodeURIComponent(user)}/tokens/${t.id}`, { method: 'DELETE' });
        row.remove();
      };
      return row;
    };
    const list = el('div', { class: 'box list' }, ...tokens.map(tokenRow));
    body.append(el('h2', { class: 'ph', text: 'API tokens' }),
      el('p', { class: 'muted small', text: 'For scripts and other programs that use the Git API as you. On your own devices the dd cli signs in for you and needs none of these.' }),
      shown, form, tokens.length ? list : el('p', { class: 'muted', text: 'No tokens yet.' }));
  }

  if (pane === 'keys') {
    const [keys, gpg] = await Promise.all([api('/user/keys'), api('/user/gpg_keys').catch(() => [])]);
    const keyRow = (k, path, label) => {
      const del = el('button', { class: 'btn bad plain', text: 'Delete' });
      const row = el('div', {}, ic('key'), el('div', { style: 'flex:1;min-width:0' }, el('b', { text: label }), el('div', { class: 'sub mono', text: k.fingerprint || k.key_id || '' })), el('span', { class: 'small muted', text: k.last_used_at && !k.last_used_at.startsWith('0001') ? `used ${ago(k.last_used_at)}` : 'never used' }), del);
      del.onclick = async () => { if (confirm(`Delete "${label}"?`)) { await api(`${path}/${k.id}`, { method: 'DELETE' }); row.remove(); } };
      return row;
    };
    const title = el('input', { placeholder: 'laptop', required: true, 'aria-label': 'Key name' });
    const key = el('textarea', { class: 'write', style: 'min-height:80px', placeholder: 'ssh-ed25519 AAAA…', required: true, 'aria-label': 'Public key' });
    const add = el('form', { class: 'box gap' }, el('header', {}, el('b', { text: 'Add an SSH key' })), el('div', { style: 'padding:12px 16px;display:grid;gap:8px;max-width:640px' }, title, key, el('div', {}, el('button', { class: 'btn go', type: 'submit', text: 'Add key' }))));
    add.onsubmit = async (e) => {
      e.preventDefault();
      try { const k = await api('/user/keys', { method: 'POST', body: { title: title.value, key: key.value.trim() } }); sshList.append(keyRow(k, '/user/keys', k.title)); add.reset(); } catch (err) { toast(err.message); }
    };
    const sshList = el('div', { class: 'box list gap' }, ...keys.map((k) => keyRow(k, '/user/keys', k.title)));
    body.append(el('h2', { class: 'ph', text: 'SSH keys' }), el('p', { class: 'muted small', text: 'For git over SSH, and for signing commits with that same key: a commit signed with one of these shows as Verified. The dd cli needs none.' }),
      keys.length ? sshList : el('p', { class: 'muted', text: 'No keys yet.' }), add,
      el('h2', { class: 'ph', style: 'margin-top:24px', text: 'GPG keys' }),
      gpg.length ? el('div', { class: 'box list' }, ...gpg.map((k) => keyRow(k, '/user/gpg_keys', k.key_id))) : el('p', { class: 'muted small', text: 'None. SSH keys sign commits too.' }));
  }

  if (pane === 'profile') {
    const s = await api('/user/settings');
    const full = el('input', { id: 'p-n', value: s.full_name || '' });
    const form = el('form', {},
      el('h2', { class: 'ph', text: 'Profile' }),
      el('div', { class: 'field' }, el('label', { for: 'p-n', text: 'Name' }), full, el('small', { text: `Shown beside ${user} on your commits and comments.` })),
      el('div', { class: 'field' }, el('span', { class: 'lbl', text: 'Email on commits made here' }), el('span', { class: 'mono', text: w.forge?.email || '' }), el('small', { text: 'A no-reply address: your own stays private.' })),
      el('button', { class: 'btn go', type: 'submit', text: 'Save' }));
    form.onsubmit = async (e) => { e.preventDefault(); await api('/user/settings', { method: 'PATCH', body: { full_name: full.value } }); toast('Saved'); };
    body.append(form);
  }
});

// ---- a person's page

route(/^\/([^/]+)$/, async ({ m, current }) => {
  noHead();
  const user = m[1];
  const [u, repos] = await Promise.all([api(`/users/${encodeURIComponent(user)}`), api(`/users/${encodeURIComponent(user)}/repos${q({ limit: 50 })}`)]);
  if (!current()) return;
  setTitle(u.login);
  repos.sort((a, b) => new Date(b.updated_at) - new Date(a.updated_at));
  main().replaceChildren(el('div', { class: 'two' },
    el('div', {},
      el('div', { class: 'hrow gap' }, el('h1', { class: 'h1', text: `${u.login}'s repositories` })),
      el('div', { class: 'box list gap' }, ...(repos.length ? await Promise.all(repos.map(repoRow)) : [el('div', { class: 'empty', text: 'Nothing you may see.' })])),
      await year(u.login)),
    el('aside', { class: 'side' }, el('section', {}, avatar(u, 'avatar l'), el('h3', { style: 'margin-top:8px', text: u.full_name || u.login }), el('p', { class: 'small muted', text: `here since ${new Date(u.created).toLocaleDateString(undefined, { month: 'long', year: 'numeric' })}` })))));
});

export { repoRow, year };
