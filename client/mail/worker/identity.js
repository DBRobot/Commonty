// Who a member is, as the control plane agreed it (docs/dev/identity-state.md):
// read from the control relay's Durable Object, never judged here. The mail
// Worker and the DNS Worker (servers/system/dns, which carries this file too)
// read members from it once IDENTITY_FROM says so:
//
//   replay  (default) as before: the Worker judges the entry itself
//   shadow  judges itself and reads the agreed state, says where the two
//           differ ("identity-shadow" in the Worker's log), goes by its own
//   agreed  goes by the agreed state alone
//
// CONTROL is the relay's Durable Object namespace (bound by `dd mail deploy`
// and `dd dns deploy`); IDENTITY_URL stands in for it in the tests.

export const mode = (env) => (['shadow', 'agreed'].includes(env.IDENTITY_FROM) ? env.IDENTITY_FROM : 'replay');

// a member's agreed record, or null when the agreed state has no such name
export async function agreedMember(env, name) {
  const q = `?name=${encodeURIComponent(name)}`;
  let r;
  if (env.CONTROL) r = await env.CONTROL.get(env.CONTROL.idFromName('relay')).fetch(`https://control/identity${q}`);
  else if (env.IDENTITY_URL) r = await fetch(`${env.IDENTITY_URL}${q}`);
  else throw new Error('no agreed identity to read: CONTROL is not bound');
  if (r.status === 404) return null;
  if (!r.ok) throw new Error(`the agreed identity did not answer: ${r.status}`);
  const v = await r.json();
  // one member, or the whole state with them in it
  const m = v && v.members ? v.members[name] : v;
  return m && typeof m === 'object' && !Array.isArray(m) ? m : null;
}

export const isMember = (m) => Boolean(m) && m.status === 'member';

const utf8 = (b64) => new TextDecoder().decode(Uint8Array.from(atob(b64), (c) => c.charCodeAt(0)));

// The entry the member's agreed item carries, as identity::SignedEntry's JSON
// ({entry, signature}): what the Worker checks a passkey's answer, or the
// member's word for a controller's name, against. The judge already checked
// its signature; it is read here only for the keys in it.
export function entryOf(m) {
  const it = typeof m.item === 'string' ? JSON.parse(m.item) : m.item;
  // v1: {"entry": SignedEntry}
  if (it && it.entry && it.entry.entry) return JSON.stringify(it.entry);
  // v2: the envelope, its body the entry's fields as signed
  if (it && it.v === 2 && it.kind === 'entry' && typeof it.body === 'string') {
    return JSON.stringify({ entry: JSON.parse(utf8(it.body)), signature: it.sig || '' });
  }
  throw new Error('the agreed item carries no entry');
}

// Where its own judgment and the agreed state differ, the Worker says so
// once a request; what it goes by stays its own until IDENTITY_FROM=agreed.
// One line: who, which check, both answers, and `why` - what each side
// said beside its answer (the agreed record's status and access, the
// reason a check refused), logged but never compared
export function differ(what, name, own, agreed, why) {
  if (JSON.stringify(own) === JSON.stringify(agreed)) return;
  const line = { 'identity-shadow': what, name, own, agreed };
  if (why !== undefined) line.why = why;
  console.warn(JSON.stringify(line));
}

// the agreed record, as the shadow log says it: public words only
export const said = (m) => (m ? { status: m.status ?? null, access: m.access ?? null } : 'no such name');
