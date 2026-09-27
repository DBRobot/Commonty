// Friends: the page that lists yours and makes links (/_dd/friends), and
// the page a link opens (/_dd/friend/<secret>). The secret is only ever in
// the address; the box keeps a hash of it (friends.rs).

const $ = (id) => document.getElementById(id);

function el(tag, props = {}, ...kids) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(props)) {
    if (k === 'class') e.className = v;
    else if (k === 'text') e.textContent = v;
    else e.setAttribute(k, v);
  }
  e.append(...kids);
  return e;
}

// a change: JSON, so another site's form cannot make one (friends.rs)
async function send(method, url) {
  const r = await fetch(url, { method, headers: { 'content-type': 'application/json' }, body: method === 'POST' ? '{}' : undefined });
  if (!r.ok) throw new Error((await r.text()) || `the box said ${r.status}`);
  return r.status === 204 ? null : r.json();
}

const when = (secs) => new Date(secs * 1000);
function since(secs) {
  const d = when(secs);
  const days = Math.floor((Date.now() - d) / 86400000);
  if (days < 1) return 'friends since today';
  if (days < 2) return 'friends since yesterday';
  return `friends since ${d.toLocaleDateString(undefined, { day: 'numeric', month: 'short' })}`;
}
const until = (secs) => when(secs).toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' });

// the same colour for a name every visit
function hue(name) {
  let h = 0;
  for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0;
  return [212, 330, 160, 32, 262, 196, 12, 96][h % 8];
}
function avatar(name, initial) {
  const a = el('span', { class: 'av', 'aria-hidden': 'true', text: (initial || name[0] || '?').toUpperCase() });
  a.style.setProperty('--h', hue(name));
  return a;
}
const kindTag = (k) => el('span', { class: `kind ${k}`, text: k === 'member' ? 'Member' : k === 'guest' ? 'Guest' : 'Not in' });

// ---- the list

async function list() {
  const r = await fetch('/_dd/friends/list');
  if (!r.ok) throw new Error(`the box said ${r.status}`);
  const d = await r.json();
  $('add').hidden = !d.canInvite;
  $('people').replaceChildren(...d.friends.map((f) => {
    const remove = el('button', { type: 'button', class: 'linkish', text: 'Remove' });
    remove.onclick = async () => {
      if (!confirm(`Stop being friends with ${f.name}? They lose anything you invited them to.`)) return;
      await send('DELETE', `/_dd/friends/remove/${encodeURIComponent(f.name)}`);
      await list();
    };
    return el('li', {},
      avatar(f.name, f.initial),
      el('span', { class: 'who' }, el('b', { text: f.name }), el('span', { text: since(f.since) })),
      kindTag(f.kind),
      remove);
  }));
  $('none').hidden = d.friends.length > 0;
  if (!d.canInvite && d.guestOf) {
    $('none').textContent = 'No friends here any more.';
  }
  $('waiting').hidden = !d.links.length;
  $('links').replaceChildren(...d.links.map((l) => {
    const cancel = el('button', { type: 'button', class: 'linkish', text: 'Cancel' });
    cancel.onclick = async () => {
      await send('DELETE', `/_dd/friends/link/${encodeURIComponent(l.id)}`);
      $('made').hidden = true;
      await list();
    };
    return el('li', { class: 'waiting' },
      el('span', { class: 'who' },
        el('b', { text: `Link made ${when(l.made).toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' })}` }),
        el('span', { text: `not opened yet · works until ${until(l.expires)}` })),
      cancel);
  }));
}

function friendsPage() {
  $('make').onclick = async () => {
    $('add-error').hidden = true;
    $('make').disabled = true;
    try {
      const l = await send('POST', '/_dd/friends/link');
      $('link').value = l.url;
      $('made-note').textContent = `Works once · until ${until(l.expires)} · send it however you like`;
      $('made').hidden = false;
      $('copy').textContent = 'Copy';
      $('link').select();
      await list();
    } catch (e) {
      $('add-error').textContent = e.message;
      $('add-error').hidden = false;
    } finally {
      $('make').disabled = false;
    }
  };
  $('copy').onclick = async () => {
    try {
      await navigator.clipboard.writeText($('link').value);
      $('copy').textContent = 'Copied';
    } catch {
      $('link').select();
    }
  };
  list().catch((e) => {
    $('people').replaceChildren(el('li', { class: 'error', text: e.message }));
  });
}

// ---- a link, opened

async function linkPage() {
  const card = $('card');
  const here = location.pathname;
  const secret = here.split('/').pop();
  const r = await fetch(`/_dd/friend/${encodeURIComponent(secret)}/about`);
  if (!r.ok) {
    card.replaceChildren(
      el('h1', { text: 'This link no longer works' }),
      el('p', { class: 'lead', text: 'It has been used, cancelled, or has run out: a link works once, for a day. Ask whoever sent it for a new one.' }));
    return;
  }
  const d = await r.json();
  const head = el('div', { class: 'invite-head' }, avatar(d.by, d.initial),
    el('div', {}, el('h1', { text: `${d.by} wants to be friends` }),
      el('p', { class: 'small', text: `Link works until ${until(d.expires)}` })));
  if (d.demo) {
    card.replaceChildren(head, el('p', { class: 'lead', text: "You're looking around as the demo, which can't make friends. Sign out, then open this link again to sign in or make an account." }),
      el('a', { class: 'button', href: '/_dd/logout', text: 'Sign out' }));
    return;
  }
  if (!d.me) {
    const rd = encodeURIComponent(here);
    card.replaceChildren(head,
      el('p', { class: 'lead', text: `Friends on Commonty play on each other's game servers. If you have no account, making one takes a name and a passkey; you come in as ${d.by}'s guest.` }),
      el('div', { class: 'row' },
        el('a', { class: 'button', href: `/_dd/join?rd=${rd}`, text: 'Create an account' }),
        el('a', { class: 'button quiet', href: `/_dd/login?rd=${rd}`, text: 'I have an account' })));
    return;
  }
  if (d.already) {
    card.replaceChildren(head, el('p', { class: 'lead', text: `You and ${d.by} are friends already.` }),
      el('a', { class: 'button', href: '/_dd/friends', text: 'Your friends' }));
    return;
  }
  const go = el('button', { type: 'button', text: `Add ${d.by} as a friend` });
  const err = el('p', { class: 'error' });
  err.hidden = true;
  go.onclick = async () => {
    go.disabled = true;
    try {
      const a = await send('POST', `/_dd/friend/${encodeURIComponent(secret)}`);
      card.replaceChildren(
        el('h1', { text: `You and ${a.by} are friends` }),
        el('p', { class: 'lead', text: a.role === 'guest'
          ? `You're ${a.by}'s guest: when they invite you to a game server, it shows up in Games.`
          : 'They are in your friends now, and you in theirs.' }),
        el('a', { class: 'button', href: '/_dd/home', text: 'Go home' }));
    } catch (e) {
      err.textContent = e.message;
      err.hidden = false;
      go.disabled = false;
    }
  };
  card.replaceChildren(head, el('p', { class: 'lead', text: `Signed in as ${d.me}.` }), go, err);
}

if (location.pathname.startsWith('/_dd/friend/')) {
  linkPage().catch((e) => {
    $('card').replaceChildren(el('p', { class: 'error', text: e.message }));
  });
} else {
  friendsPage();
}
