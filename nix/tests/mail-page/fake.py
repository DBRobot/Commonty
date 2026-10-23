# A made-up app and box for the Mail page (servers/mail/web): the page's
# files as the app carries them (app/web/_dd), the app's mail routes with a
# made-up member's device and mail key, and the box's store. Mail arrives
# locked here by an HPKE written apart from the page's (cryptography's
# X25519 and AES-GCM), so the page opening it means the two agree.
import base64, hashlib, hmac, json, os, sys, time, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import urlparse, parse_qs, unquote, quote

from cryptography.hazmat.primitives.asymmetric.x25519 import (
    X25519PrivateKey,
    X25519PublicKey,
)
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives import serialization as S

WEB = sys.argv[1]
PORT = int(sys.argv[2])
# the web bundle the app carries beside the pages (flake.nix webDist): the
# library's wasm and the PDF editor's MuPDF
BUNDLE = sys.argv[3] if len(sys.argv) > 3 else None
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
    # what each ask of /_dd/app/keys named
    "keys": [],
    "registered": False,
    "objs": {},  # "name/kind/id" -> {"bytes", "from"}
    "sent": [],
    "dev": Ed25519PrivateKey.generate(),
    "lock": threading.Lock(),
    # the member's Files library: what the gate keeps of it, by encrypted
    # path, exactly as the page sent it
    "lib": {
        "id": os.urandom(16).hex(),
        "key": base64.b64encode(os.urandom(32)).decode(),
    },
    "dav": {},
    "dirs": set(),
    "uploads": {},
    # every request, as (method, path): what the page asked of the box
    "log": [],
    # the box's transcode (Movies & TV): its key, and each session asked
    # for, by id, with whether it was answered and whether it was ended
    "box": X25519PrivateKey.generate(),
    "sessions": {},
}
# how long the box takes to pack a film's first piece
BOX_SECS = 2.0
PLAYLIST = """#EXTM3U
#EXT-X-VERSION:3
#EXT-X-TARGETDURATION:4
#EXT-X-MEDIA-SEQUENCE:0
#EXTINF:4.0,
00000.ts
#EXT-X-ENDLIST
"""
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
    ".ttf": "font/ttf",
    ".gz": "application/gzip",
}


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def parse_request(self):
        good = super().parse_request()
        if good and not self.path.startswith("/test/"):
            state["log"].append([self.command, unquote(self.path)])
        return good

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

    def file(self, rel, root=None):
        p = os.path.join(root or WEB, rel)
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
        if p == "/_dd/files":
            return self.file("pages/files.html")
        if p == "/_dd/media":
            return self.file("pages/media.html")
        if p == "/_dd/transcode/key":
            pub = (
                state["box"]
                .public_key()
                .public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)
            )
            return self.send({"key": b64u(pub)})
        if p.startswith("/_dd/transcode/session/"):
            id_, file = p[len("/_dd/transcode/session/") :].split("/", 1)
            s = state["sessions"].get(id_)
            if not s or s["ended"]:
                return self.send("no such session", 404, "text/plain")
            if file == "index.m3u8":
                return self.send(PLAYLIST, 200, "application/vnd.apple.mpegurl")
            # a piece no browser can play: the test is of when it is asked for
            return self.send(b"\x47" + bytes(187), 200, "video/mp2t")
        if p == "/_dd/app/open":
            # the app opens the address in the device's browser: nothing to show
            return self.send(b"", 204, "text/plain")
        if p == "/test/sessions":
            return self.send(state["sessions"])
        if p.startswith("/_dd/static/"):
            return self.file("static/" + p[len("/_dd/static/") :])
        if p.startswith("/_dd/web/") and BUNDLE:
            return self.file(p[len("/_dd/web/") :], BUNDLE)
        if p == "/_dd/app/library":
            return self.send(state["lib"])
        if p.startswith("/_dd/dav/"):
            b = state["dav"].get(self.dav_path())
            if b is None:
                return self.send({"error": "no"}, 404)
            if self.headers.get("x-dd-presign"):
                return self.send(
                    {"url": f"http://127.0.0.1:{PORT}/s3/{self.dav_path()}"}
                )
            rng = self.headers.get("range")
            if rng:
                # a range of the sealed file, as the gate serves it (library.js pieces)
                a, z = rng.split("=")[1].split("-")
                part = b[int(a) : int(z) + 1]
                return self.send(
                    part,
                    206,
                    "application/octet-stream",
                    {"content-range": f"bytes {a}-{int(a) + len(part) - 1}/{len(b)}"},
                )
            return self.send(b, 200, "application/octet-stream")
        if p == "/test/log":
            return self.send(state["log"])
        if p == "/test/dav":
            return self.send(
                [
                    {
                        "path": k,
                        "size": len(v),
                        "sealed": b"%PDF" not in v and b"hello" not in v,
                    }
                    for k, v in sorted(state["dav"].items())
                ]
            )
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
        if p == "/test/keys":
            return self.send(state["keys"])
        if p == "/test/objs":
            return self.send(sorted(state["objs"]))
        self.send({"error": "not here"}, 404)

    def dav_path(self):
        # /_dd/dav/<lib>/<encrypted path, a segment at a time>
        rest = urlparse(self.path).path[len("/_dd/dav/") :].split("/", 1)
        assert rest[0] == state["lib"]["id"], "another library"
        return "/".join(
            unquote(x) for x in (rest[1] if len(rest) > 1 else "").split("/") if x
        )

    def do_PROPFIND(self):
        d = self.dav_path()
        pre = d + "/" if d else ""
        kids = {}
        for k, v in state["dav"].items():
            if k.startswith(pre) and k != d:
                head = k[len(pre) :].split("/")[0]
                kids[head] = None if "/" in k[len(pre) :] else len(v)
        for k in state["dirs"]:
            if k.startswith(pre) and k != d and "/" not in k[len(pre) :]:
                kids.setdefault(k[len(pre) :], None)
        if d and d not in state["dirs"] and not kids:
            return self.send({"error": "no"}, 404)
        base = f"/_dd/dav/{state['lib']['id']}/"
        enc = lambda x: "/".join(quote(seg, safe="") for seg in x.split("/"))
        rows = [
            f"<D:response><D:href>{base}{enc(d)}/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response>"
        ]
        for name, size in sorted(kids.items()):
            href = base + enc(pre + name)
            if size is None:
                rows.append(
                    f"<D:response><D:href>{href}/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response>"
                )
            else:
                rows.append(
                    f"<D:response><D:href>{href}</D:href><D:propstat><D:prop><D:resourcetype/><D:getcontentlength>{size}</D:getcontentlength><D:getlastmodified>Sun, 18 Oct 2026 09:41:00 GMT</D:getlastmodified></D:prop></D:propstat></D:response>"
                )
        xml = (
            '<?xml version="1.0"?><D:multistatus xmlns:D="DAV:">'
            + "".join(rows)
            + "</D:multistatus>"
        )
        self.send(xml, 207, "application/xml")

    def do_MKCOL(self):
        d = self.dav_path()
        if d in state["dirs"]:
            return self.send(b"", 405, "text/plain")
        state["dirs"].add(d)
        self.send(b"", 201, "text/plain")

    def do_MOVE(self):
        a = self.dav_path()
        dest = urlparse(self.headers["destination"]).path
        b = "/".join(
            unquote(x)
            for x in dest[len(f"/_dd/dav/{state['lib']['id']}/") :].split("/")
            if x
        )
        if b in state["dav"]:
            return self.send(b"", 412, "text/plain")
        state["dav"][b] = state["dav"].pop(a)
        self.send(b"", 201, "text/plain")

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
        if p.startswith("/_dd/dav/"):
            up = self.headers.get("x-dd-upload")
            if up:
                part = int(self.headers["x-dd-part"])
                state["uploads"][up][part] = self.body()
                return self.send({"etag": f"e{part}"})
            state["dav"][self.dav_path()] = self.body()
            return self.send(b"", 201, "text/plain")
        if p.startswith("/_dd/mail/o/"):
            _, _, _, _, kind, id_ = p.split("/")
            with state["lock"]:
                state["objs"][f"{NAME}/{kind}/{id_}"] = {"bytes": self.body()}
            return self.send(b"", 204, "text/plain")
        self.send({"error": "not here"}, 404)

    def do_DELETE(self):
        p = urlparse(self.path).path
        if p.startswith("/_dd/transcode/session/"):
            s = state["sessions"].get(p.rsplit("/", 1)[1])
            if s:
                s["ended"] = time.time()
            return self.send(b"", 204, "text/plain")
        if p.startswith("/_dd/dav/"):
            state["dav"].pop(self.dav_path(), None)
            return self.send(b"", 204, "text/plain")
        if p.startswith("/_dd/mail/o/"):
            _, _, _, _, kind, id_ = p.split("/")
            with state["lock"]:
                state["objs"].pop(f"{NAME}/{kind}/{id_}", None)
            return self.send(b"", 204, "text/plain")
        self.send({"error": "not here"}, 404)

    def do_POST(self):
        p = urlparse(self.path).path
        b = self.body()
        if p.startswith("/_dd/dav/"):
            # a big file in pieces (library.js put): start, parts, then the end
            if self.headers.get("x-dd-upload") == "start":
                up = os.urandom(8).hex()
                state["uploads"][up] = {}
                return self.send({"upload": up})
            up = self.headers.get("x-dd-upload")
            parts = state["uploads"].pop(up)
            state["dav"][self.dav_path()] = b"".join(parts[k] for k in sorted(parts))
            return self.send(b"", 200, "text/plain")
        if p == "/_dd/transcode/session":
            v = json.loads(b)
            id_ = os.urandom(8).hex()
            s = {
                "asked": time.time(),
                "from": v["from"],
                "answered": None,
                "ended": None,
            }
            # one at a time per member, as the box: a new one ends the rest
            for o in state["sessions"].values():
                if not o["ended"]:
                    o["ended"] = time.time()
            state["sessions"][id_] = s
            time.sleep(BOX_SECS)  # sleep-ok: the box packing the first piece
            s["answered"] = time.time()
            return self.send(
                {
                    "id": id_,
                    "playlist": f"/session/{id_}/index.m3u8",
                    "from": v["from"],
                    "duration": 5400,
                }
            )
        if p == "/_dd/app/mail/key":
            if not state["ikm"]:
                state["ikm"] = os.urandom(32)
            return self.send(self.keydoc())
        if p == "/_dd/app/keys":
            # the app's one key route (app/src/keys.rs), on a device that has
            # never opened Photos
            asked = json.loads(b)["services"]
            state["keys"].append(asked)
            return self.send(
                {
                    s: {
                        "error": "Photos does not open on this device yet: open Photos once in the app"
                    }
                    for s in asked
                }
            )
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
