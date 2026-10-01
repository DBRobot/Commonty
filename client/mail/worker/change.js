// Changing your email, a step a page: your passkey, the new address, then
// waiting for you to open the link we sent. Back to where you came from
// when it is done. From sign-up the same, for a first address.
import { signIn, me, post } from './passkey.js';

const $ = (id) => document.getElementById(id);
const name = new URLSearchParams(location.search).get('name') || '';
const back = document.body.dataset.back;
const first = document.body.dataset.mode === 'start';
let wait = null;

function step(n) {
  for (const i of [1, 2, 3]) $(`s${i}`).hidden = i !== n;
  if (n === 2) $('email').focus();
}

function done() {
  const u = new URL(back);
  if (!first) u.searchParams.set('email', 'changed');
  location.replace(u.href);
}

function bad(where, e) {
  $(where).textContent = String(e.message || e);
  $(where).hidden = false;
}

if (first) {
  $('back').textContent = '← Back';
  $('s1-lead').textContent = 'Use your passkey to add your email.';
  $('s2-h').textContent = 'Your email';
  $('cancel').hidden = true;
}
$('back').href = back;
$('cancel').onclick = () => location.replace(back);

$('pass').onclick = async () => {
  $('pass').disabled = true;
  $('s1-msg').hidden = true;
  try {
    await signIn(name);
    step(2);
  } catch (e) {
    bad('s1-msg', e.name === 'NotAllowedError' ? 'Your passkey was not used. Try again.' : e);
  } finally {
    $('pass').disabled = false;
  }
};

$('form').addEventListener('submit', async (e) => {
  e.preventDefault();
  $('save').disabled = true;
  $('s2-msg').hidden = true;
  try {
    const email = $('email').value.trim();
    const r = await post('/api/email', { email });
    if (!r.confirm) return done();
    $('sent').textContent = email;
    step(3);
    watch();
  } catch (err) {
    if (/confirm it is you/.test(err.message)) step(1);
    bad('s2-msg', err);
  } finally {
    $('save').disabled = false;
  }
});

function watch() {
  clearTimeout(wait);
  wait = setTimeout(async () => {
    const m = await me().catch(() => null);
    if (m?.confirmed) return done();
    watch();
  }, 3000);
}

$('again').onclick = async () => {
  $('s3-msg').textContent = 'Sending…';
  try {
    const r = await post('/api/resend', {});
    if (r.confirmed) return done();
    $('s3-msg').textContent = 'Sent again.';
  } catch (e) {
    $('s3-msg').textContent = String(e.message || e);
  }
};
$('other').onclick = () => { clearTimeout(wait); step(2); };

// signed in here a moment ago: straight to the address
me().then((m) => { if (m && m.name === name) step(2); });
