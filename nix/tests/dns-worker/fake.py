# A stand-in for Cloudflare's DNS API, for the DNS Worker's check: one
# zone, records in memory, the token checked. GET /records shows them,
# GET /writes counts what was changed, POST /seed puts records in as if
# someone else had made them. With
# CHALLTESTSRV (the acme vm test), a TXT is also served by pebble's DNS.
import json
import os
import sys
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

ZONE, TOKEN = "zone1", "dns-token"
# the agreed identity state's word on the mail Worker's tester (TESTER, its
# fixture), as the control relay's /identity gives it; POST /identity-set
# changes their status
TESTER = json.load(open(os.environ["TESTER"])) if os.environ.get("TESTER") else None
identity = {"status": "member"}
PREFIX = "/client/v4/zones/%s/dns_records" % ZONE
# the account's tunnels, and the token that may make them
TUNNELS, TUNNEL_TOKEN = "/client/v4/accounts/acct1/cfd_tunnel", "tunnel-token"
tunnels = {}
made = {}
# the agreed state's leases, as the control plane's relay gives them: POST
# /control sets them and whether the agreement is fresh
control = {"fresh": False, "leases": {}}
records = {}
n = [0]
writes = [0]
DNS = os.environ.get("CHALLTESTSRV")


def served(what, host, value=None):
    if DNS:
        body = {"host": host + "."} | ({"value": value} if value is not None else {})
        urllib.request.urlopen(
            urllib.request.Request(DNS + what, data=json.dumps(body).encode()),
            timeout=10,
        )


class H(BaseHTTPRequestHandler):
    # workerd keeps its connection open, as the mail Worker's fake learned
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        sys.stderr.write("%s %s\n" % (self.command, self.path))

    def reply(self, code, v):
        b = json.dumps(v).encode()
        self.send_response(code)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def ok(self, result):
        self.reply(200, {"success": True, "errors": [], "result": result})

    def authed(self):
        if self.headers.get("authorization") != "Bearer " + TOKEN:
            self.reply(403, {"success": False, "errors": [{"message": "bad token"}]})
            return False
        return True

    def tunnel_authed(self):
        if self.headers.get("authorization") != "Bearer " + TUNNEL_TOKEN:
            self.reply(403, {"success": False, "errors": [{"message": "bad token"}]})
            return False
        return True

    def do_GET(self):
        u = urlparse(self.path)
        if u.path == "/control":
            now = int(time.time() * 1000)
            t = now if control["fresh"] else now - 600_000
            return self.reply(
                200,
                {
                    "time": t,
                    "leases": control["leases"],
                    "joined": control.get("joined", {}),
                    "placed": control.get("placed", {}),
                    # a box's own word, which the Worker must not follow
                    "runs": control.get("runs", {}),
                },
            )
        if u.path == "/tunnels":
            return self.reply(200, tunnels)
        if u.path == "/identity":
            name = parse_qs(u.query).get("name", [""])[0]
            if not TESTER or name != "tester":
                return self.reply(404, {"error": "no such name"})
            e = TESTER["v3"]
            # as state/ident/serve.go answers a name
            return self.reply(
                200,
                {
                    "height": 100,
                    "time": 1792270000000,
                    "changed": 90,
                    "name": "tester",
                    "member": {
                        "root": e["entry"]["root"],
                        "kind": "ed25519",
                        "status": identity["status"],
                        "item": {"entry": e},
                    },
                },
            )
        if u.path == TUNNELS:
            if not self.tunnel_authed():
                return
            q = parse_qs(u.query)
            name = q.get("name", [None])[0]
            return self.ok(
                [
                    {"id": i, "name": t["name"], "created_at": made[i]}
                    for i, t in tunnels.items()
                    if name is None or t["name"] == name
                ]
            )
        if u.path == "/records":
            return self.reply(200, sorted(records.values(), key=lambda r: r["id"]))
        if u.path == "/writes":
            return self.reply(200, writes[0])
        if u.path != PREFIX:
            return self.reply(404, {"success": False})
        if not self.authed():
            return
        q = parse_qs(u.query)
        out = [
            r
            for r in records.values()
            if r["type"] == q.get("type", [r["type"]])[0]
            and r["name"] == q.get("name", [r["name"]])[0]
        ]
        self.ok(out)

    def do_POST(self):
        # read whatever comes: the connection is kept for the next request
        raw = self.rfile.read(int(self.headers.get("content-length") or 0))
        if self.path == "/control":
            control.update(json.loads(raw))
            return self.reply(200, {"ok": True})
        if self.path == "/identity-set":
            identity.update(json.loads(raw))
            return self.reply(200, {"ok": True})
        if self.path == TUNNELS:
            if not self.tunnel_authed():
                return
            body = json.loads(raw)
            n[0] += 1
            i = "tun%d" % n[0]
            tunnels[i] = {"name": body["name"], "secret": body["tunnel_secret"]}
            made[i] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
            return self.ok({"id": i, "name": body["name"]})
        if self.path == "/seed":
            for r in json.loads(raw):
                n[0] += 1
                records["r%d" % n[0]] = dict(r, id="r%d" % n[0])
            return self.reply(200, {"ok": True})
        if self.path != PREFIX:
            return self.reply(404, {"success": False})
        if not self.authed():
            return
        body = json.loads(raw)
        n[0] += 1
        writes[0] += 1
        r = dict(body, id="r%d" % n[0])
        # as Cloudflare does now: a TXT comes back in quotes
        if body["type"] == "TXT":
            r["content"] = '"%s"' % body["content"]
        records[r["id"]] = r
        served("/set-txt", body["name"], body["content"])
        self.ok(r)

    def do_PUT(self):
        raw = self.rfile.read(int(self.headers.get("content-length") or 0))
        if not self.path.startswith(PREFIX + "/"):
            return self.reply(404, {"success": False})
        if not self.authed():
            return
        rid = self.path[len(PREFIX) + 1 :]
        if rid not in records:
            return self.reply(
                404, {"success": False, "errors": [{"message": "no such record"}]}
            )
        writes[0] += 1
        records[rid] = dict(json.loads(raw), id=rid)
        self.ok(records[rid])

    def do_DELETE(self):
        if self.path.startswith(TUNNELS + "/"):
            if not self.tunnel_authed():
                return
            rest = self.path[len(TUNNELS) + 1 :]
            if rest.endswith("/connections"):
                return self.ok({})
            tunnels.pop(rest, None)
            return self.ok({"id": rest})
        if not self.path.startswith(PREFIX + "/"):
            return self.reply(404, {"success": False})
        if not self.authed():
            return
        rid = self.path[len(PREFIX) + 1 :]
        r = records.pop(rid, None)
        writes[0] += r is not None
        if r is None:
            return self.reply(
                404, {"success": False, "errors": [{"message": "no such record"}]}
            )
        # challtestsrv clears every TXT at a name; put back the others
        served("/clear-txt", r["name"])
        for o in records.values():
            if o["name"] == r["name"]:
                served("/set-txt", o["name"], o["content"].strip('"'))
        self.ok({"id": rid})


ThreadingHTTPServer(
    ("127.0.0.1", int(os.environ.get("PORT", "8899"))), H
).serve_forever()
