// A page whose code would hold a member's keys, in a browser (use-app.html):
// Photos, Passwords, and the site's Files, Movies & TV, Chat and Mail. Their
// code comes with the member's app (app/src/local.rs), never from here. This
// page hands the browser to the app by its own link, which the system gives
// only to the app installed here: at a press the first time, by itself once
// the app has taken one (the dd_app note). Still answered here without the
// app: a Send opened from inside, and a Bitwarden app's or extension's
// sign-in, which starts and ends on the web vault's address.
const vault = location.hostname.startsWith('vault.');
const photos = location.hostname.startsWith('photos.') || location.pathname.startsWith('/_dd/photos');
const domain = location.hostname.split('.').slice(1).join('.');
const home = `https://home.${domain}`;
// the site's own page this was, by its path (Chat is the model box's front)
const sitePage = location.hostname.startsWith('llm.') ? '/_dd/chat' : location.pathname;
const what = vault ? 'Passwords' : photos ? 'Photos' : ({
  '/_dd/files': 'Files', '/_dd/media': 'Movies & TV', '/_dd/chat': 'Chat', '/_dd/mail': 'Mail',
  '/_dd/calendar': 'Calendar', '/_dd/contacts': 'Contacts',
})[sitePage] || 'Commonty';
const show = (id) => {
  for (const s of document.querySelectorAll('main > section'))
    s.hidden = s.id !== id;
};

document.getElementById('home-link').href = `${home}/_dd/home`;
for (const id of ['download', 'download-2']) document.getElementById(id).href = `${home}/_dd/download`;
document.getElementById('what').textContent = what;

// The page asked for, for the app to open: the vault's own pages are after
// the #, Photos' are paths, the site's its own path.
function asked() {
  if (vault) return location.hash.startsWith('#/') ? `/${location.hash}` : '/';
  if (photos) return location.pathname.startsWith('/_dd/photos') ? '/' : location.pathname + location.search;
  return sitePage + location.hash;
}

// the note that the app takes this browser's links: nothing secret, it
// only spares the press next time
const NOTE = 'dd_app=1';
const noted = () => document.cookie.split(';').some((c) => c.trim() === NOTE);
const note = () => {
  document.cookie = `${NOTE}; Domain=${domain}; Path=/; Max-Age=34560000; Secure; SameSite=Lax`;
};

// A browser says nothing when no app takes the link: the app opening the
// page takes this tab out of the front, and until it does the page says so
// after a few seconds (the browser may be asking first).
function handOff() {
  const left = () => {
    window.removeEventListener('blur', left);
    document.removeEventListener('visibilitychange', left);
    note();
    show('opened');
    // back where the member came from, when there was somewhere
    if (history.length > 1) history.back();
  };
  window.addEventListener('blur', left);
  document.addEventListener('visibilitychange', left);
  const q = new URLSearchParams({ service: vault ? 'vault' : photos ? 'photos' : 'site' });
  const path = asked();
  if (path !== '/') q.set('path', path);
  location.href = `commonty://open?${q}`;
  setTimeout(() => {
    if (document.visibilityState === 'visible' && document.hasFocus()) show('not-open');
  }, 3000);
}

// A browser the app let in and has signed in here: this box keeps where
// the app answers it (servers/gate/src/app_signin.rs), so the page goes
// there itself, through the box, with no link for the system to hand over
const linkedHere = document.cookie.split(';').some((c) => c.trim() === 'dd_app_linked=1');
function straight() {
  const q = new URLSearchParams({ open: vault ? 'vault' : photos ? 'photos' : 'site' });
  const path = asked();
  if (path !== '/') q.set('path', path);
  location.replace(`/_dd/app-signin?${q}`);
}

document.getElementById('open').addEventListener('click', handOff);
document.getElementById('retry').addEventListener('click', handOff);

// A Bitwarden app's sign-in, as the web vault's SSO page does it: the app
// sends its own client, return address, state and code challenge; this asks
// Vaultwarden for the sign-in token and goes to its authorize. No keys pass
// through here: the code comes back to the app, which proves its challenge.
async function sso(q) {
  show('sso');
  const why = document.getElementById('sso-why');
  const clientId = q.get('clientId');
  const redirectUri = q.get('redirectUri');
  const state = q.get('state');
  const challenge = q.get('codeChallenge');
  if (
    !['desktop', 'browser', 'mobile', 'cli'].includes(clientId) ||
    !redirectUri ||
    !state ||
    !challenge
  ) {
    why.textContent =
      'This sign-in link is missing what Bitwarden sends. Start again from the app.';
    return;
  }
  const identifier = q.get('identifier') || domain;
  try {
    const r = await fetch(
      `/identity/sso/prevalidate?domainHint=${encodeURIComponent(identifier)}`,
    );
    if (!r.ok) throw new Error(r.status);
    const { token } = await r.json();
    const q2 = new URLSearchParams({
      client_id: clientId,
      redirect_uri: redirectUri,
      response_type: 'code',
      scope: 'api offline_access',
      state: `${state}_identifier=${identifier}`,
      code_challenge: challenge,
      code_challenge_method: 'S256',
      response_mode: 'query',
      domain_hint: identifier,
      ssoToken: token,
    });
    location.replace(`/identity/connect/authorize?${q2}`);
  } catch {
    why.textContent =
      'Passwords did not answer. Check you are on the Commonty network and try again.';
  }
}

if (vault && location.pathname === '/sso-connector.html') {
  // the extension reads the code off this address itself
  show('signed-in');
} else if (vault && location.hash.startsWith('#/send/')) {
  location.replace(`/send${location.hash}`);
} else if (vault && location.hash.startsWith('#/sso')) {
  sso(new URLSearchParams(location.hash.split('?')[1] || ''));
} else if (linkedHere) {
  show('opening');
  straight();
} else if (noted()) {
  show('opening');
  handOff();
} else {
  show('use-app');
}
