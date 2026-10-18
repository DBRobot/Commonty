# A made-up app and box for the Mail page (servers/mail/web): the page's
# files as the app carries them (app/web/_dd), the app's mail routes with a
# made-up member's device and mail key, and the box's store. Mail arrives
# locked here by an HPKE written apart from the page's (cryptography's
# X25519 and AES-GCM), so the page opening it means the two agree.
import base64, hashlib, hmac, json, os, sys, time, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs, unquote

from cryptography.hazmat.primitives.asymmetric.x25519 import (
    X25519PrivateKey,
    X25519PublicKey,
)
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives import serialization as S

WEB = sys.argv[1]
PORT = int(sys.argv[2])
NAME = "tester"
DOMAIN = "commonty.test"

b64u = lambda b: base64.urlsafe_b64encode(b).rstrip(b"=").decode()
unb64u = lambda s: base64.urlsafe_b64decode(s + "=" * (-len(s) % 4))


def ext(salt, ikm):
    return hmac.new(salt or bytes(32), ikm, hashlib.sha256).digest()


def exp(prk, info, n):
    out, t, i = b"", b"", 1
    while len(out) < n:
        t = hmac.new(prk, t + info + bytes([i]), hashlib.sha256).digest()
        out += t
        i += 1
    return out[:n]


KEM = b"KEM" + (0x20).to_bytes(2, "big")
SUITE = (
    b"HPKE"
    + (0x20).to_bytes(2, "big")
    + (1).to_bytes(2, "big")
    + (2).to_bytes(2, "big")
)
lx = lambda suite, salt, label, ikm: ext(salt, b"HPKE-v1" + suite + label + ikm)
le = lambda suite, prk, label, info, n: exp(
    prk, n.to_bytes(2, "big") + b"HPKE-v1" + suite + label + info, n
)
raw = lambda k: k.public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)


def derive(ikm):
    sk = le(KEM, lx(KEM, b"", b"dkp_prk", ikm), b"sk", b"", 32)
    priv = X25519PrivateKey.from_private_bytes(sk)
    return priv, raw(priv.public_key())


def seal(pk, info, pt):
    e = X25519PrivateKey.generate()
    enc = raw(e.public_key())
    dh = e.exchange(X25519PublicKey.from_public_bytes(pk))
    ss = le(KEM, lx(KEM, b"", b"eae_prk", dh), b"shared_secret", enc + pk, 32)
    ctx = (
        b"\0"
        + lx(SUITE, b"", b"psk_id_hash", b"")
        + lx(SUITE, b"", b"info_hash", info.encode())
    )
    secret = lx(SUITE, ss, b"secret", b"")
    key, nonce = (
        le(SUITE, secret, b"key", ctx, 32),
        le(SUITE, secret, b"base_nonce", ctx, 12),
    )
    return b"CML1" + enc + AESGCM(key).encrypt(nonce, pt, b"")


WORKER = Ed25519PrivateKey.generate()
WORKER_PUB = base64.b64encode(raw(WORKER.public_key())).decode()
sha = lambda b: b64u(hashlib.sha256(b).digest())


def lock(pk, kind, id_, summary, msg):
    """as the mail Worker locks and signs mail (client/mail/worker/inbox.js, mail-object.js)"""
    pre = "commonty mail v1" if kind == "in" else f"commonty mail v1 {kind}"
    s = seal(pk, f"{pre} summary {id_}", json.dumps(summary).encode())
    r = seal(pk, f"{pre} raw {id_}", msg)
    h = {
        "kind": kind,
        "id": id_,
        "hs": sha(s),
        "hr": sha(r),
        "by": "worker",
        "kid": "w1",
    }
    h["sig"] = base64.b64encode(
        WORKER.sign(
            f"commonty mail object v1\0{kind}\0{id_}\0{h['hs']}\0{h['hr']}".encode()
        )
    ).decode()
    hb = json.dumps(h).encode()
    return b"CMA1" + len(hb).to_bytes(4, "big") + hb + len(s).to_bytes(4, "big") + s + r


def new_id():
    t = int(time.time() * 1000)
    s = ""
    while t:
        s = "0123456789abcdefghijklmnopqrstuvwxyz"[t % 36] + s
        t //= 36
    return (
        s.rjust(9, "0")
        + b64u(os.urandom(9)).lower().replace("-", "a").replace("_", "b")[:12]
    )


state = {
    "ikm": None,
    "registered": False,
    "objs": {},  # "name/kind/id" -> {"bytes", "from"}
    "sent": [],
    "dev": Ed25519PrivateKey.generate(),
    "lock": threading.Lock(),
}
DEV_FP = "a1b2c3d4e5f6"


def pk_of(name):
    priv, pk = derive(state["ikm"])
    return pk


MIME = {
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".woff2": "font/woff2",
    ".svg": "image/svg+xml",
    ".wasm": "application/wasm",
}


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send(self, body, code=200, ctype="application/json", headers=None):
        b = (
            body
            if isinstance(body, bytes)
            else (
                json.dumps(body).encode()
                if not isinstance(body, str)
                else body.encode()
            )
        )
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(b)))
        # the app's own page policy (app/tauri.conf.json, local.rs), no inline script
        self.send_header(
            "content-security-policy",
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob: https:; frame-src 'self' blob:; connect-src 'self'",
        )
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(b)

    def body(self):
        n = int(self.headers.get("content-length") or 0)
        return self.rfile.read(n) if n else b""

    def file(self, rel):
        p = os.path.join(WEB, rel)
        if not os.path.isfile(p):
            return self.send({"error": "no"}, 404)
        data = open(p, "rb").read()
        self.send(
            data, 200, MIME.get(os.path.splitext(p)[1], "application/octet-stream")
        )

    def do_GET(self):
        u = urlparse(self.path)
        p, q = u.path, parse_qs(u.query)
        if p == "/_dd/mail":
            return self.file("pages/mail.html")
        if p.startswith("/_dd/static/"):
            return self.file("static/" + p[len("/_dd/static/") :])
        if p == "/_dd/me":
            return self.send(
                {
                    "user": NAME,
                    "role": "member",
                    "home": f"https://home.{DOMAIN}/",
                    "services": [],
                }
            )
        if p == "/_dd/app/mail/key":
            if not state["ikm"]:
                return self.send("no mail key yet", 404, "text/plain")
            return self.send(self.keydoc())
        if p.startswith("/_dd/mail/list/"):
            kind = p.split("/")[-1]
            after = q.get("after", [""])[0]
            pre = f"{NAME}/{kind}/"
            with state["lock"]:
                ids = sorted(k[len(pre) :] for k in state["objs"] if k.startswith(pre))
            return self.send(
                [
                    {"id": i, "size": len(state["objs"][pre + i]["bytes"])}
                    for i in ids
                    if i > after
                ][:1000]
            )
        if p.startswith("/_dd/mail/o/"):
            _, _, _, _, kind, id_ = p.split("/")
            o = state["objs"].get(f"{NAME}/{kind}/{id_}")
            if not o:
                return self.send({"error": "no"}, 404)
            b = o["bytes"]
            rng = self.headers.get("range")
            h = {"x-dd-from": o["from"]} if o.get("from") else {}
            if rng:
                a, z = rng.split("=")[1].split("-")
                part = b[int(a) : int(z) + 1]
                h["content-range"] = f"bytes {a}-{int(a) + len(part) - 1}/{len(b)}"
                return self.send(part, 206, "application/octet-stream", h)
            return self.send(b, 200, "application/octet-stream", h)
        if p == "/test/sent":
            return self.send(state["sent"])
        if p == "/test/objs":
            return self.send(sorted(state["objs"]))
        self.send({"error": "not here"}, 404)

    def keydoc(self):
        pk = b64u(pk_of(NAME))
        sig = base64.b64encode(
            state["dev"].sign(f"commonty mail key v1\0{NAME}\0{pk}".encode())
        ).decode()
        return {
            "ikm": b64u(state["ikm"]),
            "pk": pk,
            "sig": sig,
            "device": {
                "fp": DEV_FP,
                "key": base64.b64encode(raw(state["dev"].public_key())).decode(),
            },
            "name": NAME,
        }

    def do_PUT(self):
        p = urlparse(self.path).path
        if p.startswith("/_dd/mail/o/"):
            _, _, _, _, kind, id_ = p.split("/")
            with state["lock"]:
                state["objs"][f"{NAME}/{kind}/{id_}"] = {"bytes": self.body()}
            return self.send(b"", 204, "text/plain")
        self.send({"error": "not here"}, 404)

    def do_DELETE(self):
        p = urlparse(self.path).path
        if p.startswith("/_dd/mail/o/"):
            _, _, _, _, kind, id_ = p.split("/")
            with state["lock"]:
                state["objs"].pop(f"{NAME}/{kind}/{id_}", None)
            return self.send(b"", 204, "text/plain")
        self.send({"error": "not here"}, 404)

    def do_POST(self):
        p = urlparse(self.path).path
        b = self.body()
        if p == "/_dd/app/mail/key":
            if not state["ikm"]:
                state["ikm"] = os.urandom(32)
            return self.send(self.keydoc())
        if p == "/_dd/app/mail/worker":
            v = json.loads(b or b"{}")
            path = v.get("path", "")
            if path == "/api/addresses":
                return self.send(
                    {
                        "address": f"{NAME}@{DOMAIN}",
                        "aliases": [
                            {
                                "address": f"shop.otter.2026@{DOMAIN}",
                                "label": "Hardware shop",
                            }
                        ],
                        "fwd": True,
                    }
                )
            if path.startswith("/api/mailkey/"):
                who = unquote(path.split("/")[-1])
                if who != NAME or not state["registered"]:
                    return self.send("they have no inbox", 404, "text/plain")
                return self.send(
                    {
                        "name": NAME,
                        "pk": b64u(pk_of(NAME)),
                        "devices": [
                            {
                                "fp": DEV_FP,
                                "key": base64.b64encode(
                                    raw(state["dev"].public_key())
                                ).decode(),
                            }
                        ],
                        "worker": [{"kid": "w1", "pub": WORKER_PUB}],
                    }
                )
            return self.send("not a question", 400, "text/plain")
        if p == "/_dd/app/mail/vouch":
            v = json.loads(b)
            msg = f"commonty mail object v1\0in\0{v['id']}\0{v['hs']}\0{v['hr']}\0{NAME}\0{DEV_FP}".encode()
            return self.send(
                {
                    "name": NAME,
                    "fp": DEV_FP,
                    "sig": base64.b64encode(state["dev"].sign(msg)).decode(),
                }
            )
        if p == "/_dd/app/mail/send":
            v = json.loads(b)
            state["sent"].append(v)
            return self.send({"sent": [{"to": t, "ok": True} for t in v["to"]]})
        if p.startswith("/_dd/mail/deliver/"):
            _, _, _, _, to, id_ = p.split("/")
            with state["lock"]:
                state["objs"][f"{to}/in/{id_}"] = {"bytes": b, "from": NAME}
            return self.send(b"", 204, "text/plain")
        if p == "/test/register":
            state["registered"] = True
            return self.send({"ok": True})
        if p == "/test/arrive":
            v = json.loads(b)
            msg = v["raw"].encode()
            id_ = new_id()
            summ = {
                **v["summary"],
                "id": id_,
                "at": int(time.time() * 1000),
                "auth": v.get("auth", {}),
            }
            with state["lock"]:
                state["objs"][f"{NAME}/in/{id_}"] = {
                    "bytes": lock(pk_of(NAME), "in", id_, summ, msg)
                }
            time.sleep(0.002)
            return self.send({"id": id_})
        self.send({"error": "not here"}, 404)


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
