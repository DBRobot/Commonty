# The member's app, as far as a browser's pages meet it (app/src/local.rs
# site_pages): the carried pages and the web bundle served on this computer,
# the libraries this device opens answered from here, and everything else
# sent on to the box with the device's own token - /v1/, /running and
# /search to llm.<domain>, the rest of /_dd/ to files.<domain> - exactly the
# split the app makes. What answers behind it is the real box.
#
#   python3 app.py <port> <carried _dd dir> <web bundle> <domain> <state.json>
#
# state.json: {"token": "<device biscuit>", "user": "...", "libraries":
# [{"id", "key"}]}, written by the test from `dd token` and `dd library`.
import json
import os
import ssl
import sys
import urllib.error
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT, CARRIED, BUNDLE, DOMAIN, STATE = sys.argv[1:6]
PORT = int(PORT)
MIME = {
    ".html": "text/html; charset=utf-8",
    ".js": "text/javascript; charset=utf-8",
    ".mjs": "text/javascript; charset=utf-8",
    ".css": "text/css; charset=utf-8",
    ".woff2": "font/woff2",
    ".svg": "image/svg+xml",
    ".wasm": "application/wasm",
    ".json": "application/json",
}
# the app's own page policy (app/tauri.conf.json, local.rs)
CSP = (
    "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; "
    "img-src 'self' data: blob: https:; media-src 'self' blob:; frame-src 'self' blob:; connect-src 'self'"
)
PAGES = {"files", "mail", "calendar", "contacts", "media", "chat", "settings", "home"}
# the box's certificate is the test's own; the app trusts the real one
TLS = ssl.create_default_context(cafile=os.environ.get("SITE_CA") or None)
# what is this hop's, and what would speak for this computer (local.rs forward)
SKIP = {
    "host",
    "connection",
    "cookie",
    "origin",
    "referer",
    "authorization",
    "content-length",
    "transfer-encoding",
    "accept-encoding",
}


def state():
    return json.load(open(STATE))


class H(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *a):
        # what the page asked and what it got, for a journey that fails
        sys.stderr.write(f"{self.command} {self.path[:160]} {fmt % a}\n")

    def send(self, code, body, ctype="application/json", headers=()):
        b = (
            body
            if isinstance(body, bytes)
            else (body if isinstance(body, str) else json.dumps(body)).encode()
        )
        self.send_response(code)
        self.send_header("content-type", ctype)
        self.send_header("content-length", str(len(b)))
        self.send_header("content-security-policy", CSP)
        for k, v in headers:
            self.send_header(k, v)
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(b)

    def file(self, path):
        if not os.path.isfile(path):
            return self.send(404, {"error": "no such file"})
        self.send(
            200,
            open(path, "rb").read(),
            MIME.get(os.path.splitext(path)[1], "application/octet-stream"),
        )

    def go(self):
        path = self.path.split("?")[0]
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n) if n else None
        name = path[len("/_dd/") :]
        if self.command == "GET" and name in PAGES:
            return self.file(f"{CARRIED}/pages/{name}.html")
        if (
            self.command == "GET"
            and path.startswith("/_dd/static/")
            and "/" not in path[12:]
        ):
            return self.file(f"{CARRIED}/static/{path[12:]}")
        if self.command == "GET" and path.startswith("/_dd/web/") and ".." not in path:
            return self.file(f"{BUNDLE}/{path[9:]}")
        s = state()
        libs = s["libraries"]
        if path == "/_dd/app/library":
            return (
                self.send(200, libs[0])
                if libs
                else self.send(404, "no library opens here", "text/plain")
            )
        if path == "/_dd/app/libraries":
            return self.send(
                200,
                [
                    {
                        **lib,
                        "owner": s["user"],
                        "reader": False,
                        "writer": False,
                        "readers": [],
                    }
                    for lib in libs
                ],
            )
        if path.startswith("/_dd/app/"):
            return self.send(404, "in the app's window", "text/plain")
        if path in ("/_dd/login", "/_dd/logout"):
            return self.send(401, "signed out", "text/plain")
        if path.startswith("/v1/") or path in ("/running", "/search"):
            base = f"https://llm.{DOMAIN}"
        elif path.startswith("/_dd/"):
            base = f"https://files.{DOMAIN}"
        else:
            return self.send(404, "not in the app", "text/plain")
        req = urllib.request.Request(base + self.path, data=body, method=self.command)
        for k, v in self.headers.items():
            if k.lower() not in SKIP:
                req.add_header(k, v)
        req.add_header("authorization", "Bearer " + s["token"])
        try:
            r = urllib.request.urlopen(req, context=TLS, timeout=120)
        except urllib.error.HTTPError as e:
            r = e
        data = r.read()
        keep = [
            (k, v)
            for k, v in r.headers.items()
            if k.lower()
            in (
                "etag",
                "content-range",
                "last-modified",
                "dav",
                "x-dd-from",
                "accept-ranges",
            )
        ]
        self.send(
            r.status,
            data,
            r.headers.get("content-type", "application/octet-stream"),
            keep,
        )

    do_GET = do_POST = do_PUT = do_DELETE = do_HEAD = do_PROPFIND = do_MKCOL = (
        do_MOVE
    ) = do_COPY = go
    do_OPTIONS = do_PATCH = go


ThreadingHTTPServer(("127.0.0.1", PORT), H).serve_forever()
