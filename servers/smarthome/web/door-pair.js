// Pairing this controller, on the house network (servers/smarthome/door/src/pair.rs).
// The Commonty app's installer does the pairing; this page says how far it
// has got, and how a card written without the app gets its pairing key.
import { $ } from './kit.js';

function draw(s) {
  const restore = s.restore || {};
  const [state, word, text] = s.finished
    ? ['ok', 'Paired', 'This home is ready. Open it from the Commonty app.']
    : restore.state === 'failed'
      ? ['broken', 'Restore failed', `Your backup could not be put back: ${restore.error || 'Home Assistant did not take it'}. Try again from the Commonty app.`]
      : restore.state === 'working'
        ? ['working', 'Restoring', 'Putting your backup back. Home Assistant restarts on its own; this takes a few minutes.']
        : s.paired
          ? ['working', 'Finishing', 'Paired with your account. The controller is joining your network and getting its name.']
          : s.pairing
            ? ['working', 'Waiting', 'Waiting for the Commonty app. Go back to its installer: it finds this controller by itself.']
            : ['off', 'No pairing key', 'Put this card in your computer and add a pairing key with the Commonty app, then put it back in the Pi.'];
  $('state').dataset.state = state;
  $('state').textContent = word;
  $('text').textContent = text;
  return s.finished;
}

async function poll() {
  try {
    const r = await fetch('/pair/status');
    if (draw(await r.json())) return;
  } catch {
    $('state').dataset.state = 'broken';
    $('state').textContent = 'Not answering';
    $('text').textContent = 'The controller is not answering. If pairing just finished, it has stopped listening here: open your home from the Commonty app.';
  }
  setTimeout(poll, 3000);
}
poll();
