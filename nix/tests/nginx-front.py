# The bare name's front page, run: the box's own nginx config, served in
# the build, with a stand-in for the gate that writes down what reaches it.
# Every request is an outsider's, through the tunnel's listener. The front
# page is served at / (not a redirect), home's / is still the way in, and
# the docs, the download and security.txt answer as they did.
import os
import re
import signal
import socket
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

conf, nginx, tmp, bare = sys.argv[1:5]
home = "home." + bare
seen = []


class Gate(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def do_GET(self):
        seen.append(self.path)
        out = f"gate:{self.path}".encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(out)))
        self.end_headers()
        self.wfile.write(out)


gate = ThreadingHTTPServer(("127.0.0.1", 4181), Gate)
threading.Thread(target=gate.serve_forever, daemon=True).start()

s = open(conf).read()
s = re.sub(r"/var/lib/acme/[^/]+/(fullchain|chain)\.pem", tmp + "/cert.pem", s)
s = re.sub(r"/var/lib/acme/[^/]+/key\.pem", tmp + "/key.pem", s)
s = re.sub(r"include\s+/(run|var|etc)/[^;]*;", "include " + tmp + "/empty.conf;", s)
s = re.sub(r"/var/(log|cache|spool)/nginx", tmp, s)
s = re.sub(r"^(\s*)pid\s+[^;]*;", r"\1pid " + tmp + "/front.pid;", s, flags=re.M)
s = re.sub(r"^(\s*)user\s+[^;]*;", "", s, flags=re.M)
s = re.sub(
    r"^(\s*)http\s*\{",
    lambda m: m.group(0) + " access_log off; client_body_temp_path " + tmp + "/body;",
    s,
    count=1,
    flags=re.M,
)
s = re.sub(
    r"(listen\s+(?:\S*:)?)(\d+)", lambda m: m.group(1) + str(int(m.group(2)) + 10000), s
)
s = re.sub(r"^(\s*)8443 1;", r"\g<1>18443 1;", s, flags=re.M)
open(tmp + "/empty.conf", "w").close()
open(tmp + "/front.conf", "w").write(s)
proc = subprocess.Popen([nginx, "-e", "stderr", "-p", tmp, "-c", tmp + "/front.conf"])

for _ in range(100):
    try:
        socket.create_connection(("127.0.0.1", 18443), timeout=1).close()
        break
    except OSError:
        time.sleep(0.1)
else:
    sys.exit("nginx did not come up")


def ask(path, host=bare):
    r = subprocess.run(
        [
            "curl",
            "-sk",
            "--http1.1",
            "-o",
            tmp + "/out",
            "-D",
            tmp + "/head",
            "-w",
            "%{http_code}",
            "--path-as-is",
            "--resolve",
            f"{host}:18443:127.0.0.1",
            "-H",
            "CF-Connecting-IP: 203.0.113.7",
            f"https://{host}:18443{path}",
        ],
        capture_output=True,
        text=True,
    )
    head = open(tmp + "/head").read().lower()
    out = open(tmp + "/out", "rb").read().decode(errors="replace")
    os.remove(tmp + "/head")
    os.remove(tmp + "/out")
    return r.stdout.strip(), head, out


failures = []
told = []


def served(path, upstream, host=bare):
    code, head, out = ask(path, host)
    told.append(f"  {code}  {host}{path}  -> {out}")
    if code != "200" or out != f"gate:{upstream}":
        failures.append(f"{host}{path}: {code} {out!r}, wanted the gate's {upstream}")
    elif "content-security-policy: default-src 'self'" not in head:
        failures.append(f"{host}{path}: not our pages' content policy")


def moved(path, to, host=bare, want="302"):
    code, head, _ = ask(path, host)
    where = re.search(r"^location: (\S+)", head, re.M)
    told.append(f"  {code}  {host}{path}  -> {where and where.group(1)}")
    if code != want or not where or where.group(1) != to:
        failures.append(
            f"{host}{path}: {code} to {where and where.group(1)}, wanted {want} to {to}"
        )


# the front page, for anyone, at the bare name itself
served("/", "/_dd/landing")
served("/_dd/static/landing.css", "/_dd/static/landing.css")
served("/_dd/static/footer.js", "/_dd/static/footer.js")
# home's own front is still the way in
served("/", "/_dd/home", host=home)
# the docs, the download and everything else as before
served("/docs", "/_dd/docs/page")
served("/docs/guide/joining", "/_dd/docs/page/guide/joining")
# the one fixed address; everything else a 302, which browsers don't keep
moved("/download", f"https://{home}/_dd/download", want="301")
moved("/_dd/login", f"https://{home}/_dd/login")
moved("/anything", f"https://{home}/anything")
code, _, out = ask("/.well-known/security.txt")
told.append(f"  {code}  {bare}/.well-known/security.txt")
if code != "200" or "Contact: mailto:security@" not in out:
    failures.append(f"{bare}/.well-known/security.txt: {code}")

os.kill(proc.pid, signal.SIGTERM)
proc.wait(timeout=10)
gate.shutdown()
print("\n".join(told))
if failures:
    print("\n".join(failures))
    sys.exit(1)
print(f"{bare}: the front page at /, home's sign-in, the docs and the download")
