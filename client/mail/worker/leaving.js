// Someone left, or was removed 30 days ago (docs/dev/leaving.md): their <name>@<domain>
// rule goes, the address it forwarded to if no other rule uses it, their
// own aliases (aliases.js), and what this Worker pinned for them. Anyone may ask; what decides is the
// member's own signed statement, checked here against the entry pinned for
// them, or the release's removal (forgetDue, client/mail/src/lib.rs).
const NAME = /^[a-z0-9][a-z0-9._-]{0,63}$/;

export async function forgetApi(req, env, body, json, h) {
  if (req.method !== 'POST') return json({ error: 'POST' }, 405);
  const name = body?.name || '';
  if (!NAME.test(name)) return json({ error: 'no such member' }, 400);
  const pinned = await env.PINNED.get(`entry:${name}`);
  // never pinned: they never set an address here
  if (!pinned) return json({ gone: true });
  // read as the entry is, with the Worker's read key (worker.js entry)
  const headers = env.DIRECTORY_KEY ? { 'x-dd-key-proof': h.readProof(env.DIRECTORY_KEY, name, Math.floor(Date.now() / 1000)) } : {};
  const r = await fetch(`https://home.${env.DOMAIN}/_dd/leaving/${encodeURIComponent(name)}`, { headers });
  const said = r.ok ? await r.text() : undefined;
  // a recovery since pinning: the new root signs the statement
  const e = await fetch(`${env.DIRECTORY || `https://home.${env.DOMAIN}/_dd/directory/`}${encodeURIComponent(name)}`, { headers });
  const fresh = e.ok ? await e.text() : undefined;
  try {
    h.forgetDue(name, pinned, fresh, said, h.MEMBERS, Math.floor(Date.now() / 1000));
  } catch (err) {
    return json({ error: String(err.message || err) }, 403);
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
  // their own aliases: deleted, and nothing left saying they were theirs
  await h.forgetAliases(env, name);
  await env.PINNED.delete(`ended:${name}`);
  await env.PINNED.delete(`entry:${name}`);
  return json({ gone: true });
}
