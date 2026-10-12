# The forge's public door (nix/modules/forge/door.nix), run: the box's own
# nginx config, served in the build, with stand-ins for Forgejo's socket and
# the gate that write down everything that reaches them. Every request is
# made as an outsider, through the tunnel's listener, and each is either
# let through to the right place as nobody, or refused before it gets there.
import json
import os
import re
import signal
import socket
import socketserver
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

conf, nginx, tmp, door, members, repo = sys.argv[1:7]
seen = []


class Stub(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def answer(self):
        n = int(self.headers.get("content-length") or 0)
        body = self.rfile.read(n) if n else b""
        seen.append(
            {
                "to": self.to,
                "method": self.command,
                "path": self.path,
                "headers": {k.lower(): v for k, v in self.headers.items()},
                "body": len(body),
            }
        )
        # a private repository, as Forgejo answers a stranger about one
        if "/secret" in self.path:
            self.send_response(401)
            self.send_header("WWW-Authenticate", 'Basic realm="Forgejo"')
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        out = f"{self.to}:{self.path}".encode()
        self.send_response(200)
        self.send_header("Set-Cookie", "i_like_gitea=someone; Path=/")
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(out)

    do_GET = do_HEAD = do_POST = do_PUT = do_PATCH = do_DELETE = do_OPTIONS = answer


class Forge(Stub):
    to = "forge"

    # a unix socket has no address to log or to hand the handler
    def address_string(self):
        return "forge"


class Gate(Stub):
    to = "gate"


class UnixHTTP(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
    daemon_threads = True

    def get_request(self):
        s, _ = self.socket.accept()
        return s, ("forge", 0)


sock = tmp + "/forgejo.sock"
for srv in (UnixHTTP(sock, Forge), ThreadingHTTPServer(("127.0.0.1", 4181), Gate)):
    threading.Thread(target=srv.serve_forever, daemon=True).start()

s = open(conf).read()
s = re.sub(r"/var/lib/acme/[^/]+/(fullchain|chain)\.pem", tmp + "/cert.pem", s)
s = re.sub(r"/var/lib/acme/[^/]+/key\.pem", tmp + "/key.pem", s)
s = re.sub(r"include\s+/(run|var|etc)/[^;]*;", "include " + tmp + "/empty.conf;", s)
s = re.sub(r"/var/(log|cache|spool)/nginx", tmp, s)
s = re.sub(r"^(\s*)pid\s+[^;]*;", r"\1pid " + tmp + "/door.pid;", s, flags=re.M)
s = re.sub(r"^(\s*)user\s+[^;]*;", "", s, flags=re.M)
s = re.sub(
    r"^(\s*)http\s*\{",
    lambda m: m.group(0) + " access_log off; client_body_temp_path " + tmp + "/body;",
    s,
    count=1,
    flags=re.M,
)
s = s.replace("/run/forgejo/forgejo.sock", sock)
# the low ports, and the tunnel's, moved where the build may listen
s = re.sub(
    r"(listen\s+(?:\S*:)?)(\d+)", lambda m: m.group(1) + str(int(m.group(2)) + 10000), s
)
s = re.sub(r"^(\s*)8443 1;", r"\g<1>18443 1;", s, flags=re.M)
open(tmp + "/empty.conf", "w").close()
open(tmp + "/door.conf", "w").write(s)
proc = subprocess.Popen(
    [nginx, "-e", "stderr", "-p", tmp, "-c", tmp + "/door.conf"],
)


def up():
    try:
        socket.create_connection(("127.0.0.1", 18443), timeout=1).close()
        return True
    except OSError:
        return False


for _ in range(100):
    if up():
        break
    time.sleep(0.1)
else:
    sys.exit("nginx did not come up")

failures = []
# every case and what came of it, printed at the end: the log is the record
told = []


def ask(method, path, host=door, headers=(), body=None, outsider=True):
    """One request, as an outsider through the tunnel: status, headers, body."""
    port = 18443 if outsider else 10443
    cmd = [
        "curl",
        "-sk",
        "--http1.1",
        "-X",
        method,
        "-o",
        tmp + "/out",
        "-D",
        tmp + "/head",
        "-w",
        "%{http_code}",
        "--path-as-is",
        "--resolve",
        f"{host}:{port}:127.0.0.1",
        f"https://{host}:{port}{path}",
    ]
    if method == "HEAD":
        cmd[cmd.index("-X") : cmd.index("-X") + 2] = ["-I"]
    if outsider:
        cmd += ["-H", "CF-Connecting-IP: 203.0.113.7"]
    for h in headers:
        cmd += ["-H", h]
    if body is not None:
        cmd += ["--data-binary", body]
    r = subprocess.run(cmd, capture_output=True, text=True)
    head = open(tmp + "/head").read() if os.path.exists(tmp + "/head") else ""
    out = open(tmp + "/out", "rb").read() if os.path.exists(tmp + "/out") else b""
    for f in ("/head", "/out"):
        if os.path.exists(tmp + f):
            os.remove(tmp + f)
    return r.stdout.strip() or "000", head.lower(), out


# what a stranger's browser or git might send, hoping to be someone
SNEAKY = [
    "Cookie: dd_session=david~x.1.y; i_like_gitea=abc",
    "Authorization: token 0123456789abcdef",
    "X-WEBAUTH-USER: david",
    "Sudo: david",
]


def allowed(method, path, to, upstream_path=None, body=None):
    before = len(seen)
    code, head, _ = ask(method, path, headers=SNEAKY, body=body)
    got = seen[before:]
    where = f"{method} {path}"
    told.append(f"  allow  {code}  {where}  -> {to}")
    if code != "200":
        failures.append(f"{where}: {code}, wanted 200")
        return
    if len(got) != 1 or got[0]["to"] != to:
        failures.append(f"{where}: reached {[g['to'] for g in got]}, wanted {to}")
        return
    g = got[0]
    if upstream_path and g["path"] != upstream_path:
        failures.append(f"{where}: {to} was asked {g['path']}")
    if g["method"] != method:
        failures.append(f"{where}: {to} was asked with {g['method']}")
    for h in ("cookie", "authorization", "sudo"):
        if g["headers"].get(h):
            failures.append(f"{where}: {to} got {h}: {g['headers'][h]}")
    if g["headers"].get("x-webauth-user"):
        failures.append(
            f"{where}: {to} was told the user is {g['headers']['x-webauth-user']}"
        )
    if "set-cookie" in head:
        failures.append(f"{where}: a cookie came back")
    if "content-security-policy: default-src 'none'" not in head:
        failures.append(f"{where}: not the door's content policy")
    if body is not None and g["body"] != len(body):
        failures.append(f"{where}: the body did not arrive")


def refused(method, path, codes, query_headers=(), host=door):
    before = len(seen)
    code, head, _ = ask(
        method,
        path,
        host=host,
        headers=list(query_headers) + SNEAKY,
        body=b"x" if method in ("POST", "PUT", "PATCH") else None,
    )
    got = seen[before:]
    where = f"{method} {host}{path}"
    told.append(f"  refuse {code}  {method} {path}")
    if code not in codes:
        failures.append(f"{where}: {code}, wanted {'/'.join(codes)}")
    if got:
        failures.append(f"{where}: reached {got[0]['to']} as {got[0]['path']}")
    if "www-authenticate" in head:
        failures.append(f"{where}: asked for credentials, which says the thing exists")


R = "/" + repo
API = "/api/v1/repos" + R

# -- allowed: the pages anyone may see, each the one public page from the gate
for p in [
    "/",
    "/david",
    R,
    R + "/tree/main/docs",
    R + "/blob/main/README.md",
    R + "/commits/main",
    R + "/commits",
    R + "/commit/0123abc",
    R + "/branches",
    R + "/releases",
    R + "/releases/tag/r200",
    R + "/src/branch/main/docs",
]:
    allowed("GET", p, "gate", "/_dd/git/public")
allowed("GET", "/_dd/static/git.js", "gate", "/_dd/static/git.js")
allowed("GET", "/_dd/web/purify.js", "gate", "/_dd/web/purify.js")
allowed("HEAD", R, "gate", "/_dd/git/public")

# -- allowed: the forge's reads the pages make, as nobody
for p in [
    "/api/v1/repos/search?q=x&sort=updated&order=desc&limit=50",
    "/api/v1/users/david/repos?limit=50",
    API,
    API + "/branches?limit=100",
    API + "/tags?limit=50",
    API + "/languages",
    API + "/commits?sha=main&path=docs&limit=1&stat=false",
    API + "/releases?limit=50",
    API + "/contents",
    API + "/contents/docs/dev?ref=main",
    API + "/raw/README.md?ref=main",
    API + "/git/commits/0123abc?stat=false",
    API + "/git/commits/0123abc.diff",
    API + "/git/trees/main?recursive=true&per_page=10000",
    API + "/commits/main/status",
    API + "/commits/feature%2Fx/status",
    API + "/compare/main...feature%2Fx",
    API + "/releases/tags/r200",
    R + "/raw/branch/main/README.md",
    R + "/media/branch/main/logo.png",
    R + "/releases/download/v0.1.0/commonty.deb",
    R + "/archive/main.tar.gz",
    "/avatars/0123abcd",
    "/repo-avatars/1-abcd",
    "/assets/img/avatar_default.png",
]:
    allowed("GET", p, "forge", p)
allowed("HEAD", R + "/raw/branch/main/README.md", "forge")

# -- allowed: clone and fetch
allowed("GET", R + ".git/info/refs?service=git-upload-pack", "forge")
allowed("GET", R + "/info/refs?service=git-upload-pack", "forge")
allowed("POST", R + ".git/git-upload-pack", "forge", body=b"0014command=ls-refs\n0000")

# -- refused: any write, to anything
for m in ["POST", "PUT", "PATCH", "DELETE"]:
    refused(m, API, ["405"])
    refused(m, API + "/contents/README.md", ["405"])
    refused(m, R, ["405"])
refused("OPTIONS", API, ["405"])
refused("GET", R + ".git/git-upload-pack", ["405"])

# -- refused: pushes, and every way git asks for one
refused("POST", R + ".git/git-receive-pack", ["404"])
refused("POST", R + "/git-receive-pack", ["404"])
refused("GET", R + ".git/info/refs?service=git-receive-pack", ["404"])
refused("GET", R + ".git/info/refs", ["404"])

# -- refused: sign-in, sign-up, settings, admin, notifications, packages, LFS,
# the runners, and every API read the pages do not make
for p in [
    "/user/login",
    "/user/sign_up",
    "/user/settings",
    "/user/events",
    "/login/oauth/authorize",
    "/-/admin",
    "/admin",
    "/explore/users",
    "/notifications",
    "/settings",
    "/new",
    "/_dd/login",
    "/_dd/home",
    "/_dd/directory",
    "/_dd/git",
    "/_dd/git/events?repo=1",
    "/_dd/fleet.json",
    "/fleet-repo.json",
    "/api/v1/user",
    "/api/v1/user/repos",
    "/api/v1/user/emails",
    "/api/v1/users/david",
    "/api/v1/users/david/heatmap",
    "/api/v1/users/david/activities/feeds",
    "/api/v1/users/search?q=a",
    "/api/v1/admin/users",
    "/api/v1/notifications",
    "/api/v1/notifications/new",
    "/api/v1/packages/david",
    "/api/v1/repos/issues/search",
    "/api/v1/orgs",
    "/api/v1/settings/api",
    API + "/collaborators",
    API + "/issues",
    API + "/pulls",
    API + "/hooks",
    API + "/keys",
    API + "/actions/runs",
    API + "/actions/secrets",
    API + "/actions/runners",
    API + "/activities/feeds",
    API + "/branch_protections",
    API + "/push_mirrors",
    API + "/subscribers",
    API + "/stargazers",
    "/api/actions/runner.v1.RunnerService/Register",
    "/api/packages/david/generic/x/1/x",
    "/v2/david/x/manifests/latest",
    R + "/info/lfs/objects/batch",
    R + "/settings",
    R + "/pulls",
    R + "/issues",
    R + "/actions",
    R + "/activity",
    R + "/actions/runs/1/jobs/0/attempt/1/logs",
]:
    refused("GET", p, ["404"])
refused("POST", R + "/info/lfs/objects/batch", ["404"])
refused("POST", "/api/actions/runner.v1.RunnerService/FetchTask", ["404"])
refused("POST", "/user/login", ["404"])
# a run is cancelled only on the members' forge, by someone who may
refused("POST", R + "/actions/runs/1/cancel", ["404"])

# -- refused: a session, a token or sudo by the query, or a name decoded
# under nginx's nose
for q in [
    "token=abc",
    "access_token=abc",
    "sudo=david",
    "q=x&TOKEN=abc",
    "%74oken=abc",
]:
    refused("GET", "/api/v1/repos/search?" + q, ["403"])
refused("GET", "/api/v1/repos/search/../../user", ["400"])
refused("GET", API + "/contents/%2e%2e/%2e%2e/%2e%2e/user", ["400"])
refused("GET", API + "/raw/%2E%2E%2F%2E%2E%2Fuser", ["400"])

# -- refused: a private repository is not there at all
for p in [
    "/api/v1/repos/tester/secret",
    "/tester/secret.git/info/refs?service=git-upload-pack",
    "/tester/secret/raw/branch/main/x",
]:
    before = len(seen)
    code, head, _ = ask("GET", p)
    told.append(f"  hide   {code}  GET {p}  (the forge said 401)")
    if code != "404" or "www-authenticate" in head:
        failures.append(
            f"GET {p}: {code}, and {'asked for' if 'www-authenticate' in head else 'no'} credentials; wanted a plain 404"
        )

# -- the members' forge is not reachable from outside at all
code, _, _ = ask("GET", "/", host=members)
told.append(f"  shut   {code}  GET {members}/ from outside")
if code != "000":
    failures.append(f"{members} from outside: {code}, wanted the connection closed")

# -- clone floods and archive floods are told to wait
codes = [ask("GET", R + ".git/info/refs?service=git-upload-pack")[0] for _ in range(40)]
told.append(f"  pace   40 ref listings at once: {codes.count('429')} told to wait")
if "429" not in codes:
    failures.append(f"40 ref listings at once were all let through: {codes}")
codes = [ask("GET", R + "/archive/main.zip")[0] for _ in range(10)]
told.append(f"  pace   10 archives at once: {codes.count('429')} told to wait")
if "429" not in codes:
    failures.append(f"10 archives at once were all let through: {codes}")

# -- the members' forge, from inside: no person reaches its admin api, and
# nobody acts as another through its sudo, header or query
# (nix/modules/forge/forgejo.nix)
before = len(seen)
code, _, _ = ask("GET", "/api/v1/admin/users", host=members, outsider=False)
told.append(f"  admin  {code}  GET {members}/api/v1/admin/users from inside")
if code != "404" or any(g["to"] == "forge" for g in seen[before:]):
    failures.append(
        f"{members}/api/v1/admin/users: {code}, wanted 404 and the forge not asked"
    )
before = len(seen)
code, _, _ = ask("GET", "/api/v1/user?sudo=someone", host=members, outsider=False)
told.append(f"  sudo   {code}  GET {members}/api/v1/user?sudo= from inside")
if code != "403" or any(g["to"] == "forge" for g in seen[before:]):
    failures.append(f"{members} ?sudo=: {code}, wanted 403 and the forge not asked")
before = len(seen)
ask("GET", "/api/v1/user", host=members, headers=["Sudo: someone"], outsider=False)
to_forge = [g for g in seen[before:] if g["to"] == "forge"]
told.append(f"  sudo   header to {members}: forge asked {len(to_forge)} time(s)")
if not to_forge or any(g["headers"].get("sudo") for g in to_forge):
    failures.append(f"{members} Sudo header: reached the forge as {to_forge}")

os.kill(proc.pid, signal.SIGTERM)
proc.wait(timeout=10)
print("\n".join(told))
if failures:
    print("\n".join(failures))
    sys.exit(1)
print(f"{door}: {len(seen)} requests let through as nobody, the rest refused")
