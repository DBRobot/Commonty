#!/usr/bin/env python3
# A member's own address, kept at Cloudflare and nowhere on this box: mail to
# <name>@<domain> is forwarded there by an Email Routing rule. The gate hands
# an address over once, when the member gives it, on a socket only the gate
# may open; this makes or updates that member's rule and forgets it. Asked
# about a member, it says whether a rule exists, never where it goes.
#
# One request per connection, one line of json each way:
#   {"op": "set", "name": "sarah", "email": "sarah@example.com"}
#   {"op": "state", "name": "sarah"}   forwarding set, and confirmed yet
#   {"op": "resend", "name": "sarah"}  Cloudflare's confirmation, again
# Environment: CF_DNS_API_TOKEN, ZONE, SOCKET.
import json
import os
import re
import socketserver
import urllib.error
import urllib.request

API = "https://api.cloudflare.com/client/v4"
token = os.environ["CF_DNS_API_TOKEN"]
zone_name = os.environ["ZONE"]
NAME = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
EMAIL = re.compile(r"^[^@\s]{1,64}@[A-Za-z0-9.-]{1,253}\.[A-Za-z]{2,}$")


def call(method, path, body=None):
    req = urllib.request.Request(
        API + path,
        method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req, timeout=30) as r:
            return json.load(r)
    except urllib.error.HTTPError as e:
        return json.load(e)


zone = call("GET", "/zones?name=" + zone_name)["result"][0]
zone_id, account_id = zone["id"], zone["account"]["id"]


def rule_for(address):
    page = 1
    while True:
        d = call("GET", "/zones/%s/email/routing/rules?per_page=50&page=%d" % (zone_id, page))
        rules = d.get("result") or []
        for r in rules:
            if any(m.get("field") == "to" and m.get("value") == address for m in r.get("matchers", [])):
                return r
        if len(rules) < 50:
            return None
        page += 1


def target(rule):
    """where a rule forwards to, read here and never handed on"""
    for a in rule.get("actions", []):
        if a.get("type") == "forward" and a.get("value"):
            return a["value"][0]
    return None


def destination(email):
    """Cloudflare's record of a destination: its id and whether its owner
    has confirmed it"""
    page = 1
    while True:
        d = call("GET", "/accounts/%s/email/routing/addresses?per_page=50&page=%d" % (account_id, page))
        found = d.get("result") or []
        for a in found:
            if a.get("email", "").lower() == email.lower():
                return a
        if len(found) < 50:
            return None
        page += 1


def handle(req):
    name = req.get("name", "")
    if not NAME.match(name):
        return {"ok": False, "why": "not a member name"}
    address = "%s@%s" % (name, zone_name)
    rule = rule_for(address)
    if req.get("op") == "state":
        to = target(rule) if rule else None
        a = to and destination(to)
        return {"ok": True, "forwarding": rule is not None, "confirmed": bool(a and a.get("verified"))}
    if req.get("op") == "resend":
        to = target(rule) if rule else None
        a = to and destination(to)
        if not a:
            return {"ok": False, "why": "no address to confirm; give one first"}
        if a.get("verified"):
            return {"ok": True, "confirmed": True}
        # Cloudflare sends its message when a destination is added: added
        # again, it sends it again
        call("DELETE", "/accounts/%s/email/routing/addresses/%s" % (account_id, a.get("tag") or a.get("id")))
        again = call("POST", "/accounts/%s/email/routing/addresses" % account_id, {"email": to})
        print("%s: confirmation sent again" % name, flush=True)
        return {"ok": bool(again.get("success")), "confirmed": False}
    email = req.get("email", "").strip()
    if req.get("op") != "set" or not EMAIL.match(email):
        return {"ok": False, "why": "that is not an email address"}
    # Cloudflare forwards only to an address its owner has confirmed: a new
    # one gets a confirmation mail from Cloudflare; one already confirmed
    # answers that it exists, which is as good
    added = call("POST", "/accounts/%s/email/routing/addresses" % account_id, {"email": email})
    fresh = bool(added.get("success"))
    body = {
        "name": "Commonty member " + name,
        "enabled": True,
        "matchers": [{"type": "literal", "field": "to", "value": address}],
        "actions": [{"type": "forward", "value": [email]}],
    }
    if rule:
        d = call("PUT", "/zones/%s/email/routing/rules/%s" % (zone_id, rule["id"]), body)
    else:
        d = call("POST", "/zones/%s/email/routing/rules" % zone_id, body)
    if not d.get("success"):
        print("%s: cloudflare refused the rule" % name, flush=True)
        return {"ok": False, "why": "Cloudflare refused it; try again in a minute"}
    print("%s: forwarding %s" % (name, "set, awaiting confirmation" if fresh else "updated"), flush=True)
    return {"ok": True, "confirm": fresh}


class Handler(socketserver.StreamRequestHandler):
    def handle(self):
        try:
            req = json.loads(self.rfile.readline(4096))
            out = handle(req)
        except Exception as e:  # never the address, only that something failed
            print("request failed: %s" % type(e).__name__, flush=True)
            out = {"ok": False, "why": "something went wrong; try again"}
        self.wfile.write((json.dumps(out) + "\n").encode())


path = os.environ["SOCKET"]
if os.path.exists(path):
    os.unlink(path)
server = socketserver.UnixStreamServer(path, Handler)
os.chmod(path, 0o660)
server.serve_forever()
