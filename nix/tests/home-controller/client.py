# The installer's and the app's side of the door, run on the controller in
# the vm test: pairing over the house network with the card's pairing key,
# and a device signing in. The member is the mail Worker's made-up tester
# (client/mail/tests/tester.json), whose root and device key is the test key
# of 32 sevens. Prints what it got, as JSON.
#   client.py pair <tester.json> <pair-key-file> <domain>
#   client.py signin <tester.json> <door name> [seed]
import base64
import hashlib
import hmac
import json
import sys
import time
import urllib.error
import urllib.request

import nacl.bindings
import nacl.public
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


def key(seed):
    return Ed25519PrivateKey.from_private_bytes(bytes([seed] * 32))


def pub(k):
    raw = k.public_key().public_bytes(
        serialization.Encoding.Raw, serialization.PublicFormat.Raw
    )
    return base64.b64encode(raw).decode()


def mac(pk, body):
    m = hmac.new(pk, b"commonty controller pair v1\0" + body, hashlib.sha256)
    return base64.b64encode(m.digest()).decode()


def sealed(url, pk, msg):
    # the mac is over the very bytes sent, and so is the reply's
    body = json.dumps(msg).encode()
    req = urllib.request.Request(
        url,
        data=body,
        method="POST",
        headers={"content-type": "application/json", "x-commonty-mac": mac(pk, body)},
    )
    try:
        with urllib.request.urlopen(req) as r:
            got = r.read()
            if not hmac.compare_digest(
                r.headers.get("x-commonty-mac", ""), mac(pk, got)
            ):
                return 0, "the reply is not the controller's"
            return r.status, got.decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def call(url, body=None, headers=None):
    req = urllib.request.Request(
        url,
        data=None if body is None else json.dumps(body).encode(),
        method="GET" if body is None else "POST",
        headers=dict({"content-type": "application/json"}, **(headers or {})),
    )
    try:
        with urllib.request.urlopen(req) as r:
            return r.status, r.read().decode(), r.headers
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode(), e.headers


what = sys.argv[1]
tester = json.load(open(sys.argv[2]))
entry = tester["v3"]

if what == "pair":
    pk = open(sys.argv[3], "rb").read()
    domain = sys.argv[4]
    msg = {"entry": entry, "at": int(time.time())}
    code, body = sealed("http://127.0.0.1:8080/pair", pk, msg)
    if code != 200:
        print(json.dumps({"step": "pair", "code": code, "body": body}))
        sys.exit(1)
    paired = json.loads(body)
    name = "%s.home.%s" % (paired["label"], domain)
    sig = base64.b64encode(
        key(7).sign(
            (
                "commonty controller name v1\0%s\0%s" % (name, paired["controller"])
            ).encode()
        )
    ).decode()
    # sealed to the controller's key, as the installer seals it
    curve = nacl.bindings.crypto_sign_ed25519_pk_to_curve25519(
        base64.b64decode(paired["controller"])
    )
    # base64url, unpadded: the library's (client/library)
    network_key = (
        base64.urlsafe_b64encode(
            nacl.public.SealedBox(nacl.public.PublicKey(curve)).encrypt(
                b"test-network-key"
            )
        )
        .decode()
        .rstrip("=")
    )
    fin = {"network_key": network_key, "name_sig": sig, "domain": domain}
    code, body = sealed("http://127.0.0.1:8080/pair/finish", pk, fin)
    print(json.dumps({"step": "finish", "code": code, "body": body, "name": name}))
    sys.exit(0 if code == 200 else 1)

if what == "signin":
    name = sys.argv[3]
    seed = int(sys.argv[4]) if len(sys.argv) > 4 else 7
    host = {"Host": name}
    code, body, _ = call("http://127.0.0.1/_door/challenge", headers=host)
    nonce = json.loads(body)["nonce"]
    k = key(seed)
    sig = base64.b64encode(
        k.sign(
            b"commonty door v1\0" + name.encode() + b"\0" + nonce.encode() + b"\0tester"
        )
    ).decode()
    code, body, headers = call(
        "http://127.0.0.1/_door/signin",
        {
            "name": "tester",
            "entry": entry,
            "device": pub(k),
            "sig": sig,
            "nonce": nonce,
        },
        host,
    )
    cookie = headers.get("Set-Cookie", "").split(";")[0]
    print(json.dumps({"code": code, "body": body, "cookie": cookie}))
    sys.exit(0 if code == 200 else 1)
