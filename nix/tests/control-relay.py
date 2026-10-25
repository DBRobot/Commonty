# The relay without a working witness (nix/tests/control-relay.nix): run in
# the directory the derivation set up, with the bundle in w/.
import base64
import json
import os
import re
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

from cryptography.hazmat.primitives import serialization as S
from cryptography.hazmat.primitives.asymmetric import ec, ed25519

PORT = 8790
URL = f"http://127.0.0.1:{PORT}"
failed = False


def ok(cond, what):
    global failed
    print(("ok   " if cond else "FAIL ") + what, flush=True)
    failed |= not cond


def raw(k):
    return k.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)


def seed(k):
    return base64.b64encode(
        k.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption())
    ).decode()


# a fleet of two boxes that vote, and the witness (3)
vals = {i: ed25519.Ed25519PrivateKey.generate() for i in (1, 2, 3)}
cfg = {
    "boxes": {},
    "validators": [],
    "rules": {"leaseTtlMs": 15000, "goneAfterMs": 10000, "leases": {}},
}
for i, n in ((1, "node1"), (2, "node2")):
    k = ec.generate_private_key(ec.SECP256R1())
    spki = k.public_key().public_bytes(
        S.Encoding.DER, S.PublicFormat.SubjectPublicKeyInfo
    )
    cfg["boxes"][n] = {
        "id": 100 + i,
        "house": f"h{i}",
        "key": base64.b64encode(spki).decode(),
    }
for i, k in vals.items():
    cfg["validators"].append(
        {"id": i, "name": f"v{i}", "key": base64.b64encode(raw(k)).decode()}
    )
open("w/config.js", "w").write("export default " + json.dumps(cfg) + ";\n")


def get(path, timeout=10):
    try:
        with urllib.request.urlopen(URL + path, timeout=timeout) as r:
            return r.status, r.read().decode()
    except urllib.error.HTTPError as e:
        return e.code, e.read().decode()


def put(path, body):
    req = urllib.request.Request(URL + path, data=body.encode(), method="PUT")
    with urllib.request.urlopen(req, timeout=10) as r:
        return r.read().decode()


def health(o=""):
    t = time.time()
    c, b = get("/health" + (f"?o={o}" if o else ""))
    return c, (json.loads(b) if c == 200 else b), time.time() - t


def until(what, cond, secs=30):
    end = time.time() + secs
    while time.time() < end:
        try:
            if cond():
                return True
        except Exception:
            pass
        time.sleep(0.5)  # sleep-ok: the poll interval of until(), outside a vm
    ok(False, f"timed out: {what}")
    return False


def handshake(vid):
    # a validator's signed hello, and the relay's answer to the upgrade
    at = int(time.time())
    sig = vals[vid].sign(f"commonty relay v1\0{vid}\0{at}".encode())
    q = f"/relay?id={vid}&at={at}&sig={base64.urlsafe_b64encode(sig).decode().rstrip('=')}"
    s = socket.create_connection(("127.0.0.1", PORT), timeout=10)
    t = time.time()
    s.sendall(
        (
            f"GET {q} HTTP/1.1\r\nHost: 127.0.0.1\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
            "Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        ).encode()
    )
    line = s.recv(4096).split(b"\r\n")[0].decode()
    s.close()
    return line, time.time() - t


def relay(key):
    open("w/.dev.vars", "w").write(f"WITNESS_KEY={key}\n")
    log = open("wrangler.log", "a")
    p = subprocess.Popen(
        ["wrangler", "dev", "--port", str(PORT), "--ip", "127.0.0.1"],
        cwd="w",
        stdout=log,
        stderr=subprocess.STDOUT,
    )
    if not until("wrangler up", lambda: get("/nothing", 2)[0] == 404, 120):
        print(open("wrangler.log").read())
        sys.exit(1)
    return p


def stop(p):
    p.terminate()
    p.wait()


# a saved state that holds together nowhere: no quorum signed it
bad = json.dumps(
    {
        "before": {"height": 4},
        "proposal": {
            "Header": base64.b64encode(
                json.dumps({"h": 5, "t": 1, "b": "00", "d": "00"}).encode()
            ).decode(),
            "Payload": "",
            "Metadata": "",
            "VerificationSequence": 0,
        },
        "sigs": [],
    }
)

# ---- a witness that panics on boot: its key is no validator of the config
stranger = ed25519.Ed25519PrivateKey.generate()
p = relay(seed(stranger))
c, h, t = health()
ok(
    c == 200 and t < 5,
    f"/health answers at once though the witness cannot start ({c} {h}, {t:.1f} s)",
)
until("the witness given up", lambda: health()[1]["witnessError"], 10)
why = health()[1]["witnessError"]
ok(
    "is not a validator in the config" in why
    and base64.b64encode(raw(stranger)).decode() in why
    and seed(stranger) not in why,
    f"/health says why: the key's public half, never the key ({why!r})",
)
line, t = handshake(1)
ok(
    " 101 " in line and t < 5,
    f"a validator's /relay handshake is answered: {line!r} in {t:.1f} s",
)
ok(get("/leases")[0] == 503, "/leases is refused without a witness")
ok(get("/identity")[0] == 503, "/identity is refused without a witness")
# the same with what it saved: dropped for good, and the relay still answers
put("/test/saved?o=saved", bad)
c, h, t = health("saved")
ok(
    c == 200 and t < 5,
    f"/health answers with a saved state the witness panics on ({c} {h}, {t:.1f} s)",
)
until("the saved state dropped", lambda: get("/test/saved?o=saved")[1] == "", 15)
ok(
    get("/test/saved?o=saved")[1] == "",
    "the saved state it could not start from is gone",
)
ok(
    health("saved")[1]["witness"] is False and get("/leases?o=saved")[0] == 503,
    "and no witness answers for it",
)
stop(p)

# ---- the witness's own key: a fresh object starts it
p = relay(seed(vals[3]))
c, h, t = health()
ok(c == 200 and t < 5, f"/health answers while the witness starts ({c} {h}, {t:.1f} s)")
until("the witness up on a fresh object", lambda: health()[1]["witness"] is True)
c, b = get("/leases")
ok(c == 200 and "leases" in json.loads(b), f"/leases answers from the witness ({c})")
line, t = handshake(2)
ok(" 101 " in line, f"a validator's /relay handshake is answered: {line!r}")
# and one whose saved state does not hold together starts without it
put("/test/saved?o=stale", bad)
until(
    "the witness up despite its saved state",
    lambda: health("stale")[1]["witness"] is True,
)
ok(
    health("stale")[1]["height"] == 0,
    "it started from nothing, to catch up from the validators",
)
stop(p)

# ---- the Workers that bind the relay reach the object it serves
name = re.search(r"RELAY_NAME = '([^']+)'", open("w/worker.js").read()).group(1)
for f in ("dns.js", "identity.js"):
    src = open(f).read()
    m = re.search(r"const RELAY = '([^']+)'", src)
    ok(
        m and m.group(1) == name and "idFromName(RELAY)" in src,
        f"{f} binds the relay's object {name!r}",
    )

if failed:
    print("--- relay")
    print(open("wrangler.log").read()[-6000:])
    sys.exit(1)
