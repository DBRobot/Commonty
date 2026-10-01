// The app's pages. One command, `status`, says which page applies: no name
// yet, a name whose entry does not list this device, or signed in. The
// admit page polls until the laptop has added this device.

const invoke = window.__TAURI__.core.invoke;
const $ = (id) => document.getElementById(id);
let timer = null;

function show(page) {
  for (const p of document.querySelectorAll(".page")) p.hidden = p.id !== "page-" + page;
}

function when(t) {
  return t ? new Date(t * 1000).toLocaleString() : "";
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
    $("admit-fp").textContent = st.fingerprint;
    $("admit-dirs").textContent = st.directories.map(([d, s]) => `${d.replace(/^https?:\/\//, "").replace(/\/_dd\/directory$/, "")}: ${s}`).join(" · ");
    return show("admit");
  }
  const e = st.entry;
  $("home-name").textContent = st.name;
  $("home-entry").textContent = `version ${e.version}, updated ${when(e.updated)}`;
  $("home-libraries").textContent = e.libraries;
  $("home-passkeys").textContent = e.passkeys;
  const gate = $("home-gate");
  gate.textContent = st.gate || "";
  gate.className = st.gate && st.gate.startsWith("accepted") ? "ok" : "error";
  const ul = $("home-devices");
  ul.replaceChildren(...e.devices.map((d) => {
    const li = document.createElement("li");
    li.className = d.this ? "this" : "";
    li.append(d.fingerprint);
    const tag = document.createElement("span");
    tag.className = "tag";
    tag.textContent = d.this ? "this device" : `added ${when(d.added)}`;
    if (st.root && !d.this) {
      const rm = document.createElement("button");
      rm.className = "quiet inline";
      rm.textContent = "Remove";
      rm.addEventListener("click", async () => {
        rm.disabled = true;
        try { render(await invoke("remove_device", { fingerprint: d.fingerprint })); }
        catch (err) { alert(String(err)); rm.disabled = false; }
      });
      tag.append(rm);
    }
    li.append(tag);
    return li;
  }));
  $("add-device").hidden = !st.root;
  $("no-root").hidden = st.root;
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

// the fleet's own network: this device on it, the boxes it can see
async function net(st) {
  try {
    st = st || await invoke("net_status");
  } catch (e) {
    $("net-text").textContent = String(e);
    return;
  }
  const text = $("net-text");
  if (st.running) {
    text.textContent = `On the network as ${st.name} (${st.ip}). Boxes are reached directly from here.`;
  } else if (st.joined) {
    text.textContent = `Joined before; the engine is ${st.state}${st.error ? ": " + st.error : ""}.`;
  } else {
    text.textContent = "This device is not on the network yet. Joining asks the gate for a key in your name; from then on boxes are reached directly, not through the public door.";
  }
  $("net-join").hidden = st.running;
  $("net-join").textContent = st.joined ? "Reconnect" : "Join";
  const ul = $("net-peers");
  ul.replaceChildren(...(st.peers || []).map((p) => {
    const li = document.createElement("li");
    li.append(`${p.name}  ${p.ip}`);
    const tag = document.createElement("span");
    tag.className = "tag";
    tag.textContent = p.online ? "online" : "offline";
    li.append(tag);
    return li;
  }));
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
    render(await invoke("passkey_add"));
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
$("admit-form").addEventListener("submit", async (ev) => {
  ev.preventDefault();
  $("admit-error").hidden = true;
  try {
    render(await invoke("admit_device", { publicKey: $("admit-key").value }));
    $("admit-key").value = "";
  } catch (e) {
    $("admit-error").textContent = String(e);
    $("admit-error").hidden = false;
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
