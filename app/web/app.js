// The app's own few screens. One command, `status`, says which applies: no
// name yet, a name whose entry does not list this device, or signed in -
// and then the site's own pages, which the app carries, take over. The
// admit page polls until another device of the account approves this one.

import { $, load } from '/_dd/static/kit.js';
import { consent } from '/_dd/static/age.js';

const invoke = window.__TAURI__.core.invoke;
let timer = null;
// the digits a code from another device gave, while waiting to be approved
let digits = null;

function show(page) {
  const was = document.activeElement?.closest(".page");
  for (const p of document.querySelectorAll(".page")) p.hidden = p.id !== "page-" + page;
  // focus on a screen that just went: the new screen's title takes it
  const title = $("page-" + page)?.querySelector("h1");
  if (was?.hidden && title) {
    title.tabIndex = -1;
    title.focus();
  }
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
  browsers();
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
  // a state word and what it means (docs/dev/ui.md)
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

// the browsers this device let in (src/local.rs), each with a way to forget it
const day = (t) => new Date(t * 1000).toLocaleDateString(undefined, { day: "numeric", month: "short", year: "numeric" });
async function browsers() {
  const list = await invoke("browsers").catch(() => []);
  $("browsers-none").hidden = list.length > 0;
  $("browsers").replaceChildren(...list.map((b) => {
    const li = document.createElement("li");
    const text = document.createElement("span");
    text.textContent = `${b.name}, let in ${day(b.made)}, last used ${day(b.seen)}`;
    const forget = document.createElement("button");
    forget.className = "quiet";
    forget.textContent = "Forget";
    forget.addEventListener("click", async () => {
      forget.disabled = true;
      await invoke("browser_forget", { id: b.id }).catch(() => {});
      browsers();
    });
    li.append(text, forget);
    return li;
  }));
}

// starting by itself at login (or at boot, on a phone), on unless turned off
const phone = /Android/.test(navigator.userAgent);
if (phone) {
  $("bg-text").textContent = "Commonty keeps running with a quiet notification, so your pages still open in your browser.";
  $("bg-label").textContent = "Start Commonty when the phone starts";
}
invoke("background_status").then((st) => {
  $("bg-login").checked = st.on;
  $("bg").hidden = !st.can;
}).catch(() => {});
$("bg-login").addEventListener("change", async () => {
  const box = $("bg-login");
  $("bg-error").hidden = true;
  box.disabled = true;
  try {
    await invoke("background_set", { on: box.checked });
  } catch (e) {
    box.checked = !box.checked;
    $("bg-error").textContent = String(e);
    $("bg-error").hidden = false;
  }
  box.disabled = false;
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
// the age question and "I agree" (age.js): a refusal under 13 is kept in
// the app's store, and the answer itself never leaves this page
const signupReady = consent($("signup-go"), {
  remembered: () => invoke("age_refused"),
  remember: () => invoke("refuse_age"),
});
for (const a of document.querySelectorAll("[data-policy]")) {
  a.addEventListener("click", (e) => {
    e.preventDefault();
    invoke("open_policy", { which: a.dataset.policy }).catch(() => {});
  });
}
$("signup-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  const b = $("signup-go");
  if (!(await signupReady)()) return;
  $("signup-error").hidden = true;
  b.disabled = true;
  try {
    const recovery = await invoke("sign_up", { name: $("signup-name").value, code: $("signup-code").value, agreed: true });
    $("recovery-key").textContent = recovery;
    show("recovery");
  } catch (e) {
    $("signup-error").textContent = String(e);
    $("signup-error").hidden = false;
  }
  b.disabled = !(await signupReady)();
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
