// Email: where a member's mail goes. The address is handed to the gate once
// and on to Cloudflare, which forwards <name>@<domain> to it; the box keeps
// nothing, so this page shows only whether forwarding is set
// (box/verify/src/mail_forward.rs).
const $ = (id) => document.getElementById(id);

async function state() {
  const r = await fetch('/_dd/email/state');
  if (r.status === 401) { location.href = '/_dd/login?rd=/_dd/email'; return; }
  if (!r.ok) { $('state').textContent = 'Email is not set up on this box.'; $('form').hidden = true; return; }
  const s = await r.json();
  $('address').textContent = s.address;
  const on = s.forwarding === true;
  $('state').classList.toggle('on', on);
  $('state').textContent = s.forwarding === null
    ? 'Whether your mail is forwarded could not be checked just now.'
    : on ? 'Your mail is forwarded. Give another address to change where it goes.'
      : 'Your mail is not going anywhere yet.';
}

$('form').addEventListener('submit', async (e) => {
  e.preventDefault();
  $('save').disabled = true;
  $('said').textContent = 'Saving…';
  try {
    const r = await fetch('/_dd/email', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ email: $('email').value }),
    });
    if (!r.ok) throw new Error((await r.text()) || `the box said ${r.status}`);
    const d = await r.json();
    $('email').value = '';
    $('said').textContent = d.confirm
      ? 'Saved. Cloudflare has sent that address a message: click the link in it to start receiving mail.'
      : 'Saved. Your mail now goes there.';
    await state();
  } catch (err) {
    $('said').textContent = err.message;
  } finally {
    $('save').disabled = false;
  }
});
state();
