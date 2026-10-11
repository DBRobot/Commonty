// Leave Commonty, a tab of Settings: how to take everything out, and the
// button. Leaving is a statement the member's root signs, agreed by every
// box (servers/gate/src/leaving.rs): their passkey for an account made in a
// browser, the app for one whose root is on a device. Within 30 days, the
// same tab brings them back with nothing lost.
import { requestOptions, assertion, post } from './webauthn.js';
import { docsUrl, inApp, servedByApp, appFetch } from './shell.js';

const $ = (id) => document.getElementById(id);
const day = (t) => new Date(t * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric' });

let state = null;

export async function where() {
  if (!state) {
    const r = await fetch('/_dd/leave');
    if (!r.ok) throw new Error(`the box said ${r.status}`);
    state = await r.json();
  }
  return state;
}

function links() {
  const domain = location.hostname.split('.').slice(-2).join('.');
  const set = (id, href) => { const a = $(id); if (a) a.href = href; };
  set('lv-files', `https://files.${domain}/`);
  set('lv-photos', `https://photos.${domain}/`);
  set('lv-files-docs', docsUrl('/dev/libraries#from-a-terminal'));
  set('lv-images-docs', docsUrl('/dev/libraries#disk-images'));
  set('lv-docs', docsUrl('/dev/leaving'));
}

// where the app serves this page, the app signs with the root it holds
const byApp = inApp || servedByApp;

// the root signs the statement; the box puts it in the fleet's log
async function say(back) {
  if (!state.passkey) {
    const init = {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ back }),
    };
    const r = await (inApp ? appFetch('/_dd/app/leaving', init) : fetch('/_dd/app/leaving', init));
    if (!r.ok) throw new Error(await r.text());
    return;
  }
  const start = await post('/_dd/leave/start', { back });
  const { publicKey, ceremony } = await start.json();
  const cred = await navigator.credentials.get({ publicKey: requestOptions(publicKey) });
  await post('/_dd/leave/sign', assertion(cred), { 'x-dd-ceremony': ceremony });
}

function show(s) {
  // anyone may leave: a member, a guest, or an account still waiting
  const role = s.role || (s.member ? 'member' : 'waiting');
  $('lv-member').hidden = !!s.left;
  $('lv-take-box').hidden = role !== 'member' || !!s.left;
  for (const r of ['member', 'guest', 'waiting']) $(`lv-as-${r}`).hidden = role !== r;
  $('lv-left').hidden = !s.left;
  if (s.left) {
    $('lv-since').textContent = day(s.left);
    $('lv-until').textContent = day(s.until);
    // the owner's removal is undone by the owner, not here
    $('lv-back').hidden = s.removed;
    $('lv-removed').hidden = !s.removed;
  }
  if (!s.passkey) {
    $('lv-ask-how').hidden = true;
    $('lv-ask-yes').textContent = 'Leave';
  }
  // a root on a device signs only through the app that holds it
  if (!s.passkey && !byApp) {
    $('lv-go').hidden = true;
    $('lv-back').hidden = true;
    $('lv-device').hidden = false;
  }
}

async function act(back, button) {
  button.disabled = true;
  const msg = $(back ? 'lv-msg' : 'lv-ask-msg');
  msg.textContent = '';
  try {
    await say(back);
    if (back) {
      location.href = '/_dd/settings';
      return;
    }
    // this browser is signed out now: what it shows is the last it knew
    const t = Math.floor(Date.now() / 1000);
    $('lv-ask').close();
    show({ ...state, left: t, until: t + state.window });
    $('lv-back').hidden = true;
  } catch (e) {
    button.disabled = false;
    msg.textContent = e.name === 'NotAllowedError' ? 'Your passkey was not used.' : String(e.message || e);
  }
}

let wired = false;
export async function start() {
  links();
  if (!wired) {
    wired = true;
    $('lv-go').onclick = () => { $('lv-ask-msg').textContent = ''; $('lv-ask').showModal(); };
    $('lv-ask-no').onclick = () => $('lv-ask').close();
    $('lv-ask-yes').onclick = (e) => act(false, e.currentTarget);
    $('lv-back').onclick = (e) => act(true, e.currentTarget);
  }
  try {
    show(await where());
  } catch (e) {
    $('lv-msg').textContent = String(e.message || e);
  }
}
