// Settings: the member's own. Their email: where Commonty's mail to them
// goes, forwarded by Cloudflare (box/verify/src/mail_forward.rs). Changing it
// asks for their passkey first, then the new address, which its owner
// confirms from Cloudflare's message; this page notices when they have.
import { requestOptions, assertion, post } from './webauthn.js';

const $ = (id) => document.getElementById(id);
const say = (t) => { $('msg').textContent = t; };
let me = null;
let next = null;

async function state() {
  const r = await fetch('/_dd/email/state');
  if (r.status === 401) { location.href = '/_dd/login?rd=/_dd/settings'; return; }
  if (!r.ok) { $('state').textContent = 'Email is not set up on this box.'; $('changer').hidden = true; return; }
  const s = await r.json();
  const confirmed = s.forwarding === true && s.confirmed === true;
  const waiting = s.forwarding === true && s.confirmed === false;
  $('state').classList.toggle('on', confirmed);
  $('state').textContent = s.forwarding === null ? 'Your email could not be checked just now.'
    : confirmed ? 'Your email is confirmed.'
      : waiting ? 'Waiting for you to confirm: open the message from Cloudflare and press its link.'
        : 'You have no email yet.';
  $('change').textContent = s.forwarding ? 'Change email' : 'Add email';
  $('again').hidden = !waiting;
  clearTimeout(next);
  if (waiting) next = setTimeout(state, 3000);
}

// your passkey, then the new address
$('change').onclick = async () => {
  say('Confirm it is you with your passkey…');
  try {
    if (!me) me = await (await fetch('/_dd/me')).json();
    const start = await post('/_dd/login/start', { username: me.user });
    const { publicKey, ceremony } = await start.json();
    const cred = await navigator.credentials.get({ publicKey: requestOptions(publicKey) });
    await post('/_dd/login/finish', assertion(cred), { 'x-dd-ceremony': ceremony });
    say('');
    $('changer').hidden = true;
    $('form').hidden = false;
    $('email').focus();
  } catch (e) {
    say('Your passkey was not confirmed: ' + e.message);
  }
};

$('form').addEventListener('submit', async (e) => {
  e.preventDefault();
  $('save').disabled = true;
  say('Saving…');
  try {
    const r = await fetch('/_dd/email', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ email: $('email').value }),
    });
    if (!r.ok) throw new Error((await r.text()) || `the box said ${r.status}`);
    const d = await r.json();
    $('email').value = '';
    $('form').hidden = true;
    $('changer').hidden = false;
    say(d.confirm ? 'Saved. Cloudflare has sent that address a message: press the link in it, and this page will notice.' : 'Saved.');
    await state();
  } catch (err) {
    say(err.message);
  } finally {
    $('save').disabled = false;
  }
});

$('again').onclick = async () => {
  say('Sending it again…');
  const r = await fetch('/_dd/email/resend', { method: 'POST', headers: { 'content-type': 'application/json' }, body: '{}' });
  say(r.ok ? 'Sent again. Open it and press the link.' : 'That did not work; try again in a minute.');
};

state();
