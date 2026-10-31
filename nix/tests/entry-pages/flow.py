# Settings with the member's entry as the directory gives it. Since the v2
# envelopes (29e795cd) a box answers /_dd/directory/<name> with
# {"v":2,"kind":"entry","body":base64,"sig":...}, and the pages read
# `.entry` of it: Profile threw "Cannot read properties of undefined
# (reading 'root')". Here both shapes: Profile and Devices show the same
# entry from each, and the text the wasm checks before a passkey signs on
# from it is the answer as it came, held to the signature over its body.
import base64
import json
import os
import sys

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
from playwright.sync_api import TimeoutError as Timeout
from playwright.sync_api import sync_playwright

WEB = sys.argv[1]
WASM = sys.argv[2]
DOMAIN = "commonty.test"
HOME = f"https://home.{DOMAIN}"
USER = "tester"

ME = {
    "user": USER,
    "initial": "t",
    "demo": False,
    "role": "member",
    "home": HOME,
    "menu": [[{"label": "Settings", "url": "/_dd/settings"}]],
    "services": [],
}
CONFIG = {"rpId": DOMAIN, "envelope": 2}

b64 = lambda b: base64.b64encode(b).decode()


def public(k):
    return b64(k.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw))


device = Ed25519PrivateKey.generate()
recovery = Ed25519PrivateKey.generate()
# a passkey account: one device, its root passkey
ENTRY = {
    "name": USER,
    "root": "webauthn:pk1:digest",
    "recovery": public(recovery),
    "devices": [
        {
            "fingerprint": base64.b64decode(public(device))[:6].hex(),
            "public_key": public(device),
            "added": 1700000000,
        }
    ],
    "passkeys": [{"id": "pk1", "cred": {}, "added": 1700000000}],
    "libraries": [],
    "version": 3,
    "updated": 1700000000,
}


def envelope(entry, key=None):
    body = json.dumps(entry, separators=(",", ":")).encode()
    sig = key.sign(b"commonty entry v2\0" + body) if key else b"not signed"
    return {"v": 2, "kind": "entry", "body": b64(body), "sig": b64(sig)}


SHAPES = {
    "v2": json.dumps(envelope(ENTRY)),
    "v1": json.dumps({"entry": ENTRY, "signature": b64(b"not signed")}),
}

# a device-key account's envelope, signed by its root: the wasm takes it,
# and refuses the same with a device slipped into the body
root = Ed25519PrivateKey.generate()
OWN = dict(ENTRY, root=public(root), passkeys=[])
signed = envelope(OWN, root)
slipped = dict(signed)
slipped["body"] = b64(
    json.dumps(
        dict(
            OWN,
            devices=OWN["devices"]
            + [dict(OWN["devices"][0], fingerprint="ffffffffffff")],
        ),
        separators=(",", ":"),
    ).encode()
)

TYPES = {
    ".js": "text/javascript",
    ".css": "text/css",
    ".svg": "image/svg+xml",
    ".woff2": "font/woff2",
    ".wasm": "application/wasm",
}
shape = "v2"


def static(route, f):
    if not os.path.isfile(f):
        return route.fulfill(status=404, body="")
    return route.fulfill(
        status=200,
        content_type=TYPES.get(os.path.splitext(f)[1], "application/octet-stream"),
        body=open(f, "rb").read(),
    )


def answer(route):
    req = route.request
    u = req.url.split("#")[0]
    origin = "/".join(u.split("/")[:3])
    path = "/" + "/".join(u.split("/")[3:]).split("?")[0]
    if origin != HOME:
        return route.abort()
    js = lambda v: route.fulfill(
        status=200, content_type="application/json", body=json.dumps(v)
    )
    if path == "/_dd/me":
        return js(ME)
    if path == "/_dd/config":
        return js(CONFIG)
    if path == f"/_dd/directory/{USER}":
        return route.fulfill(
            status=200, content_type="application/json", body=SHAPES[shape]
        )
    if path == "/_dd/signins":
        return js({"sessions": [], "keys": {}})
    if path == "/_dd/boxes/mine":
        return js({"boxes": [], "waiting": []})
    if path.startswith("/_dd/static/"):
        return static(route, os.path.join(WEB, "static", path[len("/_dd/static/") :]))
    if path.startswith("/_dd/web/"):
        return static(route, os.path.join(WASM, path[len("/_dd/web/") :]))
    if path == "/_dd/settings" and req.resource_type == "document":
        return route.fulfill(
            status=200,
            content_type="text/html",
            body=open(os.path.join(WEB, "pages", "settings.html")).read(),
        )
    return route.fulfill(status=404, body="")


failed = []


def check(what, ok):
    print(("ok  " if ok else "FAIL") + " " + what)
    if not ok:
        failed.append(what)


with sync_playwright() as p:
    b = p.chromium.launch(args=["--mute-audio"])
    for shape, text in SHAPES.items():
        ctx = b.new_context()
        ctx.route("**/*", answer)
        page = ctx.new_page()
        errors = []
        page.on("pageerror", lambda e, errors=errors: errors.append(str(e)))

        page.goto(f"{HOME}/_dd/settings#profile")
        try:
            page.wait_for_function(
                "() => !document.getElementById('recovery').textContent.includes('…')",
                timeout=10000,
            )
        except Timeout:
            check(f"{shape}: Profile is drawn ({errors})", False)
            ctx.close()
            continue
        check(
            f"{shape}: Profile counts the entry's keys",
            page.inner_text("#devices").startswith("1 device, 1 passkey"),
        )
        check(
            f"{shape}: Profile shows the recovery key",
            page.inner_text("#recovery").strip() == "On file",
        )

        page.click("[data-tab='devices']")
        page.wait_for_function(
            "() => document.querySelectorAll('#dv-keys > *').length > 0", timeout=10000
        )
        check(
            f"{shape}: Devices lists the device and the passkey",
            page.eval_on_selector_all("#dv-keys > *", "l => l.length") == 2,
        )
        check(
            f"{shape}: Devices says nothing went wrong",
            page.inner_text("#dv-msg").strip() == "",
        )

        # what the page hands the wasm: the answer exactly as it came
        same = page.evaluate(
            """async (t) => {
              const { entryOf, entryText } = await import('/_dd/static/shell.js');
              const e = await entryOf('tester');
              return e.text === await entryText('tester') && e.text === t && !JSON.stringify(e).includes('"text"');
            }""",
            text,
        )
        check(f"{shape}: entryOf keeps the directory's text as it came", same)
        check(f"{shape}: no page error", not errors)
        for e in errors:
            print("   ", e)
        ctx.close()

    # the wasm on the v2 text the page hands it: the root's signature over
    # the body is checked before anything is planned from it
    ctx = b.new_context()
    ctx.route("**/*", answer)
    page = ctx.new_page()
    page.goto(f"{HOME}/_dd/settings#profile")
    said = page.evaluate(
        """async (texts) => {
          const { readEntry } = await import('/_dd/static/kit.js');
          const w = await import('/_dd/web/dd_web.js');
          await w.default();
          const out = {};
          for (const [k, t] of Object.entries(texts)) {
            try { w.entry_without(readEntry(t).text, 'ffffffffffff', undefined, 5n); out[k] = 'planned'; }
            catch (e) { out[k] = String(e.message || e); }
          }
          return out;
        }""",
        {
            "signed": json.dumps(signed),
            "slipped": json.dumps(slipped),
            "unsigned": SHAPES["v2"],
        },
    )
    print("   wasm said", said)
    check(
        "wasm: a signed v2 entry passes its check (a device root is then the app's)",
        "main key" in said["signed"],
    )
    check(
        "wasm: a device slipped into the body is refused",
        "not signed by its own key" in said["slipped"],
    )
    check(
        "wasm: an unsigned passkey entry is refused",
        "not signed by its own key" in said["unsigned"],
    )
    ctx.close()
    b.close()

if failed:
    print(f"{len(failed)} failed")
    sys.exit(1)
print("all passed")
