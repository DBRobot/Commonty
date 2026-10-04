// The app's own few screens. One command, `status`, says which applies: no
// name yet, a name whose entry does not list this device, or signed in -
// and then the site's own pages, which the app carries, take over. The
// admit page polls until another device of the account approves this one.

import { $, load } from '/_dd/static/kit.js';

const invoke = window.__TAURI__.core.invoke;
let timer = null;
// the digits a code from another device gave, while waiting to be approved
let digits = null;

function show(page) {
  for (const p of document.querySelectorAll(".page")) p.hidden = p.id !== "page-" + page;
}

function render(st) {
  // the corner says who only once this device is someone's
  const who = $("who");
  who.hidden = !(st.name && st.admitted);
  if (!who.hidden) {
    const a = document.createElement("span");
    a.className = "avatar";
    a.textContent = st.name[0];
    who.replaceChildren(a, document.createTextNode(st.name));
  }
  if (!st.name) return show("name");
  if (!st.admitted) {
    $("admit-match").hidden = !digits;
    $("admit-digits").textContent = digits || "";
    return show("admit");
  }
  $("home-name").textContent = st.name;
  show("home");
  net().then(goToSite);
}

// the site's own pages, which the app carries and serves to itself
// (src/site.rs): where a signed-in device on the network belongs
const site = (/Windows|Android/.test(navigator.userAgent) ? "http://commonty.localhost" : "commonty://localhost") + "/_dd/home";
$("to-site").href = site;
let offered = new URLSearchParams(location.search).has("stay");
function goToSite(st) {
  if (offered || !st || !st.running) return;
  offered = true;
  location.replace(site);
}

// the fleet's own network: this device on it
async function net(st) {
  try {
    st = st || await invoke("net_status");
  } catch (e) {
    $("net-text").textContent = String(e);
    return;
  }
  // a state word and what it means (docs/ui.md)
  const [state, word, text] = st.running
    ? ["ok", "Connected", "Your boxes are reached directly from here."]
    : st.error
      ? ["broken", "Not connected", st.error]
      : st.joined
        ? ["working", "Connecting", "Reaching your boxes…"]
        : ["off", "Not connected", "This device has not joined your boxes' network yet."];
  $("net-state").dataset.state = state;
  $("net-state").textContent = word;
  $("net-text").textContent = text;
  $("to-site").hidden = !st.running;
  $("net-join").hidden = st.running;
  $("net-join").textContent = st.joined ? "Reconnect" : "Connect";
  return st;
}

$("net-join").addEventListener("click", async () => {
  const b = $("net-join");
  b.disabled = true;
  b.textContent = "Joining…";
  $("net-error").hidden = true;
  try {
    net(await invoke("net_join"));
  } catch (e) {
    $("net-error").textContent = String(e);
    $("net-error").hidden = false;
  }
  b.disabled = false;
});

$("passkey-add").addEventListener("click", async () => {
  const b = $("passkey-add");
  const t = $("passkey-text");
  b.disabled = true;
  t.hidden = false;
  t.textContent = "Finish in the browser that just opened…";
  try {
    await invoke("passkey_add");
    t.textContent = "Done: the browser can sign in now.";
  } catch (e) {
    t.textContent = String(e);
  }
  b.disabled = false;
});

// What this device showed last time draws at once, the answer from the
// directories when it comes (kit.js load): the screen never waits on them.
async function refresh() {
  clearTimeout(timer);
  try {
    const st = await load(() => invoke("status"), render, { key: "app:status" });
    if (st.name && !st.admitted) timer = setTimeout(refresh, 5000);
  } catch (e) {
    $("error-text").textContent = String(e);
    show("error");
  }
}

$("name-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  $("name-error").hidden = true;
  try {
    await invoke("set_name", { name: $("name").value });
    refresh();
  } catch (e) {
    $("name-error").textContent = String(e);
    $("name-error").hidden = false;
  }
});
$("to-signup").addEventListener("click", () => show("signup"));
$("to-code").addEventListener("click", () => { show("code"); $("code").focus(); });
$("code-back").addEventListener("click", () => show("name"));
$("code-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  $("code-error").hidden = true;
  $("code-wait").hidden = false;
  try {
    digits = await invoke("join_with_code", { code: $("code").value });
    $("code-wait").hidden = true;
    refresh();
  } catch (e) {
    $("code-wait").hidden = true;
    $("code-error").textContent = String(e);
    $("code-error").hidden = false;
  }
});
$("to-recover").addEventListener("click", () => show("recover"));
$("recover-back").addEventListener("click", () => show("name"));
$("recover-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  $("recover-error").hidden = true;
  try {
    const next = await invoke("recover", { name: $("recover-name").value, recovery: $("recover-key").value });
    $("recover-key").value = "";
    $("recovery-key").textContent = next;
    show("recovery");
  } catch (e) {
    $("recover-error").textContent = String(e);
    $("recover-error").hidden = false;
  }
});
$("signup-back").addEventListener("click", () => show("name"));
$("signup-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const b = ev.submitter || $("signup-form").querySelector("button");
  $("signup-error").hidden = true;
  b.disabled = true;
  try {
    const recovery = await invoke("sign_up", { name: $("signup-name").value, code: $("signup-code").value });
    $("recovery-key").textContent = recovery;
    show("recovery");
  } catch (e) {
    $("signup-error").textContent = String(e);
    $("signup-error").hidden = false;
  }
  b.disabled = false;
});
$("recovery-done").addEventListener("click", () => {
  $("recovery-key").textContent = "";
  refresh();
  // on the network from the first minute
  invoke("net_join").then((st) => net(st)).catch(() => {});
});
// signing out: the remembered screen goes with the name, or it would draw
// the signed-in one again for a moment
async function signOut() {
  await invoke("forget");
  try { localStorage.removeItem("app:status"); } catch { /* nothing kept */ }
  refresh();
}
$("admit-back").addEventListener("click", signOut);
$("home-forget").addEventListener("click", signOut);
$("error-retry").addEventListener("click", refresh);

refresh();
