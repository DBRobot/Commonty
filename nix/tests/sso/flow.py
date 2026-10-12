# Signing in once for every box, in a browser (sso.nix). The member signs in
# at the id page on one name, is handed on to another name of the box with no
# passkey prompt, and what a box must refuse is refused.
import base64
import hashlib
import json
import sys
import urllib.error
import urllib.parse
import urllib.request
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric import ec
from playwright.sync_api import sync_playwright

GATE = "http://127.0.0.1:4181"
WORKER = "http://127.0.0.1:8788"
ID = "https://id.commonty.test"


def b64u(b):
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


# the member's passkey (client/mail/examples/fixture.rs): P-256 from [9; 32]
PASSKEY = ec.derive_private_key(int.from_bytes(bytes([9] * 32), "big"), ec.SECP256R1())
CRED_ID = json.load(open(sys.argv[1]))["v5"]["entry"]["passkeys"][0]["id"]
prompts = []


def sign(challenge, origin=ID):
    """the authenticator: present, for commonty.test, made on `origin`"""
    prompts.append(challenge)
    client = json.dumps(
        {"type": "webauthn.get", "challenge": challenge, "origin": origin}
    ).encode()
    auth = hashlib.sha256(b"commonty.test").digest() + bytes([0x05, 0, 0, 0, 1])
    sig = PASSKEY.sign(
        auth + hashlib.sha256(client).digest(), ec.ECDSA(hashes.SHA256())
    )
    return {
        "id": CRED_ID,
        "authenticatorData": b64u(auth),
        "clientDataJSON": b64u(client),
        "signature": b64u(sig),
    }


STUB = """
Object.defineProperty(navigator, 'credentials', { configurable: true, value: { get: async (o) => {
  const c = btoa(String.fromCharCode(...new Uint8Array(o.publicKey.challenge))).replace(/\\+/g,'-').replace(/\\//g,'_').replace(/=+$/,'');
  const a = await window.__sign(c);
  const u = (s) => Uint8Array.from(atob(s.replace(/-/g,'+').replace(/_/g,'/')), x => x.charCodeAt(0)).buffer;
  return { rawId: u(a.id), response: { authenticatorData: u(a.authenticatorData), clientDataJSON: u(a.clientDataJSON), signature: u(a.signature) } };
} } });
"""

posted = []  # the hand-offs as the browser posted them


# --- the internet, here: one https front for every name, with its own
# certificate, sending id. to the Worker and the rest to the gate as nginx
# would (the asked name in X-Original-Host)
def front():
    import datetime
    import ssl
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from cryptography import x509
    from cryptography.hazmat.primitives import serialization
    from cryptography.x509.oid import NameOID

    key = ec.generate_private_key(ec.SECP256R1())
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, "commonty.test")])
    now = datetime.datetime.now(datetime.timezone.utc)
    cert = (
        x509.CertificateBuilder()
        .subject_name(name)
        .issuer_name(name)
        .public_key(key.public_key())
        .serial_number(1)
        .not_valid_before(now - datetime.timedelta(days=1))
        .not_valid_after(now + datetime.timedelta(days=1))
        .add_extension(
            x509.SubjectAlternativeName(
                [x509.DNSName("*.commonty.test"), x509.DNSName("commonty.test")]
            ),
            critical=False,
        )
        .sign(key, hashes.SHA256())
    )
    open("front.pem", "wb").write(cert.public_bytes(serialization.Encoding.PEM))
    open("front.key", "wb").write(
        key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        )
    )
    spki = cert.public_key().public_bytes(
        serialization.Encoding.DER, serialization.PublicFormat.SubjectPublicKeyInfo
    )

    class NoFollow(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):
            return None

    opener = urllib.request.build_opener(NoFollow)

    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *a):
            pass

        def go(self):
            host = (self.headers.get("host") or "").split(":")[0]
            body = self.rfile.read(int(self.headers.get("content-length") or 0)) or None
            if self.command == "POST" and self.path == "/_dd/sso":
                posted.append((host, body.decode(), self.headers.get("origin")))
            back = WORKER if host == "id.commonty.test" else GATE
            headers = {
                k: v
                for k, v in self.headers.items()
                if k.lower() not in ("host", "connection", "content-length")
            }
            headers["host"] = host
            headers["x-original-host"] = host
            req = urllib.request.Request(
                back + self.path, data=body, method=self.command, headers=headers
            )
            try:
                r = opener.open(req)
            except urllib.error.HTTPError as e:
                r = e
            data = r.read()
            self.send_response(r.status if hasattr(r, "status") else r.code)
            for k, v in r.headers.items():
                if k.lower() not in (
                    "transfer-encoding",
                    "connection",
                    "content-length",
                ):
                    self.send_header(k, v)
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        do_GET = do_POST = do_HEAD = go

    srv = ThreadingHTTPServer(("127.0.0.1", 8443), H)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain("front.pem", "front.key")
    srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return base64.b64encode(hashlib.sha256(spki).digest()).decode()


def post(host, body, origin=ID):
    """a hand-off sent straight to the gate, as anyone could"""
    req = urllib.request.Request(
        f"{GATE}/_dd/sso",
        data=body.encode(),
        method="POST",
        headers={
            "content-type": "application/x-www-form-urlencoded",
            "host": host,
            "x-original-host": host,
            **({"origin": origin} if origin else {}),
        },
    )

    class NoFollow(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):
            return None

    try:
        r = urllib.request.build_opener(NoFollow).open(req)
        return (
            r.status,
            r.headers.get("set-cookie") or "",
            r.headers.get("location") or "",
        )
    except urllib.error.HTTPError as e:
        return (
            e.code,
            e.headers.get("set-cookie") or "",
            e.headers.get("location") or "",
        )


failed = []


def check(what, ok):
    print(("ok  " if ok else "FAIL") + " " + what)
    if not ok:
        failed.append(what)


with sync_playwright() as p:
    # every name of the fleet is the front, whose certificate the browser takes
    spki = front()
    b = p.chromium.launch(
        args=[
            "--mute-audio",
            "--host-resolver-rules=MAP *.commonty.test:443 127.0.0.1:8443",
            f"--ignore-certificate-errors-spki-list={spki}",
        ]
    )
    ctx = b.new_context()
    ctx.expose_function("__sign", sign)
    ctx.add_init_script(STUB)
    page = ctx.new_page()
    errors = []
    page.on("pageerror", lambda e: errors.append(str(e)))

    # 1. nobody signed in: the box sends the browser to the id page
    page.goto("https://home.commonty.test/_dd/login?rd=/_dd/me")
    page.wait_for_url("https://id.commonty.test/go?**")
    page.wait_for_selector("#signin:not([hidden])")
    check(
        "a box sends a stranger to the id page",
        page.url.startswith(
            "https://id.commonty.test/go?host=home.commonty.test&nonce="
        ),
    )
    page.fill("#u", "tester")
    page.click("#go")
    try:
        page.wait_for_url("https://home.commonty.test/_dd/me", timeout=20000)
    except Exception:
        print(
            "stuck at",
            page.url,
            "| message:",
            page.evaluate("() => document.getElementById('msg')?.textContent"),
            "| posted:",
            [(h, o) for h, _, o in posted],
            "| prompts:",
            len(prompts),
        )
        print(page.content()[:1500])
        raise
    check(
        "after the passkey, back where it was going",
        page.url == "https://home.commonty.test/_dd/me",
    )
    check("one passkey prompt", len(prompts) == 1)
    cookies = {c["domain"]: c for c in ctx.cookies()}
    c = cookies.get("home.commonty.test")
    check(
        "a cookie for that name alone",
        c is not None
        and c["name"] == "__Host-dd"
        and c["secure"]
        and c["httpOnly"]
        and not c["domain"].startswith("."),
    )
    me = page.evaluate("() => fetch('/_dd/me').then(r => r.json())")
    check("the box knows who it is", (me or {}).get("user") == "tester")

    # 2. another name of the box: handed on, no prompt
    page.goto("https://files.commonty.test/_dd/login?rd=/_dd/me")
    page.wait_for_url("https://files.commonty.test/_dd/me", timeout=20000)
    check("another name: in, without the passkey", len(prompts) == 1)
    check(
        "its own cookie", "files.commonty.test" in {c["domain"] for c in ctx.cookies()}
    )

    # 3. what a box must refuse - the hand-offs the browser posted, sent again
    host, body, origin = posted[0]
    check("the browser posted from the id page", origin == ID)
    status, cookie, _ = post(host, body)
    check(
        "a hand-off is good once", "__Host-dd=" not in cookie and status in (302, 303)
    )
    # the second one (for files), to the other name
    _, body2, _ = posted[1]
    status, cookie, _ = post("home.commonty.test", body2)
    check("a hand-off is for the name it was signed for", "__Host-dd=" not in cookie)

    # a fresh challenge, answered by a signature for something else
    class Stay(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):
            return None

    try:
        urllib.request.build_opener(Stay).open(
            urllib.request.Request(
                f"{GATE}/_dd/sso/start?rd=/", headers={"host": "home.commonty.test"}
            )
        )
        loc = ""
    except urllib.error.HTTPError as e:
        loc = e.headers.get("location") or ""
    fields = dict(urllib.parse.parse_qsl(body))
    nonce = dict(urllib.parse.parse_qsl(urllib.parse.urlsplit(loc).query)).get(
        "nonce", ""
    )
    check(
        "a box's challenge goes to the id page",
        loc.startswith("https://id.commonty.test/go?host=home.commonty.test&nonce="),
    )
    forged = urllib.parse.urlencode(
        {"d": fields["d"], "sig": fields["sig"], "nonce": nonce}
    )
    status, cookie, _ = post("home.commonty.test", forged)
    check(
        "a signature over another challenge is refused",
        status == 401 and "__Host-dd=" not in cookie,
    )
    status, cookie, _ = post("home.commonty.test", body, origin=None)
    check("a hand-off not posted by the id page is refused", status == 403)
    status, cookie, _ = post(
        "home.commonty.test", body, origin="https://files.commonty.test"
    )
    check("nor one posted by a box's page", status == 403)

    # 3b. signed out everywhere: the entry says every sign-in before now is
    # over (as `dd device sign-out` or the Devices tab would sign it), and
    # each box reads it - sessions end, and the sign-in kept in this browser
    # no longer hands off
    import time as _t

    path = "gate/keys/tester.json"
    signed = json.load(open(path))
    _t.sleep(1.1)
    signed["entry"]["signed_out"] = int(_t.time())
    json.dump(signed, open(path, "w"))
    gone = page.evaluate("() => fetch('/_dd/me').then(r => r.json()).catch(() => null)")
    check(
        "signed out everywhere: this name's session is over",
        (gone or {}).get("user") != "tester",
    )
    page.goto("https://files.commonty.test/_dd/me")
    check(
        "and the other name's",
        "tester" not in (page.content() or "")
        or page.url.startswith("https://id.commonty.test"),
    )
    before = len(prompts)
    page.goto("https://home.commonty.test/_dd/login?rd=/_dd/me")
    page.wait_for_url("https://home.commonty.test/_dd/sso", timeout=20000)
    check(
        "the sign-in kept from before is refused at the box",
        "not accepted" in page.content() and len(prompts) == before,
    )
    del signed["entry"]["signed_out"]
    json.dump(signed, open(path, "w"))

    # 4. signing out here forgets the browser's sign-in: the next name asks again
    page.goto("https://id.commonty.test/out?then=https://id.commonty.test/")
    page.wait_for_selector("#signin:not([hidden])")
    ctx.clear_cookies()
    page.goto("https://files.commonty.test/_dd/login?rd=/_dd/me")
    page.wait_for_selector("#signin:not([hidden])")
    check(
        "signed out: the id page asks for the passkey again",
        page.url.startswith("https://id.commonty.test/go?"),
    )
    # and /out sends nowhere but this fleet
    page.goto("https://id.commonty.test/out?then=https://evil.example/")
    page.wait_for_selector("#gone:not([hidden])")
    check(
        "/out does not send the browser off the fleet",
        page.url.startswith("https://id.commonty.test/out"),
    )

    check("no page errors", not errors)
    if errors:
        print("\n".join(errors))
    b.close()

if failed:
    print(f"{len(failed)} failed")
    sys.exit(1)
print("all passed")
