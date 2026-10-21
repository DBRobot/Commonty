# The gateway box's front, run in the build with no vm: node1's own nginx
# config and the real gate (verify) with the pages and the web bundle the
# release carries, and a stand-in for every other service nginx sends to.
# A stand-in answers 200 with where it is and writes down what reached it,
# so a test sees nginx's and the gate's answer exactly as a browser would,
# and what a backend was handed. People are in people.py.
#
#     from stack import Stack
#     with Stack(tmp, conf, nginx, verify, env_json) as s:
#         s.get("home", "/_dd/me", who="mia")          # urllib, no redirects
#         s.browser_context(browser, who="mia")         # playwright, signed in
#
# Every name answers on HOST: the inside listener (443 -> 10443) as
# from the fleet network, and the tunnel's (8443 -> 18443) as an outsider.
import http.client
import json
import os
import re
import socket
import socketserver
import ssl
import subprocess
import threading
import tempfile
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import people

# one loopback address of its own, so a run beside other servers on the
# machine (or another run) shares no port with them
# (SITE_HOST: two runs on one machine each take their own)
HOST = os.environ.get("SITE_HOST") or "127.0.0.77"
GATE = 4181
# every port nginx listens on, moved up by this much
OFFSET = int(os.environ.get("SITE_PORT_OFFSET") or 10000)
INSIDE = 443 + OFFSET
TUNNEL = 8443 + OFFSET


def wait_for(what, check, timeout=60):
    """until check() is true; a real condition, never a fixed sleep"""
    end = time.monotonic() + timeout
    last = None
    while time.monotonic() < end:
        try:
            if check():
                return
        except Exception as e:  # not up yet
            last = e
        time.sleep(
            0.05
        )  # sleep-ok: polling check(), the condition itself, until it holds
    raise AssertionError(f"{what}: not within {timeout}s ({last})")


class Seen:
    """what reached the stand-ins: (upstream, method, host, path, headers)"""

    def __init__(self):
        self.rows = []
        self.lock = threading.Lock()

    def add(self, row):
        with self.lock:
            self.rows.append(row)

    def since(self, n):
        with self.lock:
            return list(self.rows[n:])

    def __len__(self):
        with self.lock:
            return len(self.rows)


def stand_in(name, seen, answers=None):
    class H(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *a):
            pass

        def address_string(self):
            return name

        def go(self):
            n = int(self.headers.get("content-length") or 0)
            if n:
                self.rfile.read(n)
            seen.add(
                (
                    name,
                    self.command,
                    self.headers.get("host", ""),
                    self.path,
                    {k.lower(): v for k, v in self.headers.items()},
                )
            )
            path = self.path.split("?")[0]
            got = next(
                (
                    v
                    for k, v in (answers or {}).items()
                    if k == path or (k.endswith("*") and path.startswith(k[:-1]))
                ),
                None,
            )
            ctype, body = got if got else ("text/plain", f"stand-in {name} {self.path}")
            b = body.encode() if isinstance(body, str) else body
            self.send_response(200)
            self.send_header("content-type", ctype)
            self.send_header("content-length", str(len(b)))
            self.end_headers()
            if self.command != "HEAD":
                self.wfile.write(b)

        do_GET = do_POST = do_PUT = do_DELETE = do_HEAD = do_PROPFIND = do_OPTIONS = go
        do_PATCH = do_MKCOL = do_MOVE = go

    return H


class UnixHTTP(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True
    request_queue_size = 256


class TCPHTTP(ThreadingHTTPServer):
    # a page asks dozens at once; the default backlog of 5 drops the rest,
    # and the browser's retry a second later is not the page's own time
    request_queue_size = 256


class Control(BaseHTTPRequestHandler):
    """dd-control as the gate asks it: the agreed identity state, and a
    directory log with nothing new"""

    state = {}

    def log_message(self, *a):
        pass

    def do_GET(self):
        if self.path.startswith("/identity"):
            b = json.dumps(Control.state).encode()
        elif self.path.startswith("/state"):
            b = json.dumps(people.control_state()).encode()
        elif self.path.startswith("/directory"):
            since = re.search(r"since=(\d+)", self.path)
            b = json.dumps(
                {"items": [], "next": int(since.group(1)) if since else 0}
            ).encode()
        else:
            self.send_response(404)
            self.send_header("content-length", "0")
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)


def rewrite(conf, tmp, sockdir):
    """node1's nginx.conf, made to run here: a test certificate, files under
    tmp, every port +10000, and each unix socket upstream under tmp"""
    s = open(conf).read()
    s = re.sub(r"/var/lib/acme/[^/]+/(fullchain|chain)\.pem", tmp + "/cert.pem", s)
    s = re.sub(r"/var/lib/acme/[^/]+/key\.pem", tmp + "/key.pem", s)
    s = re.sub(r"include\s+/(run|var|etc)/[^;]*;", "include " + tmp + "/empty.conf;", s)
    s = re.sub(r"/var/(log|cache|spool)/nginx", tmp, s)
    s = re.sub(r"^(\s*)pid\s+[^;]*;", r"\1pid " + tmp + "/nginx.pid;", s, flags=re.M)
    s = re.sub(r"^(\s*)user\s+[^;]*;", "", s, flags=re.M)
    s = re.sub(
        r"^(\s*)http\s*\{",
        lambda m: (
            m.group(0)
            + f" access_log off; client_body_temp_path {tmp}/body; proxy_temp_path {tmp}/proxy;"
            # a redirect names the port the browser asked on (443), not ours
            + " port_in_redirect off;"
        ),
        s,
        count=1,
        flags=re.M,
    )
    s = re.sub(r"^\s*listen\s+\[::0?\]:\d+[^;]*;", "", s, flags=re.M)
    s = re.sub(
        r"(listen\s+)(?:0\.0\.0\.0|127\.0\.0\.1):(\d+)",
        lambda m: f"{m.group(1)}{HOST}:{int(m.group(2)) + OFFSET}",
        s,
    )
    s = s.replace("127.0.0.1:", HOST + ":")
    s = re.sub(r"^(\s*)8443 1;", lambda m: f"{m.group(1)}{TUNNEL} 1;", s, flags=re.M)
    # flood limits count requests per credential and address; a test asks
    # hundreds of times a second from one address, so here they are off
    # (they are nginx -t'd in nix/tests/nginx.nix and not under test here)
    s = re.sub(r"^\s*limit_(req|conn)\s[^;]*;", "", s, flags=re.M)
    socks = sorted(set(re.findall(r"unix:(/run/[^:;\s]+\.sock)", s)))
    for p in socks:
        s = s.replace("unix:" + p, "unix:" + sockdir + "/" + os.path.basename(p))
    # the box's resolver is not here; nothing in it resolves a name but
    # localhost (the museum upstream)
    s = s.replace("server localhost:", f"server {HOST}:")
    h = re.escape(HOST)
    ports = sorted(
        {int(p) for p in re.findall(rf"proxy_pass https?://{h}:(\d+)", s)}
        | {int(p) for p in re.findall(rf"server {h}:(\d+)", s)}
    )
    hosts = sorted(
        {
            h
            for line in re.findall(r"^\s*server_name\s+([^;]*);", s, flags=re.M)
            for h in line.split()
            if h != "_"
        }
    )
    return (
        s,
        hosts,
        [p for p in ports if p != GATE],
        [sockdir + "/" + os.path.basename(p) for p in socks],
    )


def cert(tmp, domain):
    from cryptography import x509
    from cryptography.hazmat.primitives import hashes, serialization
    from cryptography.hazmat.primitives.asymmetric import ec
    from cryptography.x509.oid import NameOID
    import datetime

    key = ec.generate_private_key(ec.SECP256R1())
    name = x509.Name([x509.NameAttribute(NameOID.COMMON_NAME, domain)])
    now = datetime.datetime.now(datetime.timezone.utc)
    c = (
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
                    x509.DNSName("*." + domain),
                    x509.DNSName(domain),
                    x509.DNSName("*.node1." + domain),
                ]
            ),
            critical=False,
        )
        .sign(key, hashes.SHA256())
    )
    open(tmp + "/cert.pem", "wb").write(c.public_bytes(serialization.Encoding.PEM))
    open(tmp + "/key.pem", "wb").write(
        key.private_bytes(
            serialization.Encoding.PEM,
            serialization.PrivateFormat.PKCS8,
            serialization.NoEncryption(),
        )
    )


# values that name a place without the gate going there: the names it
# writes into pages and tokens
KEEP_ORIGIN = {
    # the home tiles: links on a page
    "VERIFY_HOME",
    "VERIFY_OIDC_ISSUER",
    "VERIFY_OIDC_REDIRECT",
    "VERIFY_HOUSE_ORIGINS",
    "VERIFY_HEADSCALE_URL",
}

# what the services the gate itself asks answer, as much as the gate reads
GATE_ASKS = {
    # SearXNG, for Chat's web search
    "VERIFY_SEARCH": {"/search": ("application/json", '{"results": []}')},
    # Pi-hole's API, for ad blocking
    "VERIFY_ADBLOCK": {
        "/api/auth": (
            "application/json",
            '{"session": {"valid": true, "sid": "site-test", "validity": 300}}',
        ),
        "/api/*": ("application/json", "{}"),
    },
}


# the services nginx sends to, as much as our pages read of them: an empty
# forge and an empty Prometheus. Anything else gets the plain stand-in line.
EMPTY_PROM = (
    "application/json",
    '{"status": "success", "data": {"resultType": "vector", "result": []}}',
)
NGINX_ASKS = {
    "forgejo.sock": {
        "/api/v1/user": (
            "application/json",
            '{"id": 1, "login": "site", "full_name": "", "avatar_url": ""}',
        ),
        "/api/v1/notifications/new": ("application/json", '{"new": 0}'),
        "/api/v1/version": ("application/json", '{"version": "site-test"}'),
        "/api/v1/repos/search": ("application/json", '{"ok": true, "data": []}'),
        "/api/v1/*": ("application/json", "[]"),
    },
    10903: {"/api/v1/*": EMPTY_PROM},
    10913: {"/api/v1/*": EMPTY_PROM},
}


class Response:
    def __init__(self, status, headers, body):
        self.status = status
        self.headers = headers  # [(name lowercased, value)], in order
        self.body = body

    def header(self, name):
        v = [b for a, b in self.headers if a == name.lower()]
        return v[0] if v else None

    def all(self, name):
        return [b for a, b in self.headers if a == name.lower()]

    def __repr__(self):
        return f"<{self.status} {self.header('location') or ''} {self.body[:60]!r}>"


class Stack:
    def __init__(self, tmp, conf, nginx, verify, env_json, answers=None):
        self.tmp, self.conf, self.nginx, self.verify = tmp, conf, nginx, verify
        self.env = json.load(open(env_json))
        self.domain = self.env["VERIFY_DOMAIN"]
        self.seen = Seen()
        self.answers = answers or {}
        self.procs, self.servers = [], []
        self.gate = self.ngx = None

    # --- up and down

    def __enter__(self):
        try:
            return self._up()
        except BaseException:
            self.__exit__()
            raise

    def _up(self):
        t = self.tmp
        os.makedirs(t, exist_ok=True)
        for d in ("body", "proxy", "gate/keys"):
            os.makedirs(f"{t}/{d}", exist_ok=True)
        open(t + "/empty.conf", "w").close()
        cert(t, self.domain)
        # a socket's path has room for 107 bytes: a short folder of its own
        self.sockdir = tempfile.mkdtemp(prefix="site-")
        conf, self.hosts, ports, socks = rewrite(self.conf, t, self.sockdir)
        open(t + "/nginx.conf", "w").write(conf)
        # every other place the gate would reach (the bucket, Photos' API, the
        # mail Worker, Headscale's name, the other boxes): a stand-in here,
        # so a run never asks the live fleet
        self.env = self.outside_to_stand_ins(self.env)
        # the box's own services the gate asks, beside those nginx sends to
        asks = {}
        for k, v in self.env.items():
            for port in (
                re.findall(r"https?://127\.0\.0\.1:(\d+)", str(v))
                if k.startswith("VERIFY_")
                else []
            ):
                if int(port) != GATE:
                    asks.setdefault(int(port), {}).update(GATE_ASKS.get(k, {}))
        for p, a in list(asks.items()) + list(NGINX_ASKS.items()):
            self.answers.setdefault(p, {}).update(a)
        ports = sorted(set(ports) | set(asks))
        for p in ports:
            self.serve(
                TCPHTTP(
                    (HOST, p),
                    stand_in(f"127.0.0.1:{p}", self.seen, self.answers.get(p)),
                )
            )
        for p in socks:
            self.serve(
                UnixHTTP(
                    p,
                    stand_in(
                        os.path.basename(p),
                        self.seen,
                        self.answers.get(os.path.basename(p)),
                    ),
                )
            )
            os.chmod(p, 0o666)
        Control.state = people.agreed_state()
        control = TCPHTTP((HOST, 0), Control)
        self.serve(control)
        self.gate_env = self.gate_environment(control.server_address[1])
        people.write_gate_state(t + "/gate", self.domain)
        self.gate = subprocess.Popen(
            [self.verify + "/bin/verify"],
            env=self.gate_env,
            stdout=open(t + "/gate.log", "w"),
            stderr=subprocess.STDOUT,
        )
        self.procs.append(self.gate)
        self.ngx = subprocess.Popen(
            [self.nginx, "-e", "stderr", "-p", t, "-c", t + "/nginx.conf"],
            stdout=open(t + "/nginx.log", "w"),
            stderr=subprocess.STDOUT,
        )
        self.procs.append(self.ngx)
        wait_for(
            "the gate",
            lambda: self._alive() and http_ok(f"http://{HOST}:{GATE}/health"),
        )
        wait_for(
            "nginx",
            lambda: (
                self._alive() and self.get("home", "/_dd/static/bar.css").status == 200
            ),
        )
        # the agreed state read: a member is let in (ident.rs follows it in
        # the background, and nobody is in until it has)
        wait_for(
            "the agreed state",
            lambda: (
                self._alive() and self.get("home", "/_dd/me", who="mia").status == 200
            ),
        )
        return self

    def __exit__(self, *a):
        for p in self.procs:
            if p.poll() is None:
                p.terminate()
        for p in self.procs:
            try:
                p.wait(timeout=10)
            except subprocess.TimeoutExpired:
                p.kill()
        for s in self.servers:
            s.shutdown()

    def _alive(self):
        for p, what in ((self.gate, "gate"), (self.ngx, "nginx")):
            if p is not None and p.poll() is not None:
                raise SystemExit(f"{what} exited {p.returncode}:\n" + self.log(what))
        return True

    def log(self, what):
        try:
            return open(f"{self.tmp}/{what}.log").read()[-4000:]
        except OSError:
            return ""

    def outside_to_stand_ins(self, env):
        out, ports = {}, {}
        for k, v in env.items():
            if not k.startswith("VERIFY_") or not isinstance(v, str):
                out[k] = v
                continue

            def to_here(m):
                origin = m.group(1)
                if origin.startswith("127.0.0.1:") or k in KEEP_ORIGIN:
                    return m.group(0)
                port = ports.setdefault(origin, 40000 + len(ports))
                return f"http://127.0.0.1:{port}"

            v = re.sub(r"https?://([A-Za-z0-9.:-]+)", to_here, v)
            # a box's fleet address (VERIFY_FLEET, VERIFY_NAMES) is this one
            v = re.sub(
                r"\b100\.(6[4-9]|[7-9][0-9]|1[01][0-9]|12[0-7])\.\d+\.\d+\b", HOST, v
            )
            out[k] = v
        self.outside = ports
        return out

    def serve(self, srv):
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        self.servers.append(srv)

    def gate_environment(self, control_port):
        t = self.tmp
        # the box's own services, at this run's address
        e = {
            k: v.replace("127.0.0.1:", HOST + ":")
            for k, v in self.env.items()
            if k.startswith("VERIFY_")
        }
        # files the box keeps at run time, here empty or made up; the
        # configuration itself is node1's
        for k in [k for k in e if k.endswith("_FILE")]:
            open(f"{t}/gate/{k.lower()}", "w").write("test-" + k.lower() + "\n")
            os.chmod(f"{t}/gate/{k.lower()}", 0o600)
            e[k] = f"{t}/gate/{k.lower()}"
        cfg = {"boxes": {"node1": {}}, "validators": [{"name": "node1"}]}
        open(t + "/gate/control.json", "w").write(json.dumps(cfg))
        e.update(
            {
                "PATH": os.environ.get("PATH", ""),
                # the bucket's key (library-key.env on a box): the stand-in takes any
                "LIBRARY_ID": "GK0123456789abcdef01234567",
                "LIBRARY_SECRET": "0" * 64,
                "HOME": t,
                "VERIFY_BIND": f"{HOST}:{GATE}",
                "VERIFY_DIR": t + "/gate/keys",
                "VERIFY_ADBLOCK_STATE": t + "/gate/adblock",
                "VERIFY_STORAGE_LEDGER": t + "/gate/ledger.json",
                "VERIFY_HOUSE": t + "/gate/house",
                "VERIFY_ENDED": t + "/gate/ended",
                "VERIFY_HOUSE_PEERS": "",
                "VERIFY_FORGE_EVENTS": "",
                "VERIFY_MEMBER_LIST": "",
                "VERIFY_APP_MANIFEST": "",
                "VERIFY_MAIL_SIGN": t + "/gate/mail-sign.sock",
                "VERIFY_CONTROL": f"http://{HOST}:{control_port}",
                "VERIFY_CONTROL_CONFIG": t + "/gate/control.json",
                "VERIFY_CONTROL_SUBMIT": t + "/gate/submit.sock",
                "VERIFY_BOX": "node1",
                # the access a box gives is the agreed state's word
                # (docs/dev/joining.md "Access")
                "VERIFY_IDENTITY_FROM": "agreed",
                "VERIFY_MEMBERS": json.dumps(people.member_list()),
                "VERIFY_ADMIN": people.ADMIN,
                "VERIFY_OWNER": people.ADMIN,
                "VERIFY_ADBLOCK_HOUSEHOLD": people.ADMIN,
                "VERIFY_BOXES": json.dumps(
                    {
                        "node1": {"metrics": "", "owner": people.ADMIN},
                        "node2": {"metrics": "", "owner": people.ADMIN},
                    }
                ),
            }
        )
        for d in ("house", "ended"):
            os.makedirs(f"{t}/gate/{d}", exist_ok=True)
        return e

    # --- asking

    def url(self, host, path="/"):
        name = self.domain if host in ("", "@") else f"{host}.{self.domain}"
        return f"https://{name}{path}"

    def name(self, host):
        return self.domain if host in ("", "@") else f"{host}.{self.domain}"

    def cookie(self, who):
        return people.cookie(self.tmp + "/gate", who)

    def get(
        self, host, path, who=None, method="GET", headers=None, body=None, outside=False
    ):
        """one request to https://<host>.<domain><path>, through nginx as a
        browser on the fleet network asks it (outside: through the tunnel,
        as an outsider), no redirect followed"""
        # the test's own certificate, the only one trusted
        ctx = ssl.create_default_context(cafile=self.tmp + "/cert.pem")
        name = self.name(host)
        port = TUNNEL if outside else INSIDE
        c = http.client.HTTPSConnection(HOST, port, context=ctx, timeout=30)
        try:
            c.sock = ctx.wrap_socket(
                socket.create_connection((HOST, port), timeout=30), server_hostname=name
            )
        except (ConnectionError, ssl.SSLError) as e:
            # a name the box does not serve: the handshake is refused
            return Response(444, [], str(e).encode())
        h = {"host": name, "user-agent": "site-test"}
        if who:
            h["cookie"] = self.cookie(who)
        if outside:
            h["cf-connecting-ip"] = "203.0.113.7"
        h.update(headers or {})
        try:
            c.request(method, path, body=body, headers=h)
            r = c.getresponse()
            b = r.read()
            return Response(r.status, [(k.lower(), v) for k, v in r.getheaders()], b)
        except (ConnectionError, http.client.RemoteDisconnected, ssl.SSLError) as e:
            # nginx's 444: the connection closed with no answer
            return Response(444, [], str(e).encode())
        finally:
            c.close()

    def chrome_args(self):
        """Chromium flags that send every name of the domain here"""
        return [
            "--mute-audio",
            f"--host-resolver-rules=MAP *.{self.domain} {HOST}:{INSIDE}, MAP {self.domain} {HOST}:{INSIDE}",
        ]

    def browser_context(self, browser, who=None, **kw):
        ctx = browser.new_context(ignore_https_errors=True, **kw)
        if who:
            name, value = self.cookie(who).split("=", 1)
            ctx.add_cookies(
                [
                    {
                        "name": name,
                        "value": value,
                        "domain": "." + self.domain,
                        "path": "/",
                        "secure": True,
                        "httpOnly": True,
                        "sameSite": "Lax",
                    }
                ]
            )
        return ctx


def http_ok(url):
    import urllib.request

    try:
        return urllib.request.urlopen(url, timeout=5).status == 200
    except Exception:
        return False


class Watch:
    """a page's requests in flight, so a test waits for the page to have
    asked and been answered everything (not for a fixed time). A stream
    that stays open (EventSource, a websocket) is not waited for."""

    LONG = ("eventsource", "websocket")

    def __init__(self, page):
        self.page, self.inflight, self.bytes, self.failed, self.count = (
            page,
            set(),
            0,
            [],
            0,
        )
        page.on("request", self._start)
        page.on("requestfinished", self._done)
        page.on("requestfailed", self._fail)

    def _start(self, r):
        self.count += 1
        if r.resource_type not in self.LONG:
            self.inflight.add(r)

    def _done(self, r):
        self.inflight.discard(r)
        try:
            z = r.sizes()
            self.bytes += z["responseBodySize"] + z["responseHeadersSize"]
        except Exception:  # a request the browser dropped
            pass

    def _fail(self, r):
        self.inflight.discard(r)
        self.failed.append((r.url, r.failure))

    def settled(self, timeout=15):
        """the page loaded, its bar drawn from /_dd/me where it has one,
        and nothing it asked for still out"""
        p = self.page
        p.wait_for_load_state("load", timeout=timeout * 1000)
        if p.query_selector("#me") is not None:
            p.wait_for_function(
                "() => document.getElementById('me')?.childElementCount > 0",
                timeout=timeout * 1000,
            )
        end = time.monotonic() + timeout
        while self.inflight:
            if time.monotonic() > end:
                raise AssertionError(
                    f"{p.url}: still waiting on {[r.url for r in self.inflight][:5]}"
                )
            p.wait_for_timeout(
                20
            )  # sleep-ok: polling the page's own requests until none is out
