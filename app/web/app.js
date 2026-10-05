// The app's own few screens. One command, `status`, says which applies: no
// name yet, a name whose entry does not list this device, or signed in -
// and then the site's own pages, which the app carries, take over. The
// admit page polls until another device of the account approves this one.

const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);
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
  const text = $("net-text");
  if (st.running) {
    text.textContent = "Connected to your boxes.";
  } else if (st.joined) {
    text.textContent = st.error ? `Not connected: ${st.error}` : "Connecting to your boxes…";
  } else {
    text.textContent = "Not connected to your boxes yet.";
  }
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

async function refresh() {
  try {
    const st = await invoke("status");
    render(st);
    clearTimeout(timer);
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
$("admit-back").addEventListener("click", async () => { await invoke("forget"); refresh(); });
$("home-forget").addEventListener("click", async () => { await invoke("forget"); refresh(); });
$("error-retry").addEventListener("click", refresh);

refresh();
