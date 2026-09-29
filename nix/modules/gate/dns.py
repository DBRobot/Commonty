#!/usr/bin/env python3
# The fleet's names at Cloudflare, converged: `*` points at the gateway's
# tailnet address, and each public host is a proxied CNAME to the tunnel
# while the door is open; the bare name is one of them when BARE is set,
# and the tailnet address otherwise. Only records this script names are
# touched; anything else in the zone is left alone. Environment: the token,
# ZONE, TAILNET, TUNNEL, HOSTS (space separated), PUBLIC and BARE (true/false).
import json
import os
import sys
import urllib.request

API = "https://api.cloudflare.com/client/v4"
token = os.environ["CF_DNS_API_TOKEN"]
zone = os.environ["ZONE"]
tailnet = os.environ["TAILNET"]
tunnel = os.environ["TUNNEL"]
hosts = os.environ["HOSTS"].split()
public = os.environ["PUBLIC"] == "true"
# the bare name through the tunnel too: what people type to find the site
bare = public and os.environ.get("BARE") == "true"


def call(method, path, body=None):
    req = urllib.request.Request(
        API + path,
        method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Authorization": "Bearer " + token, "Content-Type": "application/json"},
    )
    with urllib.request.urlopen(req, timeout=30) as r:
        d = json.load(r)
    if not d["success"]:
        sys.exit("cloudflare: %s %s: %s" % (method, path, d["errors"]))
    return d["result"]


zone_id = call("GET", "/zones?name=" + zone)[0]["id"]
have = {}
for r in call("GET", "/zones/%s/dns_records?per_page=500" % zone_id):
    have.setdefault(r["name"], []).append(r)

want = {
    zone: ("CNAME", tunnel + ".cfargotunnel.com", True) if bare else ("A", tailnet, False),
    "*." + zone: ("A", tailnet, False),
}
for h in hosts:
    if public:
        want[h + "." + zone] = ("CNAME", tunnel + ".cfargotunnel.com", True)
    else:
        want[h + "." + zone] = None

ours = tunnel + ".cfargotunnel.com"


def mine(r):
    """a record this script made: the tailnet A, or a CNAME to this tunnel.
    Mail, TXT, CAA, another tunnel's names: never touched."""
    return (r["type"] == "A" and r["content"] == tailnet) or (
        r["type"] == "CNAME" and r["content"] == ours
    )


def delete(name, r, why):
    call("DELETE", "/zones/%s/dns_records/%s" % (zone_id, r["id"]))
    print("removed", name, r["type"], "(%s)" % why)


# a name that was public and no longer is: its proxied record goes, so the
# wildcard answers with the tailnet address again
for name, records in have.items():
    if name in want:
        continue
    for r in records:
        if r["type"] == "CNAME" and r["content"] == ours:
            delete(name, r, "no longer public")

for name, spec in want.items():
    current = have.get(name, [])
    if spec is None:
        for r in current:
            if r["type"] == "CNAME" and r["content"] == ours:
                delete(name, r, "not public")
        continue
    typ, content, proxied = spec
    body = {"type": typ, "name": name, "content": content, "proxied": proxied, "ttl": 1}
    match = [r for r in current if r["type"] == typ]
    # what stands in the way of the record: a CNAME shares its name with
    # nothing, and an A with no CNAME. Ours goes; anyone else's is left
    # and the record is not made, loudly.
    clash = [
        r
        for r in current
        if r["type"] != typ and (typ == "CNAME" or r["type"] == "CNAME")
    ]
    if any(not mine(r) for r in clash):
        print("left", name, "alone: it has records this script did not make", file=sys.stderr)
        continue
    for r in clash:
        delete(name, r, "replaced by " + typ)
    if not match:
        call("POST", "/zones/%s/dns_records" % zone_id, body)
        print("added", name, typ, content)
    elif match[0]["content"] != content or match[0]["proxied"] != proxied:
        if not mine(match[0]) and typ == "CNAME":
            print("left", name, "alone: its CNAME points elsewhere", file=sys.stderr)
            continue
        call("PUT", "/zones/%s/dns_records/%s" % (zone_id, match[0]["id"]), body)
        print("updated", name, typ, content)
print("%s: %s" % (zone, "the door is open" if public else "tailnet only"))
