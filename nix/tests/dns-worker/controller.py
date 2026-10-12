# Home controllers' names at the DNS Worker (servers/system/dns/controller.js):
# a member's root names one, nobody else can, and a controller gets
# challenges for its own name and no other. The member is the mail
# Worker's made-up tester (client/mail/tests/tester.json), whose root is the
# test key of 32 sevens.
import base64
import hashlib
import json
import sys
import time
import urllib.error
import urllib.request

from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

WORKER = "http://127.0.0.1:8787"
tester = json.load(open(sys.argv[1]))
entry = json.dumps(tester["v3"])
root = Ed25519PrivateKey.from_private_bytes(bytes([7] * 32))
controller = Ed25519PrivateKey.from_private_bytes(bytes([5] * 32))
stranger = Ed25519PrivateKey.from_private_bytes(bytes([8] * 32))


def raw(k):
    return k.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )


def pub(k):
    return base64.b64encode(raw(k)).decode()


# a controller's label is its key's (identity::controller_label)
def label_of(k):
    h = hashlib.sha256(b"commonty controller label v1\0" + raw(k)).digest()
    return base64.b32encode(h[:10]).decode().lower()


LABEL = label_of(controller)
NAME = "%s.home.commonty.test" % LABEL


def sign(k, msg):
    return base64.b64encode(k.sign(msg.encode())).decode()


def post(path, body):
    req = urllib.request.Request(
        WORKER + path,
        data=json.dumps(body).encode(),
        method="POST",
        headers={"content-type": "application/json"},
    )
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, json.loads(r.read())
    except urllib.error.HTTPError as e:
        return e.code, json.loads(e.read() or b"{}")


def records():
    with urllib.request.urlopen("http://127.0.0.1:8899/records") as r:
        return sorted(
            (x["type"], x["name"], x["content"].strip('"'))
            for x in json.loads(r.read())
            # controllers' names only, under home: the failover checks
            # before this one point home itself at a tunnel
            if x["name"].endswith(".home.commonty.test")
        )


def register(
    label=LABEL, by=root, ctl=controller, address="100.64.0.9", at=None, own=None
):
    at = int(time.time()) if at is None else at
    name = "%s.home.commonty.test" % label
    return post(
        "/api/controller",
        {
            "label": label,
            "controller": pub(ctl),
            "entry": entry,
            "sig": sign(by, "commonty controller name v1\0%s\0%s" % (name, pub(ctl))),
            "address": address,
            "at": at,
            "own": sign(
                own or ctl,
                "commonty controller address v1\0%s\0%s\0%d" % (label, address, at),
            ),
        },
    )


def challenge(
    name, signer, label=LABEL, op="present", value="C" * 43, by=root, word=True
):
    at = int(time.time())
    body = {
        "controller": label,
        "at": at,
        "op": op,
        "name": name,
        "value": value,
        "sig": sign(
            signer,
            "commonty acme v1\0%s\0%d\0%s\0%s\0%s" % (label, at, op, name, value),
        ),
    }
    if word:
        # the member's word for it, every time
        body["key"] = pub(controller)
        body["entry"] = entry
        body["name_sig"] = sign(
            by,
            "commonty controller name v1\0%s.home.commonty.test\0%s"
            % (label, pub(controller)),
        )
    return post("/api/challenge", body)


fails = []


def check(what, got, want):
    if got != want:
        fails.append("%s: got %r, want %r" % (what, got, want))
    print("%s %s" % ("ok  " if got == want else "FAIL", what))


check("named by a stranger's key", register(by=stranger)[0], 403)
check("a label that is a word", register(label="living-room-pi")[0], 400)
check(
    "a label that is another controller's",
    register(label=label_of(stranger))[0],
    403,
)
check("an address off the fleet's network", register(address="192.168.1.5")[0], 400)
check("the controller did not sign its address", register(own=stranger)[0], 403)
check("stale", register(at=int(time.time()) - 600)[0], 403)
check("nothing written by those", records(), [])
check("a challenge for a name nobody has", challenge(NAME, controller)[0], 403)

check("named by the member's root", register(), (200, {"ok": True, "name": NAME}))
check(
    "its key and its address, and nothing of the member",
    records(),
    [
        ("A", NAME, "100.64.0.9"),
        ("TXT", "_controller." + NAME, "key=" + pub(controller)),
    ],
)
check("another controller cannot take the name", register(ctl=stranger)[0], 409)
check("its own name's challenge", challenge(NAME, controller)[0], 200)
check("signed by anyone else", challenge(NAME, stranger, value="D" * 43)[0], 403)
check("another name", challenge("home.commonty.test", controller)[0], 403)
check(
    "without its member's word",
    challenge(NAME, controller, value="E" * 43, word=False)[0],
    403,
)
check(
    "with a word that is not its member's root's",
    challenge(NAME, controller, value="F" * 43, by=stranger)[0],
    403,
)

# its address moves only with its member's word again
moved = register(address="100.64.0.10")
check("it moves itself", moved, (200, {"ok": True, "name": NAME}))
check(
    "nobody else moves it",
    register(address="100.64.0.11", own=stranger)[0],
    403,
)
check(
    "nor without its member's word",
    register(address="100.64.0.11", by=stranger)[0],
    403,
)
check("no other way to move it", post("/api/controller/address", {})[0], 404)
check(
    "where it is now",
    [r for r in records() if r[0] == "A"],
    [("A", NAME, "100.64.0.10")],
)

# the fleet's zone is not anyone's to fill: past the cap, no new name
seed = [
    {
        "type": "TXT",
        "name": "_controller.%s.home.commonty.test"
        % base64.b32encode(i.to_bytes(10, "big")).decode().lower(),
        "content": "key=x",
    }
    for i in range(64)
]
req = urllib.request.Request(
    "http://127.0.0.1:8899/seed",
    data=json.dumps(seed).encode(),
    method="POST",
    headers={"content-type": "application/json"},
)
urllib.request.urlopen(req).read()
third = Ed25519PrivateKey.from_private_bytes(bytes([9] * 32))
check(
    "past the cap, a new name is refused",
    register(label=label_of(third), ctl=third)[0],
    429,
)
check("and its owner's own still moves", moved[0], 200)

if fails:
    print("\n".join(fails))
    sys.exit(1)
