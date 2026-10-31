# Signing in once for every box, in a browser (sso.nix). The member signs in
# at the id page on one name, is handed on to another name of the box with no
# passkey prompt, and what a box must refuse is refused. Then a box someone
# else holds, with a certificate for a name under the domain, gets nothing that
# works anywhere else.
import base64
import hashlib
import json
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from cryptography.hazmat.primitives import hashes, serialization
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
JOINED = "x.j0123456789.commonty.test"
at_joined = []  # every request the joined box was sent: (method, path, headers, body)
JOINED_PAGE = b"<!doctype html><title>joined</title>a member's own service"


# --- the internet, here: one https front for every name, with its own
# certificate, sending id. to the Worker and the rest to the gate as nginx
# would (the asked name in X-Original-Host)
def front():
    import datetime
    import ssl
    import threading
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    from cryptography import x509
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
                [
                    x509.DNSName("*.commonty.test"),
                    x509.DNSName("commonty.test"),
                    x509.DNSName("*.j0123456789.commonty.test"),
                ]
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
            if host == JOINED:
                # the box's own nginx and gate: it sees all the browser sends,
                # and tries what a box can: a cookie for the whole domain
                at_joined.append(
                    (
                        self.command,
                        self.path,
                        {k.lower(): v for k, v in self.headers.items()},
                        (body or b"").decode(),
                    )
                )
                self.send_response(200)
                self.send_header("content-type", "text/html")
                self.send_header("content-length", str(len(JOINED_PAGE)))
                self.send_header(
                    "set-cookie",
                    "dd_session=mallory~x.9999999999.y; Domain=commonty.test; Path=/; Secure",
                )
                self.end_headers()
                self.wfile.write(JOINED_PAGE)
                return
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

    # 2. another name of the box: there already, with no trip and no prompt
    check(
        "the box's other name has its own cookie already",
        "files.commonty.test" in {c["domain"] for c in ctx.cookies()},
    )
    trips = []
    page.on(
        "request",
        lambda r: trips.append(r.url) if r.url.startswith(ID) else None,
    )
    page.goto("https://files.commonty.test/_dd/me")
    check(
        "another name: in, with no trip",
        '"tester"' in page.content() and not trips,
    )
    check("and without the passkey", len(prompts) == 1)
    # and one the page's background hand-off did not reach makes the trip
    ctx.clear_cookies(domain="files.commonty.test")
    page.goto("https://files.commonty.test/_dd/login?rd=/_dd/me")
    page.wait_for_url("https://files.commonty.test/_dd/me", timeout=20000)
    check("a name without a session: handed on", len(prompts) == 1 and bool(trips))

    # 3. what a box must refuse - the hand-offs the browser posted, sent again
    host, body, origin = next(x for x in posted if "warm=" not in x[1])
    check("the browser posted from the id page", origin == ID)
    status, cookie, _ = post(host, body)
    check(
        "a hand-off is good once", "__Host-dd=" not in cookie and status in (302, 303)
    )
    # one for files, to the other name
    _, body2, _ = next(x for x in posted if x[0] == "files.commonty.test")
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
    status, cookie, to = post("home.commonty.test", forged)
    check(
        "a signature over another challenge is refused, and the id page asks again",
        status in (302, 303) and "__Host-dd=" not in cookie and "again=1" in to,
    )
    status, cookie, _ = post("home.commonty.test", body, origin=None)
    check("a hand-off not posted by the id page is refused", status == 403)
    status, cookie, _ = post(
        "home.commonty.test", body, origin="https://files.commonty.test"
    )
    check("nor one posted by a box's page", status == 403)

    # 4. a box someone else holds, under a name of the domain, certificate and all
    home_me = "https://home.commonty.test/_dd/me"
    page.goto(f"https://{JOINED}/")
    sent = at_joined[-1][2].get("cookie", "")
    check("the joined box is not sent a session", "__Host-dd" not in sent)
    check(
        "its cookie for the whole domain changes nobody's session",
        any(c["name"] == "dd_session" for c in ctx.cookies())
        and '"tester"' in (page.goto(home_me) and page.content()),
    )

    # its page asks home's challenge of the passkey: the browser allows it, the
    # relying party being the domain, and says where it was asked
    def ask_gate(path, body=None, headers=None):
        r = urllib.request.Request(
            GATE + path,
            data=json.dumps(body).encode() if body is not None else None,
            method="POST",
            headers={
                "host": "home.commonty.test",
                "x-original-host": "home.commonty.test",
                "content-type": "application/json",
                **(headers or {}),
            },
        )
        try:
            x = urllib.request.urlopen(r)
            return x.status, x.headers.get("set-cookie") or "", x.read()
        except urllib.error.HTTPError as e:
            return e.code, e.headers.get("set-cookie") or "", e.read()

    code, _, start = ask_gate("/_dd/login/start", {"username": "tester"})
    start = json.loads(start)
    answer = sign(start["publicKey"]["challenge"], origin=f"https://{JOINED}")
    code, cookie, _ = ask_gate(
        "/_dd/login/finish",
        {
            "id": CRED_ID,
            "rawId": CRED_ID,
            "type": "public-key",
            "extensions": {},
            "response": {
                "authenticatorData": answer["authenticatorData"],
                "clientDataJSON": answer["clientDataJSON"],
                "signature": answer["signature"],
                "userHandle": None,
            },
        },
        {"x-dd-ceremony": start["ceremony"]},
    )
    check(
        f"a sign-in its page got from the passkey opens nothing here ({code})",
        code >= 400 and not cookie,
    )

    # and a delegation of its own, to a key it holds, the passkey asked on its page
    key = ec.generate_private_key(ec.SECP256R1())
    spki = b64u(
        key.public_key().public_bytes(
            serialization.Encoding.DER,
            serialization.PublicFormat.SubjectPublicKeyInfo,
        )
    )
    sid, iat = "joined-box-sign-in-1", int(time.time())
    made = f"commonty session v1\0tester\0{spki}\0{sid}\0{iat}\0{iat + 3600}"
    chal = b64u(hashlib.sha256(made.encode()).digest())
    a = sign(chal, origin=f"https://{JOINED}")
    d = {
        "user": "tester",
        "passkey": CRED_ID,
        "key": spki,
        "sid": sid,
        "iat": iat,
        "exp": iat + 3600,
        "assertion": {
            k: a[k] for k in ("authenticatorData", "clientDataJSON", "signature")
        },
    }

    def fresh_nonce():
        try:
            urllib.request.build_opener(Stay).open(
                urllib.request.Request(
                    f"{GATE}/_dd/sso/start?rd=/", headers={"host": "home.commonty.test"}
                )
            )
        except urllib.error.HTTPError as e:
            return dict(
                urllib.parse.parse_qsl(
                    urllib.parse.urlsplit(e.headers.get("location")).query
                )
            )["nonce"]

    n = fresh_nonce()
    sig = b64u(
        key.sign(
            f"commonty handoff v1\0home.commonty.test\0{n}".encode(),
            ec.ECDSA(hashes.SHA256()),
        )
    )
    body = urllib.parse.urlencode(
        {"d": b64u(json.dumps(d).encode()), "sig": sig, "nonce": n}
    )
    status, cookie, _ = post("home.commonty.test", body)
    check(
        "a delegation signed on its page is refused, its key and the Origin header its own",
        "__Host-dd=" not in cookie,
    )

    # what the browser's real sign-in does give it: the hand-off for its own name
    page.goto(f"{ID}/go?host={JOINED}&nonce=joinedboxnonce1")
    page.wait_for_function(f"() => location.hostname === '{JOINED}'")
    got = [x for x in at_joined if x[1] == "/_dd/sso"]
    check("the id page hands the joined box its own hand-off", len(got) == 1)
    fields = dict(urllib.parse.parse_qsl(got[0][3]))
    n = fresh_nonce()
    status, cookie, _ = post(
        "home.commonty.test",
        urllib.parse.urlencode({"d": fields["d"], "sig": fields["sig"], "nonce": n}),
    )
    check(
        "replayed at a box with that box's own challenge, it opens nothing",
        "__Host-dd=" not in cookie,
    )
    status, cookie, _ = post("home.commonty.test", got[0][3])
    check("nor as it stands", "__Host-dd=" not in cookie)

    # its page, in the member's browser, cannot act with the member's session
    page.goto(f"https://{JOINED}/")
    page.evaluate(
        """() => fetch('https://home.commonty.test/_dd/logout', { method: 'POST', mode: 'no-cors', credentials: 'include' }).catch(() => {})"""
    )
    page.goto(home_me)
    check("its page cannot sign the member out", '"tester"' in page.content())

    # 5. a sign-in the box has ended: the id page asks, it does not go round again
    page.goto(f"{ID}/")
    page.evaluate(
        """() => fetch('https://home.commonty.test/_dd/sso/out', { method: 'POST', mode: 'no-cors', credentials: 'include' })"""
    )
    page.goto("https://home.commonty.test/_dd/login?rd=/_dd/me")
    page.wait_for_url("https://id.commonty.test/go?*again=1*", timeout=20000)
    page.wait_for_selector("#signin:not([hidden])")
    check("a sign-in ended here asks for the passkey", "again=1" in page.url)
    before = len(prompts)
    page.fill("#u", "tester")
    page.click("#go")
    page.wait_for_url("https://home.commonty.test/_dd/me", timeout=20000)
    check("and the passkey signs back in", len(prompts) == before + 1)

    # 6. signing out at the id page reaches the names it handed to
    page.goto("https://id.commonty.test/out?then=https://id.commonty.test/")
    page.wait_for_selector("#signin:not([hidden])")
    left = {c["domain"] for c in ctx.cookies() if c["name"] == "__Host-dd"}
    check(f"no name keeps a session after signing out ({left})", not left)

    # 7. signing out here forgets the browser's sign-in: the next name asks again
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

    # 8. the stage before: the Worker can be tried and nothing else moved
    TRYING = "http://127.0.0.1:4182"

    def ask(path, method="GET", body=None, headers=None):
        r = urllib.request.Request(
            TRYING + path,
            method=method,
            data=body,
            headers={"host": "home.commonty.test", **(headers or {})},
        )
        try:
            x = urllib.request.build_opener(Stay).open(r)
            return x.status, x.headers, x.read()
        except urllib.error.HTTPError as e:
            return e.code, e.headers, e.read()

    code, _, html = ask("/_dd/login")
    check("trying: the box's own sign-in page as before", code == 200 and b"<" in html)
    code, head, _ = ask("/_dd/sso/start?rd=/")
    check(
        "trying: and the Worker's there for whoever goes to it",
        code in (302, 303) and (head.get("location") or "").startswith(ID + "/go?"),
    )

    check("no page errors", not errors)
    if errors:
        print("\n".join(errors))
    b.close()

if failed:
    print(f"{len(failed)} failed")
    sys.exit(1)
print("all passed")
