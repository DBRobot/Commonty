// Someone left, or was removed 30 days ago (docs/dev/leaving.md): their <name>@<domain>
// rule goes, the address it forwarded to if no other rule uses it, their
// own aliases (aliases.js), and what this Worker pinned for them. Anyone may ask; what decides is the
// member's own signed statement, checked here against the entry pinned for
// them, or the release's removal (forgetDue, client/mail/src/lib.rs).
const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;
// after the release's removal of someone, how long before their address
// goes: forgetDue's window (client/mail/src/lib.rs LEAVE_WINDOW)
const WINDOW = 90 * 24 * 3600;

// May their address go, by the agreed state (docs/dev/identity-state.md)? At
// once for their own leaving (status "left"); the window after the release's
// removal of them, for "revoked" or "suspended", by any root they have had.
// The agreement has judged both; this only reads it.
export function dueByAgreement(m, members, now) {
  if (!m) return 'not in the agreed state';
  if (m.status === 'left') {
    // the agreed state keeps times in ms (seconds would not reach 1e11 for
    // three thousand years); a statement's own `at` is seconds
    const left = Number(m.left || 0);
    return (left >= 1e11 ? left / 1000 : left) <= now ? null : 'they have not left yet';
  }
  if (m.status === 'revoked' || m.status === 'suspended') {
    const removed = (members && members.removed) || {};
    for (const id of m.ids || [m.id]) {
      const at = removed[id];
      if (typeof at === 'number' && at + WINDOW <= now) return null;
    }
    return 'removed, and the window is not over';
  }
  return 'they have not left';
}

export async function forgetApi(req, env, body, json, h) {
  if (req.method !== 'POST') return json({ error: 'POST' }, 405);
  const name = body?.name || '';
  if (!NAME.test(name)) return json({ error: 'no such member' }, 400);
  const pinned = await env.PINNED.get(`entry:${name}`);
  // never pinned: they never set an address here
  if (!pinned) return json({ gone: true });
  const how = h.mode(env);
  const now = Math.floor(Date.now() / 1000);
  let agreedWhy;
  if (how !== 'replay') {
    let m = null;
    try {
      m = await h.agreedMember(env, name);
      agreedWhy = dueByAgreement(m, h.MEMBERS, now);
    } catch (err) {
      agreedWhy = `unread: ${String(err.message || err)}`;
    }
    if (how === 'agreed' && agreedWhy) return json({ error: agreedWhy }, 403);
  }
  if (how !== 'agreed') {
    let ownWhy = null;
    try {
      await replayDue(env, name, pinned, now, h);
    } catch (err) {
      ownWhy = String(err.message || err);
    }
    if (how === 'shadow') h.differ('forget', name, { due: !ownWhy }, { due: !agreedWhy }, { own: ownWhy, agreed: agreedWhy ?? null });
    if (ownWhy) return json({ error: ownWhy }, 403);
  }
  const z = await h.zone(env);
  const l = await h.lists(env);
  const s = h.stateIn(l, env, name);
  if (s.rule) {
    // the release's aliases of theirs forward to their address: those rules
    // go first, while their own rule still says which address it is
    for (const r of l.rules) {
      const a = h.aliasOf(r);
      if (a === null || (h.ALIASES || {})[a] !== name) continue;
      const g = await h.cf(env, 'DELETE', `/zones/${z.id}/email/routing/rules/${r.id}`);
      if (!g.success) return json({ error: 'Cloudflare did not answer; try again later' }, 503);
    }
    const d = await h.cf(env, 'DELETE', `/zones/${z.id}/email/routing/rules/${s.rule.id}`);
    if (!d.success) return json({ error: 'Cloudflare did not answer; try again later' }, 503);
    const used = l.rules.some((x) => x.id !== s.rule.id && !((h.ALIASES || {})[h.aliasOf(x)] === name) && (x.actions || []).some((a) => (a.value || []).some((v) => (v || '').toLowerCase() === (s.email || '').toLowerCase())));
    if (s.destination && !used) await h.cf(env, 'DELETE', `/accounts/${z.account}/email/routing/addresses/${s.destination.tag || s.destination.id}`);
  }
  // their inbox's rule, its key, and mail no box has taken yet
  for (const r of l.rules) {
    if ((r.name || '') !== `${h.INBOX_RULE}${name}`) continue;
    const g = await h.cf(env, 'DELETE', `/zones/${z.id}/email/routing/rules/${r.id}`);
    if (!g.success) return json({ error: 'Cloudflare did not answer; try again later' }, 503);
  }
  await h.forgetInbox(env, name);
  // their own aliases: deleted, and nothing left saying they were theirs
  await h.forgetAliases(env, name);
  await env.PINNED.delete(`ended:${name}`);
  await env.PINNED.delete(`entry:${name}`);
  return json({ gone: true });
}

// judged here (replay and shadow): their statement, against the entry pinned
// for them or one that follows from it, or the release's removal
async function replayDue(env, name, pinned, now, h) {
  // read as the entry is, with the Worker's read key (worker.js entry)
  const headers = env.DIRECTORY_KEY ? { 'x-dd-key-proof': h.readProof(env.DIRECTORY_KEY, name, now) } : {};
  const r = await fetch(`https://home.${env.DOMAIN}/_dd/leaving/${encodeURIComponent(name)}`, { headers });
  const said = r.ok ? await r.text() : undefined;
  // a recovery since pinning: the new root signs the statement
  const e = await fetch(`${env.DIRECTORY || `https://home.${env.DOMAIN}/_dd/directory/`}${encodeURIComponent(name)}`, { headers });
  const fresh = e.ok ? await e.text() : undefined;
  h.forgetDue(name, pinned, fresh, said, h.MEMBERS, now);
}
