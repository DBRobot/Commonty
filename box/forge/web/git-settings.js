// A repository's settings: what it is, who works on it, the rules its
// branches and tags keep, Actions' secrets and runners, webhooks, deploy
// keys, labels and milestones, its mirror, and the careful things.

import { put, app, el, ic, api, q, when, ago, plural, avatar, route, go, toast, setTitle, pop } from './git-core.js';
import { repo, header, forget, fleetRepoName } from './git-repo.js';

const main = () => app();
const PANES = [
  ['general', 'General'], ['people', 'People'], ['rules', 'Rulesets'], ['actions', 'Actions: secrets & variables'], ['runners', 'Runners'],
  ['hooks', 'Webhooks'], ['keys', 'Deploy keys'], ['labels', 'Labels & milestones'], ['mirror', 'Mirror'], ['careful', 'Careful'],
];

const field = (label, input, help) => el('div', { class: 'field' }, el('label', { for: input.id, text: label }), input, help ? el('small', { text: help }) : null);
const check = (label, checked, help) => {
  const cb = el('input', { type: 'checkbox', checked });
  return [cb, el('label', { class: 'choice' }, cb, el('span', {}, el('b', { text: label }), help ? [el('br'), el('small', { class: 'muted', text: help })] : null))];
};
const delBtn = (what, fn) => {
  const b = el('button', { class: 'btn bad plain', text: 'Delete' });
  b.onclick = async () => { if (confirm(`Delete ${what}?`)) { try { await fn(); } catch (e) { toast(e.message); } } };
  return b;
};

route(/^\/([^/]+)\/([^/]+)\/settings(?:\/([a-z]+))?$/, async ({ m, current }) => {
  const r = await repo(m[1], m[2], true);
  if (!current()) return;
  header(r, 'settings');
  const pane = m[3] || 'general';
  setTitle('Settings', r.full_name);
  if (!r.permissions?.admin) { put(main(), el('div', { class: 'box empty', text: 'Only the people who run this repository see its settings.' })); return; }
  const body = el('div', { style: 'min-width:0' });
  if (!current()) return;
  put(main(), el('div', { class: 'settings' },
    el('nav', {}, ...PANES.map(([k, t]) => el('a', { href: `/${r.full_name}/settings/${k}`, 'aria-current': pane === k ? 'page' : null, style: k === 'careful' ? 'color:var(--bad)' : null, text: t }))),
    body));
  const A = `/repos/${r.full_name}`;
  const reload = () => { forget(r); go(location.pathname, true); };

  if (pane === 'general') {
    const name = el('input', { id: 's-name', value: r.name });
    const desc = el('input', { id: 's-desc', value: r.description });
    const site = el('input', { id: 's-site', value: r.website, placeholder: 'https://' });
    const branch = el('select', { id: 's-branch' });
    api(`${A}/branches${q({ limit: 100 })}`).then((bs) => branch.replaceChildren(...bs.map((b) => el('option', { text: b.name, selected: b.name === r.default_branch }))));
    const [issues, issuesRow] = check('Issues', r.has_issues);
    const [pulls, pullsRow] = check('Pull requests', r.has_pull_requests);
    const [actions, actionsRow] = check('Actions', r.has_actions);
    const [wiki, wikiRow] = check('Wiki', r.has_wiki, 'Kept by the forge; these pages do not show it yet.');
    const [drop, dropRow] = check('Delete the branch after a merge', r.default_delete_branch_after_merge);
    const styles = [['allow_merge_commits', 'Merge commits'], ['allow_squash_merge', 'Squash'], ['allow_rebase', 'Rebase'], ['allow_fast_forward_only_merge', 'Fast-forward only']].map(([k, t]) => [k, ...check(t, r[k])]);
    const form = el('form', {}, el('h2', { class: 'ph', text: 'General' }),
      field('Name', name, 'Renaming changes its address; the old one sends people on.'), field('What it is', desc), field('Website', site), field('Default branch', branch),
      el('div', { class: 'field' }, el('span', { class: 'lbl', text: 'Features' }), issuesRow, pullsRow, actionsRow, wikiRow),
      el('div', { class: 'field' }, el('span', { class: 'lbl', text: 'Merging' }), ...styles.map((s) => s[2]), dropRow),
      el('button', { class: 'btn go', type: 'submit', text: 'Save' }));
    form.onsubmit = async (e) => {
      e.preventDefault();
      try {
        const x = await api(A, { method: 'PATCH', body: { name: name.value, description: desc.value, website: site.value, default_branch: branch.value, has_issues: issues.checked, has_pull_requests: pulls.checked, has_actions: actions.checked, has_wiki: wiki.checked, default_delete_branch_after_merge: drop.checked, ...Object.fromEntries(styles.map(([k, cb]) => [k, cb.checked])) } });
        toast('Saved');
        forget(r);
        go(`/${x.full_name}/settings/general`, true);
      } catch (err) { toast(err.message); }
    };
    body.append(form);
  }

  if (pane === 'people') {
    const people = await api(`${A}/collaborators${q({ limit: 100 })}`);
    const perm = async (u) => (await api(`${A}/collaborators/${u.login}/permission`).catch(() => ({ permission: 'read' }))).permission;
    const rows = await Promise.all(people.map(async (u) => {
      const p = await perm(u);
      const sel = el('select', { class: 'btn plain', 'aria-label': `${u.login}'s access` }, ...['read', 'write', 'admin'].map((x) => el('option', { value: x, text: { read: 'Read', write: 'Write', admin: 'Admin' }[x], selected: x === p })));
      sel.onchange = async () => { await api(`${A}/collaborators/${u.login}`, { method: 'PUT', body: { permission: sel.value } }); toast('Saved'); };
      const row = el('div', {}, avatar(u), el('b', { text: u.login }), el('span', { class: 'spacer' }), sel, delBtn(`${u.login}'s access`, async () => { await api(`${A}/collaborators/${u.login}`, { method: 'DELETE' }); row.remove(); }));
      return row;
    }));
    const add = pop(el('button', { class: 'btn' }, ic('plus'), 'Add someone'), async (close) => {
      const input = el('input', { placeholder: 'Their name on Commonty', 'aria-label': 'Name' });
      const hits = el('div');
      input.oninput = async () => {
        const s = input.value.trim();
        if (s.length < 2) { hits.replaceChildren(); return; }
        const { data = [] } = await api(`/users/search${q({ q: s, limit: 8 })}`);
        hits.replaceChildren(...data.map((u) => el('a', { href: '#', onclick: async (e) => { e.preventDefault(); await api(`${A}/collaborators/${u.login}`, { method: 'PUT', body: { permission: 'write' } }); close(); reload(); } }, avatar(u), u.login)));
      };
      return [input, hits];
    });
    body.append(el('h2', { class: 'ph', text: 'People' }), el('p', { class: 'muted small', text: `${r.owner.login} owns it. These can work on it too.` }),
      people.length ? el('div', { class: 'box list gap', style: 'max-width:680px' }, ...rows) : el('p', { class: 'muted', text: 'Nobody else yet.' }), add);
  }

  if (pane === 'rules') {
    const [branches, tags] = await Promise.all([api(`${A}/branch_protections`), api(`${A}/tag_protections`).catch(() => [])]);
    const editor = (b = {}) => {
      const name = el('input', { id: 'bp-name', value: b.rule_name || '', placeholder: 'main, or release/*' });
      const [pr, prRow] = check('Changes come through a pull request', b.enable_push === false || (b.enable_push && b.enable_push_whitelist), 'No direct pushes, except by those listed below.');
      const pushers = el('input', { id: 'bp-push', value: (b.push_whitelist_usernames || []).join(', '), placeholder: 'names who may still push directly' });
      const approvals = el('input', { id: 'bp-appr', type: 'number', min: 0, max: 10, value: b.required_approvals ?? 0, style: 'width:80px' });
      const [checks, checksRow] = check('These checks pass first', b.enable_status_check);
      const contexts = el('textarea', { id: 'bp-ctx', class: 'write', style: 'min-height:90px;font-family:var(--mono);font-size:12.5px', text: (b.status_check_contexts || []).join('\n'), placeholder: 'one check per line, or a pattern' });
      const [outdated, outdatedRow] = check('Up to date with the target before merging', b.block_on_outdated_branch);
      const [signed, signedRow] = check('Signed commits only', b.require_signed_commits, 'With a key from your settings.');
      const [stale, staleRow] = check('A new push drops earlier approvals', b.dismiss_stale_approvals);
      const [admins, adminsRow] = check('Owners keep these rules too', b.apply_to_admins, 'Otherwise the repository\'s admins may skip them.');
      const form = el('form', { class: 'box gap' }, el('header', {}, el('b', { text: b.rule_name ? `Rule: ${b.rule_name}` : 'A new rule for branches' })),
        el('div', { style: 'padding:14px 16px' },
          field('Branches it applies to', name, 'A name or a pattern; * matches anything.'),
          prRow, field('May still push directly', pushers),
          el('div', { class: 'field' }, el('label', { for: 'bp-appr', text: 'Approvals needed' }), approvals),
          checksRow, field('Which checks', contexts, 'As they appear on a pull request, e.g. entry_point / lint (pull_request).'),
          outdatedRow, signedRow, staleRow, adminsRow,
          el('div', { class: 'hrow' }, el('button', { class: 'btn go', type: 'submit', text: b.rule_name ? 'Save rule' : 'Add rule' }), b.rule_name ? delBtn(`the rule for ${b.rule_name}`, async () => { await api(`${A}/branch_protections/${encodeURIComponent(b.rule_name)}`, { method: 'DELETE' }); reload(); }) : null)));
      form.onsubmit = async (e) => {
        e.preventDefault();
        const users = pushers.value.split(',').map((s) => s.trim()).filter(Boolean);
        const bodyJ = {
          rule_name: name.value.trim(), enable_push: !pr.checked || users.length > 0, enable_push_whitelist: pr.checked && users.length > 0, push_whitelist_usernames: users,
          required_approvals: Number(approvals.value), enable_status_check: checks.checked, status_check_contexts: contexts.value.split('\n').map((s) => s.trim()).filter(Boolean),
          block_on_outdated_branch: outdated.checked, require_signed_commits: signed.checked, dismiss_stale_approvals: stale.checked, apply_to_admins: admins.checked,
        };
        try {
          if (b.rule_name) await api(`${A}/branch_protections/${encodeURIComponent(b.rule_name)}`, { method: 'PATCH', body: bodyJ });
          else await api(`${A}/branch_protections`, { method: 'POST', body: { ...bodyJ, branch_name: bodyJ.rule_name } });
          toast('Saved');
          reload();
        } catch (err) { toast(err.message); }
      };
      return form;
    };
    const tagName = el('input', { placeholder: 'v*', 'aria-label': 'Tag pattern' });
    const tagWho = el('input', { placeholder: 'who may make them: names, comma separated', 'aria-label': 'Who may make them' });
    const tagForm = el('form', { class: 'hrow', style: 'padding:12px 14px' }, tagName, tagWho, el('button', { class: 'btn', type: 'submit', text: 'Protect these tags' }));
    tagForm.onsubmit = async (e) => { e.preventDefault(); await api(`${A}/tag_protections`, { method: 'POST', body: { name_pattern: tagName.value, whitelist_usernames: tagWho.value.split(',').map((s) => s.trim()).filter(Boolean) } }); reload(); };
    const fleetRepo = await fleetRepoName();
    body.append(el('h2', { class: 'ph', text: 'Rulesets' }),
      el('p', { class: 'muted small', text: 'Rules for branches and tags: what a change needs before it lands, and who may skip that.' }),
      fleetRepo === r.full_name ? el('p', { class: 'box', style: 'padding:10px 14px;border-left:4px solid var(--busy)', text: 'The rule for main is also written in the fleet\'s Nix (nix/modules/forge/forgejo.nix), which sets it again on every release: change it there to keep a change.' }) : null,
      ...branches.map(editor), editor(),
      el('h2', { class: 'ph', style: 'margin-top:24px;font-size:16px', text: 'Tags' }),
      el('div', { class: 'box' }, el('div', { class: 'list' }, ...tags.map((t) => el('div', {}, ic('tag'), el('b', { class: 'mono', text: t.name_pattern }), el('span', { class: 'small muted', text: t.whitelist_usernames?.length ? `only ${t.whitelist_usernames.join(', ')} may make or move them` : 'nobody may move or delete them' }), el('span', { class: 'spacer' }), delBtn(`the rule for ${t.name_pattern}`, async () => { await api(`${A}/tag_protections/${t.id}`, { method: 'DELETE' }); reload(); })))), tagForm));
  }

  if (pane === 'actions') {
    const [secrets, vars] = await Promise.all([api(`${A}/actions/secrets`).catch(() => []), api(`${A}/actions/variables`).catch(() => [])]);
    const sName = el('input', { placeholder: 'NAME', 'aria-label': 'Secret name', style: 'font-family:var(--mono)' });
    const sVal = el('input', { type: 'password', placeholder: 'its value', 'aria-label': 'Secret value', autocomplete: 'off' });
    const sForm = el('form', { class: 'hrow', style: 'padding:12px 14px' }, sName, sVal, el('button', { class: 'btn go', type: 'submit', text: 'Save secret' }));
    sForm.onsubmit = async (e) => { e.preventDefault(); await api(`${A}/actions/secrets/${encodeURIComponent(sName.value.trim())}`, { method: 'PUT', body: { data: sVal.value } }); reload(); };
    const vName = el('input', { placeholder: 'NAME', 'aria-label': 'Variable name', style: 'font-family:var(--mono)' });
    const vVal = el('input', { placeholder: 'its value', 'aria-label': 'Variable value' });
    const vForm = el('form', { class: 'hrow', style: 'padding:12px 14px' }, vName, vVal, el('button', { class: 'btn', type: 'submit', text: 'Save variable' }));
    vForm.onsubmit = async (e) => {
      e.preventDefault();
      const n = vName.value.trim();
      const exists = vars.some((v) => v.name === n);
      await api(`${A}/actions/variables/${encodeURIComponent(n)}`, { method: exists ? 'PUT' : 'POST', body: { value: vVal.value } });
      reload();
    };
    body.append(el('h2', { class: 'ph', text: 'Actions: secrets & variables' }),
      el('p', { class: 'muted small', text: 'Secrets are sealed: runs get them, nobody can read one back, you included. Saving a name that exists replaces it.' }),
      el('div', { class: 'box gap' }, el('header', {}, el('b', { text: 'Secrets' })), el('div', { class: 'list' }, ...secrets.map((s) => el('div', {}, ic('lock'), el('span', { class: 'mono', text: s.name }), el('span', { class: 'spacer' }), el('span', { class: 'small muted' }, 'set ', when(s.created_at)), delBtn(`the secret ${s.name}`, async () => { await api(`${A}/actions/secrets/${encodeURIComponent(s.name)}`, { method: 'DELETE' }); reload(); })))), sForm),
      el('div', { class: 'box' }, el('header', {}, el('b', { text: 'Variables' }), el('span', { class: 'small muted', text: 'plain values, visible in logs' })), el('div', { class: 'list' }, ...vars.map((v) => el('div', {}, el('span', { class: 'mono', text: v.name }), el('span', { class: 'mono small muted', style: 'overflow:hidden;text-overflow:ellipsis;white-space:nowrap;flex:1', text: v.data }), delBtn(`the variable ${v.name}`, async () => { await api(`${A}/actions/variables/${encodeURIComponent(v.name)}`, { method: 'DELETE' }); reload(); })))), vForm));
  }

  if (pane === 'runners') {
    const runners = await api(`${A}/actions/runners`).catch(() => null);
    const list = Array.isArray(runners) ? runners : runners?.runners || runners?.data || [];
    body.append(el('h2', { class: 'ph', text: 'Runners' }),
      list.length ? el('div', { class: 'box list' }, ...list.map((x) => el('div', {}, el('span', { class: `dot ${x.status === 'online' || x.online ? 'ok' : ''}` }), el('b', { text: x.name }), ...(x.labels || []).map((l) => el('span', { class: 'chip', text: typeof l === 'string' ? l : l.name })), el('span', { class: 'spacer' }), el('span', { class: 'small muted', text: x.version || '' }))))
        : el('p', { class: 'muted', text: 'The runners that take this repository\'s jobs belong to the forge as a whole.' }),
      el('p', { class: 'gnote', style: 'margin-top:10px', text: 'Runners come from the fleet\'s Nix, not from here: a box runs jobs because its roles say so.' }));
  }

  if (pane === 'hooks') {
    const hooks = await api(`${A}/hooks`);
    const url = el('input', { placeholder: 'https://', 'aria-label': 'Address', type: 'url', required: true, style: 'flex:1;min-width:240px' });
    const secret = el('input', { placeholder: 'a secret it can check (optional)', 'aria-label': 'Secret', autocomplete: 'off' });
    const events = el('select', { class: 'btn plain', 'aria-label': 'When' }, el('option', { value: 'push', text: 'Pushes' }), el('option', { value: 'all', text: 'Everything' }), el('option', { value: 'pr', text: 'Pull requests' }));
    const form = el('form', { class: 'hrow', style: 'padding:12px 14px' }, url, secret, events, el('button', { class: 'btn go', type: 'submit', text: 'Add webhook' }));
    form.onsubmit = async (e) => {
      e.preventDefault();
      const ev = events.value === 'all' ? { events: ['push', 'pull_request', 'issues', 'issue_comment', 'release', 'create', 'delete'] } : events.value === 'pr' ? { events: ['pull_request'] } : { events: ['push'] };
      try { await api(`${A}/hooks`, { method: 'POST', body: { type: 'forgejo', active: true, config: { url: url.value, content_type: 'json', secret: secret.value }, ...ev } }); reload(); }
      catch (err) { toast(err.message); }
    };
    body.append(el('h2', { class: 'ph', text: 'Webhooks' }),
      el('p', { class: 'muted small', text: 'A POST to another service when something happens here, signed with the secret so it can tell the post is real.' }),
      el('div', { class: 'box' }, el('div', { class: 'list' }, ...hooks.map((h) => {
        const test = el('button', { class: 'btn plain', text: 'Send a test', onclick: async () => { await api(`${A}/hooks/${h.id}/tests`, { method: 'POST' }); toast('Sent'); } });
        return el('div', {}, el('span', { class: `dot ${h.active ? 'ok' : ''}` }), el('span', { class: 'mono', style: 'flex:1;min-width:0;overflow:hidden;text-overflow:ellipsis', text: h.config?.url }), el('span', { class: 'small muted', text: (h.events || []).join(', ') }), test, delBtn('this webhook', async () => { await api(`${A}/hooks/${h.id}`, { method: 'DELETE' }); reload(); }));
      })), form),
      el('p', { class: 'gnote', style: 'margin-top:10px', text: 'Addresses inside the house or the tailnet are refused, so a webhook cannot be pointed at our own boxes.' }));
  }

  if (pane === 'keys') {
    const keys = await api(`${A}/keys`);
    const title = el('input', { placeholder: 'github mirror push', 'aria-label': 'Name', required: true });
    const key = el('textarea', { class: 'write', style: 'min-height:70px', placeholder: 'ssh-ed25519 AAAA…', 'aria-label': 'Public key', required: true });
    const [write, writeRow] = check('May push, not only read', false);
    const form = el('form', { style: 'padding:12px 14px;display:grid;gap:8px;max-width:640px' }, title, key, writeRow, el('div', {}, el('button', { class: 'btn go', type: 'submit', text: 'Add deploy key' })));
    form.onsubmit = async (e) => { e.preventDefault(); try { await api(`${A}/keys`, { method: 'POST', body: { title: title.value, key: key.value.trim(), read_only: !write.checked } }); reload(); } catch (err) { toast(err.message); } };
    body.append(el('h2', { class: 'ph', text: 'Deploy keys' }),
      el('p', { class: 'muted small', text: 'An SSH key that reaches this one repository and nothing else: for a machine, not a person.' }),
      el('div', { class: 'box' }, el('div', { class: 'list' }, ...keys.map((k) => el('div', {}, ic('key'), el('div', { style: 'flex:1;min-width:0' }, el('b', { text: k.title }), el('div', { class: 'sub mono', text: `${k.fingerprint} · ${k.read_only ? 'read' : 'write'}` })), el('span', { class: 'small muted' }, 'added ', when(k.created_at)), delBtn(`the key ${k.title}`, async () => { await api(`${A}/keys/${k.id}`, { method: 'DELETE' }); reload(); })))), form));
  }

  if (pane === 'labels') {
    const [labels, miles] = await Promise.all([api(`${A}/labels${q({ limit: 100 })}`), api(`${A}/milestones${q({ state: 'all', limit: 50 })}`)]);
    const lName = el('input', { placeholder: 'name', 'aria-label': 'Label name', required: true });
    const lColor = el('input', { type: 'color', value: '#1d5c42', 'aria-label': 'Colour' });
    const lDesc = el('input', { placeholder: 'what it marks', 'aria-label': 'Description' });
    const lForm = el('form', { class: 'hrow', style: 'padding:12px 14px' }, lName, lColor, lDesc, el('button', { class: 'btn', type: 'submit', text: 'New label' }));
    lForm.onsubmit = async (e) => { e.preventDefault(); await api(`${A}/labels`, { method: 'POST', body: { name: lName.value, color: lColor.value, description: lDesc.value } }); reload(); };
    const mTitle = el('input', { placeholder: 'The app on every device', 'aria-label': 'Milestone', required: true });
    const mDue = el('input', { type: 'date', 'aria-label': 'Due' });
    const mForm = el('form', { class: 'hrow', style: 'padding:12px 14px' }, mTitle, mDue, el('button', { class: 'btn', type: 'submit', text: 'New milestone' }));
    mForm.onsubmit = async (e) => { e.preventDefault(); await api(`${A}/milestones`, { method: 'POST', body: { title: mTitle.value, due_on: mDue.value ? new Date(mDue.value).toISOString() : undefined } }); reload(); };
    body.append(el('h2', { class: 'ph', text: 'Labels' }),
      el('div', { class: 'box gap' }, el('div', { class: 'list' }, ...labels.map((l) => el('div', {}, el('span', { class: 'chip', style: `background:#${l.color.replace('#', '')}22;color:var(--ink);box-shadow:inset 0 0 0 1px #${l.color.replace('#', '')}88`, text: l.name }), el('span', { class: 'small muted', style: 'flex:1', text: l.description || '' }), delBtn(`the label ${l.name}`, async () => { await api(`${A}/labels/${l.id}`, { method: 'DELETE' }); reload(); })))), lForm),
      el('h2', { class: 'ph', text: 'Milestones' }),
      el('div', { class: 'box' }, el('div', { class: 'list' }, ...miles.map((ms) => {
        const done = ms.closed_issues, all = ms.open_issues + ms.closed_issues;
        return el('div', {}, el('div', { style: 'flex:1;min-width:0' }, el('b', { text: ms.title }), el('div', { class: 'sub', text: `${ms.due_on ? `due ${new Date(ms.due_on).toLocaleDateString()} · ` : ''}${done} of ${all} done · ${ms.state}` })),
          el('div', { style: 'width:140px;height:8px;background:var(--line-2);border-radius:4px;overflow:hidden' }, el('div', { style: `width:${all ? (done / all) * 100 : 0}%;height:100%;background:var(--accent)` })),
          delBtn(`the milestone ${ms.title}`, async () => { await api(`${A}/milestones/${ms.id}`, { method: 'DELETE' }); reload(); }));
      })), mForm));
  }

  if (pane === 'mirror') {
    const mirrors = await api(`${A}/push_mirrors`).catch(() => []);
    const url = el('input', { type: 'url', placeholder: 'https://github.com/you/repo.git', 'aria-label': 'Where to push', required: true, style: 'flex:1;min-width:240px' });
    const user = el('input', { placeholder: 'user', 'aria-label': 'User' });
    const pass = el('input', { type: 'password', placeholder: 'token', 'aria-label': 'Token', autocomplete: 'off' });
    const form = el('form', { class: 'hrow', style: 'padding:12px 14px' }, url, user, pass, el('button', { class: 'btn', type: 'submit', text: 'Add a mirror' }));
    form.onsubmit = async (e) => { e.preventDefault(); try { await api(`${A}/push_mirrors`, { method: 'POST', body: { remote_address: url.value, remote_username: user.value, remote_password: pass.value, interval: '8h', sync_on_commit: true } }); reload(); } catch (err) { toast(err.message); } };
    body.append(el('h2', { class: 'ph', text: 'Mirror' }),
      el('p', { class: 'muted small', text: 'Another place this repository is pushed to after changes. The fleet repository\'s GitHub mirror is pushed by the release queue, not from here.' }),
      el('div', { class: 'box' }, el('div', { class: 'list' }, ...mirrors.map((x) => el('div', {}, el('span', { class: `dot ${x.last_error ? 'bad' : 'ok'}` }), el('div', { style: 'flex:1;min-width:0' }, el('b', { class: 'mono', text: x.remote_address }), el('div', { class: 'sub', text: x.last_error || `last pushed ${x.last_update ? ago(x.last_update) : 'never'}` })),
        el('button', { class: 'btn plain', text: 'Push now', onclick: async () => { await api(`${A}/push_mirrors-sync`, { method: 'POST' }); toast('Pushing'); } }),
        delBtn('this mirror', async () => { await api(`${A}/push_mirrors/${encodeURIComponent(x.remote_name)}`, { method: 'DELETE' }); reload(); })))), form));
  }

  if (pane === 'careful') {
    const row = (title, words, button) => el('div', {}, el('div', { style: 'flex:1;min-width:200px' }, el('b', { text: title }), el('br'), el('span', { class: 'small muted', text: words })), button);
    const vis = el('button', { class: 'btn bad', text: r.private ? 'Make it public' : 'Make it private' });
    vis.onclick = async () => { if (confirm(r.private ? 'Anyone will be able to read it. Go on?' : 'Only the people it is shared with will see it. Go on?')) { await api(A, { method: 'PATCH', body: { private: !r.private } }); reload(); } };
    const transfer = el('button', { class: 'btn bad', text: 'Transfer' });
    transfer.onclick = async () => {
      const to = prompt('Give it to whom? Their name on Commonty.');
      if (to) { try { await api(`${A}/transfer`, { method: 'POST', body: { new_owner: to } }); toast(`Offered to ${to}`); } catch (err) { toast(err.message); } }
    };
    const archive = el('button', { class: 'btn bad', text: r.archived ? 'Unarchive' : 'Archive' });
    archive.onclick = async () => { if (confirm(r.archived ? 'Make it writable again?' : 'Make it read-only? Nothing is deleted.')) { await api(A, { method: 'PATCH', body: { archived: !r.archived } }); reload(); } };
    const del = el('button', { class: 'btn bad', text: 'Delete repository' });
    del.onclick = async () => {
      const typed = prompt(`This deletes ${r.full_name}, its history, pull requests and issues, and cannot be undone. Type its name to go on.`);
      if (typed !== r.name) { if (typed !== null) toast('The name did not match; nothing was deleted'); return; }
      await api(A, { method: 'DELETE' });
      forget(r);
      go('/');
    };
    body.append(el('h2', { class: 'ph', style: 'color:var(--bad)', text: 'Careful' }),
      el('div', { class: 'danger', style: 'max-width:760px' },
        row('Who can see it', r.private ? 'Now: only the people it is shared with.' : 'Now: anyone.', vis),
        row('Give it to someone else', 'They own it; you keep the access they give you.', transfer),
        row(r.archived ? 'Archived' : 'Archive', r.archived ? 'Read-only now.' : 'Read-only; nothing is deleted.', archive),
        row('Delete', 'The repository and everything in it, for good. There is no undo.', del)));
  }
});
