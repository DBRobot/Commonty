#!/usr/bin/env python3
# A failing job stops the whole run. Forgejo's api cannot cancel a run, but
# its web route can, and this box is the forge: a job posts its run's id
# here, and this cancels the run as the admin through forgejo's socket -
# once the forge itself records one of that run's jobs as failed.
#
# No secret: anyone may ask, and all they can do is stop a run that has
# already failed, which is what would happen anyway. So there is nothing to
# leak to a pull request's jobs.
# Environment: FORGE (the unix socket), ADMIN, REPO (owner/name), LISTEN (port).
#
# The socket, not a port: forgejo believes X-WEBAUTH-USER from anything that
# reaches it, and jobs run on this box. Only what systemd puts in forgejo's
# group can open it, and a job's user is not in that group.
import http.client
import http.server
import json
import os
import re
import socket
import threading
import time

forge = os.environ["FORGE"]
admin = os.environ["ADMIN"]
repo = os.environ["REPO"]


class UnixConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout):
        super().__init__("localhost", timeout=timeout)
        self.unix = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.unix)


def ask(method, path, body=None):
    """forgejo, as the admin, over its socket: (status, body bytes)"""
    c = UnixConnection(forge, 20)
    try:
        c.request(method, path, body=body, headers={"X-WEBAUTH-USER": admin})
        r = c.getresponse()
        return r.status, r.read()
    finally:
        c.close()


def failed(index, sha):
    """whether the forge has recorded a job of run `index` as failed"""
    status, body = ask("GET", "/api/v1/repos/%s/commits/%s/statuses?limit=100" % (repo, sha))
    if status != 200:
        return False
    mine = "/actions/runs/%s/jobs/" % index
    return any(
        s.get("status") == "failure"
        and mine in (s.get("target_url") or "")
        and not (s.get("description") or "").startswith("Has been cancelled")
        for s in json.loads(body)
    )


def stop(run):
    """cancel `run` once one of its jobs is on record as failed. The asking
    job is still in its last step when it asks, so its own failure lands a
    moment later; a run with none within two minutes is left alone."""
    try:
        status, body = ask("GET", "/api/v1/repos/%s/actions/runs/%s" % (repo, run))
        if status != 200:
            raise ValueError("http %s" % status)
        r = json.loads(body)
        # the web route wants the run's number in the repository, which the
        # api gives as the tail of the run's page
        index = r["html_url"].rstrip("/").rsplit("/", 1)[1]
        sha = r["commit_sha"]
    except (OSError, KeyError, ValueError) as e:
        print("run %s: not found (%s)" % (run, e), flush=True)
        return
    for _ in range(24):
        try:
            if failed(index, sha):
                code, _ = ask("POST", "/%s/actions/runs/%s/cancel" % (repo, index), b"")
                print("run %s (#%s): cancel -> %s" % (run, index, code), flush=True)
                return
        except (OSError, ValueError) as e:
            print("run %s: %s" % (run, e), flush=True)
        time.sleep(5)
    print("run %s (#%s): no failed job on record; left running" % (run, index), flush=True)


class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        m = re.fullmatch(r"/_dd/ci/cancel/(\d+)", self.path)
        if not m:
            self.send_response(404)
            self.end_headers()
            return
        threading.Thread(target=stop, args=(m.group(1),), daemon=True).start()
        self.send_response(202)
        self.end_headers()

    def log_message(self, *_):
        pass


http.server.ThreadingHTTPServer(("127.0.0.1", int(os.environ["LISTEN"])), Handler).serve_forever()
