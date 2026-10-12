# The DNS Worker's rules, asked at its API: a box's own names get a
# challenge, signed either way a box signs; anything else is refused and
# writes nothing. Keys: box1.pem and box2.pem, made by the check.
import base64
import json
import subprocess
import sys
import time
import urllib.error
import urllib.request

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature

WORKER = "http://127.0.0.1:8787"
V1 = "A" * 43
V2 = "b-_" + "c" * 40
keys = {
    b: serialization.load_pem_private_key(open("%s.pem" % b, "rb").read(), None)
    for b in ("box1", "box2")
}


def b64u(b):
    return base64.urlsafe_b64encode(b).decode().rstrip("=")


def sign(key, msg, raw=True):
    der = key.sign(msg.encode(), ec.ECDSA(hashes.SHA256()))
    if not raw:
        return b64u(der)
    r, s = decode_dss_signature(der)
    return b64u(r.to_bytes(32, "big") + s.to_bytes(32, "big"))


def ask(
    box,
    op,
    name,
    value,
    signer=None,
    at=None,
    raw=True,
    signed=None,
    method="POST",
    path="/api/challenge",
):
    at = int(time.time()) if at is None else at
    msg = (
        "commonty acme v1\0%s\0%d\0%s\0%s\0%s" % signed
        if signed
        else "commonty acme v1\0%s\0%d\0%s\0%s\0%s" % (box, at, op, name, value)
    )
    body = {
        "box": box,
        "at": at,
        "op": op,
        "name": name,
        "value": value,
        "sig": sign(keys[signer or box], msg, raw),
    }
    req = urllib.request.Request(
        WORKER + path,
        data=json.dumps(body).encode() if method == "POST" else None,
        method=method,
        headers={"content-type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def records():
    with urllib.request.urlopen("http://127.0.0.1:8899/records") as r:
        return [
            (x["type"], x["name"], x["content"].strip('"'))
            for x in json.loads(r.read())
        ]


fails = []


def check(what, got, want):
    if got != want:
        fails.append("%s: got %r, want %r" % (what, got, want))
    print("%s %s" % ("ok  " if got == want else "FAIL", what))


now = int(time.time())
# refused, and nothing written
check("another box's name", ask("box1", "present", "photos.commonty.test", V1)[0], 403)
check("a name nobody serves", ask("box1", "present", "evil.commonty.test", V1)[0], 403)
check(
    "a box not in the fleet",
    ask("box3", "present", "home.commonty.test", V1, signer="box1")[0],
    403,
)
check(
    "signed by another box",
    ask("box1", "present", "home.commonty.test", V1, signer="box2")[0],
    403,
)
check(
    "signed for another value",
    ask(
        "box1",
        "present",
        "home.commonty.test",
        V2,
        signed=("box1", now, "present", "home.commonty.test", V1),
        at=now,
    )[0],
    403,
)
check(
    "signed for another name",
    ask(
        "box1",
        "present",
        "home.commonty.test",
        V1,
        signed=("box1", now, "present", "files.commonty.test", V1),
        at=now,
    )[0],
    403,
)
check("too old", ask("box1", "present", "home.commonty.test", V1, at=now - 400)[0], 403)
check(
    "from the future",
    ask("box1", "present", "home.commonty.test", V1, at=now + 400)[0],
    403,
)
check(
    "not a challenge value", ask("box1", "present", "home.commonty.test", "x y")[0], 400
)
check("not present or cleanup", ask("box1", "delete", "home.commonty.test", V1)[0], 400)
check(
    "not a POST", ask("box1", "present", "home.commonty.test", V1, method="GET")[0], 404
)
check(
    "not the path",
    ask("box1", "present", "home.commonty.test", V1, path="/api/x")[0],
    404,
)
check("nothing written by any of those", records(), [])

# a box's own names, either way it signs
check(
    "present, r||s",
    ask("box1", "present", "home.commonty.test", V1),
    (200, {"ok": True}),
)
check(
    "present again: one record",
    ask("box1", "present", "home.commonty.test", V1)[0],
    200,
)
check("present, DER", ask("box1", "present", "commonty.test", V2, raw=False)[0], 200)
check(
    "the other box, its own name",
    ask("box2", "present", "photos.commonty.test", V1)[0],
    200,
)
check(
    "the records",
    sorted(records()),
    sorted(
        [
            ("TXT", "_acme-challenge.home.commonty.test", V1),
            ("TXT", "_acme-challenge.commonty.test", V2),
            ("TXT", "_acme-challenge.photos.commonty.test", V1),
        ]
    ),
)
check(
    "cleanup another box's", ask("box1", "cleanup", "photos.commonty.test", V1)[0], 403
)
check("cleanup its own", ask("box1", "cleanup", "home.commonty.test", V1)[0], 200)
check(
    "cleanup takes only that one",
    sorted(r[1] for r in records()),
    ["_acme-challenge.commonty.test", "_acme-challenge.photos.commonty.test"],
)


# the box's own helper, as lego's exec provider reaches it
def helper(line):
    p = subprocess.run(
        ["bash", sys.argv[1]], input=line, capture_output=True, text=True
    )
    return p.stdout.strip()


check(
    "helper: present",
    helper("present _acme-challenge.files.commonty.test. %s\n" % V2),
    "ok",
)
check(
    "helper: the record",
    ("TXT", "_acme-challenge.files.commonty.test", V2) in records(),
    True,
)
check(
    "helper: cleanup",
    helper("cleanup _acme-challenge.files.commonty.test. %s\n" % V2),
    "ok",
)
check(
    "helper: gone",
    any(r[1] == "_acme-challenge.files.commonty.test" for r in records()),
    False,
)
check(
    "helper: refuses a name not its own, before asking",
    helper("present _acme-challenge.photos.commonty.test. %s\n" % V1).startswith(
        "not a name this box serves"
    ),
    True,
)
check(
    "helper: refuses a non-challenge name",
    helper("present home.commonty.test. %s\n" % V1).startswith("not a challenge name"),
    True,
)
check(
    "helper: refuses a bad value",
    helper("present _acme-challenge.home.commonty.test. a;b\n"),
    "not a challenge",
)


# The fleet's names, as the release has them (RECORDS in the check's
# fleet.js: door open, bare name public, hosts home, accounts, demo-files,
# tunnel t1, address 100.64.0.2). The second Worker, on 8788, has the door
# shut. Someone else's records are already in the zone.
def get(url):
    with urllib.request.urlopen(url) as r:
        return json.loads(r.read())


def seed(rs):
    req = urllib.request.Request(
        "http://127.0.0.1:8899/seed", data=json.dumps(rs).encode(), method="POST"
    )
    urllib.request.urlopen(req).read()


def names(kind=("A", "CNAME", "MX")):
    out = {}
    for x in get("http://127.0.0.1:8899/records"):
        if x["type"] in kind:
            out.setdefault(x["name"], []).append(
                (x["type"], x["content"], bool(x.get("proxied")))
            )
    return {k: sorted(v) for k, v in out.items()}


def converge(port):
    """the cron, as wrangler runs it; done when the zone stops changing"""
    before = get("http://127.0.0.1:8899/writes")
    urllib.request.urlopen(
        "http://127.0.0.1:%d/cdn-cgi/handler/scheduled?cron=*/5+*+*+*+*" % port
    ).read()
    last = -1
    for _ in range(40):
        time.sleep(0.25)
        w = get("http://127.0.0.1:8899/writes")
        if w == last:
            break
        last = w
    return last - before


T1 = "t1.cfargotunnel.com"
seed(
    [
        {
            "type": "MX",
            "name": "commonty.test",
            "content": "mx.example",
            "proxied": False,
        },
        {
            "type": "A",
            "name": "commonty.test",
            "content": "100.64.0.2",
            "proxied": False,
        },
        {
            "type": "A",
            "name": "accounts.commonty.test",
            "content": "1.2.3.4",
            "proxied": False,
        },
        {"type": "CNAME", "name": "old.commonty.test", "content": T1, "proxied": True},
        {
            "type": "CNAME",
            "name": "other.commonty.test",
            "content": "t2.cfargotunnel.com",
            "proxied": True,
        },
        {
            "type": "A",
            "name": "*.commonty.test",
            "content": "9.9.9.9",
            "proxied": False,
        },
    ]
)
txt_before = names(("TXT",))
check("names: the cron changed the zone", converge(8787) > 0, True)
check(
    "names: as the release has them, and nobody else's touched",
    names(),
    {
        "commonty.test": sorted([("CNAME", T1, True), ("MX", "mx.example", False)]),
        "*.commonty.test": [("A", "100.64.0.2", False)],
        "home.commonty.test": [("CNAME", T1, True)],
        "demo-files.commonty.test": [("CNAME", T1, True)],
        "accounts.commonty.test": [("A", "1.2.3.4", False)],
        "other.commonty.test": [("CNAME", "t2.cfargotunnel.com", True)],
    },
)
check("names: challenges untouched", names(("TXT",)), txt_before)
check("names: once in step, nothing more is written", converge(8787), 0)
check("names, door shut: the cron changed the zone", converge(8788) > 0, True)
check(
    "names, door shut: the gateway's address, no tunnel names of ours",
    names(),
    {
        "commonty.test": sorted(
            [("A", "100.64.0.2", False), ("MX", "mx.example", False)]
        ),
        "*.commonty.test": [("A", "100.64.0.2", False)],
        "accounts.commonty.test": [("A", "1.2.3.4", False)],
        "other.commonty.test": [("CNAME", "t2.cfargotunnel.com", True)],
    },
)
check("names, door shut: once in step, nothing more", converge(8788), 0)

# Failover (the third Worker): a box's own tunnel, and the names following
# the leases in the agreed state
W3 = "http://127.0.0.1:8789"


def door(what, box, extra=None, parts=(), signer=None, at=None):
    at = int(time.time()) if at is None else at
    msg = "\0".join(["commonty %s v1" % what, box, str(at)] + list(parts))
    body = dict(extra or {}, box=box, at=at, sig=sign(keys[signer or box], msg))
    req = urllib.request.Request(
        "%s/api/%s" % (W3, what),
        data=json.dumps(body).encode(),
        method="POST",
        headers={"content-type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def lease(fresh, **holders):
    leases = {
        "svc/" + svc.replace("_", "-"): {"holder": b, "epoch": 1, "expires": 2**53}
        for svc, b in holders.items()
    }
    req = urllib.request.Request(
        "http://127.0.0.1:8899/control",
        data=json.dumps({"fresh": fresh, "leases": leases}).encode(),
        method="POST",
    )
    urllib.request.urlopen(req).read()


S1 = base64.b64encode(b"\1" * 32).decode()
S2 = base64.b64encode(b"\2" * 32).decode()
code, _ = door("tunnel", "box2", {"secret": S1}, [S1], signer="box1")
check(
    "tunnel: another box's signature makes none",
    (code, get("http://127.0.0.1:8899/tunnels")),
    (403, {}),
)
code, got = door("tunnel", "box2", {"secret": S1}, [S2])
check("tunnel: a signature over another secret makes none", code, 403)
t0 = int(time.time())
code, got = door("tunnel", "box2", {"secret": S1}, [S1], at=t0)
t = get("http://127.0.0.1:8899/tunnels")
check(
    "tunnel: box2's own, with the secret it made",
    (code, t.get(got.get("id"))),
    (200, {"name": "commonty-box2", "secret": S1}),
)
first = got.get("id")
code, _ = door("tunnel", "box2", {"secret": S1}, [S1], at=t0)
check(
    "tunnel: the same request again changes nothing",
    (code, list(get("http://127.0.0.1:8899/tunnels"))),
    (409, [first]),
)
# the box lost its secret and asks again, later
code, got = door("tunnel", "box2", {"secret": S2}, [S2], at=t0 + 2)
t = get("http://127.0.0.1:8899/tunnels")
check(
    "tunnel: asked again, the old one goes",
    (code, first in t, t.get(got.get("id"), {}).get("secret")),
    (200, False, S2),
)
B2 = "%s.cfargotunnel.com" % got.get("id")

# no fresh agreement: nothing that moves is touched
lease(False, headscale="box2", ente_museum="box2")
before = names()
converge(8789)
check("failover, stale agreement: nothing moves", names(), before)

lease(True, headscale="box2", ente_museum="box2")
check("failover: the cron moved the names", converge(8789) > 0, True)
now = names()
check(
    "failover: * at the gateway's lease's box",
    now.get("*.commonty.test"),
    [("A", "100.64.0.5", False)],
)
check(
    "failover: a public name at its tunnel",
    now.get("home.commonty.test"),
    [("CNAME", B2, True)],
)
check(
    "failover: a service's name at its lease's box",
    now.get("photos.commonty.test"),
    [("A", "100.64.0.5", False)],
)

# back to box1, asked by a box rather than waiting for the cron
lease(True, headscale="box1", ente_museum="box1")
code, _ = door("converge", "box1", signer="box2")
check("converge: another box's signature is refused", code, 403)
code, got = door("converge", "box1")
check(
    "converge: asked by a box, in step at once",
    (code, got.get("changed", 0) > 0),
    (200, True),
)
now = names()
check(
    "failover back: * at box1", now.get("*.commonty.test"), [("A", "100.64.0.2", False)]
)
check(
    "failover back: box1 has no tunnel of its own, so the release's",
    now.get("home.commonty.test"),
    [("CNAME", T1, True)],
)
check(
    "failover back: the service's name at box1",
    now.get("photos.commonty.test"),
    [("A", "100.64.0.2", False)],
)

# box1's own door, as its helper asks: a tunnel with a secret it made, kept
# in its credentials; asked again with them there, nothing is asked
d = subprocess.run(["mktemp", "-d"], capture_output=True, text=True).stdout.strip()
p = subprocess.run(["bash", sys.argv[2], "tunnel", d], capture_output=True, text=True)
print(p.stdout, p.stderr)
creds = json.load(open(d + "/credentials.json")) if p.returncode == 0 else {}
t = get("http://127.0.0.1:8899/tunnels")
check(
    "door helper: box1's tunnel, its secret kept here alone",
    (t.get(creds.get("TunnelID"), {}).get("secret"), creds.get("AccountTag")),
    (creds.get("TunnelSecret"), "acct1"),
)
check("door helper: made one", bool(creds), True)
before = dict(t)
subprocess.run(["bash", sys.argv[2], "tunnel", d], capture_output=True, text=True)
check(
    "door helper: once made, not asked again",
    get("http://127.0.0.1:8899/tunnels"),
    before,
)
converge(8789)
check(
    "door: box1's public name at its own tunnel now",
    names().get("home.commonty.test"),
    [("CNAME", "%s.cfargotunnel.com" % creds.get("TunnelID"), True)],
)
p = subprocess.run(["bash", sys.argv[2], "converge"], capture_output=True, text=True)
check("door helper: converge", (p.returncode, "names in step" in p.stdout), (0, True))

if fails:
    print("\n".join(fails))
    sys.exit(1)
