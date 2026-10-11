// photos.<domain> and vault.<domain> in a browser (use-app.html): their
// clients hold a person's keys, so they come with the app (app/src/local.rs)
// and never from here. The button hands over to the app (commonty://open),
// which opens this browser at its own copy. What still has to work here
// without a client: a Send opened from inside, and a Bitwarden desktop
// app's or browser extension's sign-in, which starts and ends on the web
// vault's address.
const vault = location.hostname.startsWith('vault.');
const domain = location.hostname.split('.').slice(1).join('.');
const home = `https://home.${domain}`;
const show = (id) => {
  for (const s of document.querySelectorAll('main > section'))
    s.hidden = s.id !== id;
};

document.getElementById('home').href = `${home}/_dd/home`;
document.getElementById('download').href = `${home}/_dd/download`;
for (const id of ['what', 'what-2', 'what-3']) {
  document.getElementById(id).textContent = vault ? 'Passwords' : 'Photos';
}

// The page asked for, for the app to open after its door: the vault's
// own pages are after the #, Photos' are paths.
function asked() {
  if (vault) return location.hash.startsWith('#/') ? `/${location.hash}` : '/';
  return location.pathname + location.search;
}

// A browser says nothing when no app takes the link: if this page is still
// in front a few seconds on, the app did not open.
function handOff() {
  document.getElementById('not-open').hidden = true;
  let left = false;
  const away = () => {
    left = true;
  };
  window.addEventListener('blur', away, { once: true });
  document.addEventListener('visibilitychange', away, { once: true });
  const q = new URLSearchParams({ service: vault ? 'vault' : 'photos' });
  const path = asked();
  if (path !== '/') q.set('path', path);
  location.href = `commonty://open?${q}`;
  setTimeout(() => {
    window.removeEventListener('blur', away);
    document.removeEventListener('visibilitychange', away);
    if (!left && document.visibilityState === 'visible') {
      document.getElementById('not-open').hidden = false;
    }
  }, 3000);
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
}
