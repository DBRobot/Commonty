# Signing out everywhere across the fleet (docs/dev/identity-state.md,
# "Signing out everywhere"), with no network and no vm: three dd-control
# validators and the witness, a full gate on each box reading the agreed
# identity state. node3 starts on the dd-control from before the sign-out.
#
# dee signs in on every box. node2 goes off. She signs out everywhere from
# node1, keeping the session she asked from: node1 ends the rest; node3,
# on the old code, keeps deciding with the others and ignores it; node2,
# back, ends its sessions from before. node3 moves to this release and ends
# its own then, but not the one made there after the sign-out. Sent again,
# or an older one, changes nothing.
import base64
import hashlib
import json
import os
import subprocess
import time
import urllib.error
import urllib.request

from cryptography.hazmat.primitives import serialization as S
from cryptography.hazmat.primitives.asymmetric import ed25519 as ed

T = os.getcwd()
GROUP = subprocess.check_output(["id", "-gn"]).decode().strip()
failed = []


def ok(c, m):
    print(("PASS " if c else "FAIL ") + m, flush=True)
    if not c:
        failed.append(m)


def until(what, cond, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        if cond():
            return True
        time.sleep(0.5)  # sleep-ok: the poll of until, which is the wait
    ok(False, f"timed out: {what}")
    return False


for i in (1, 2, 3):
    json.dump({}, open(f"{T}/carries{i}.json", "w"))
    json.dump({"kvm": 0}, open(f"{T}/facts{i}.json", "w"))
with open(os.environ["PLACEMENT"]) as f:
    open(f"{T}/placement.json", "w").write(f.read())
os.makedirs(f"{T}/bin", exist_ok=True)
with open(f"{T}/bin/systemctl", "w") as f:
    f.write('#!/bin/sh\necho "$@" >> "$SYSTEMCTL_LOG"\n')
os.chmod(f"{T}/bin/systemctl", 0o755)


def control(i, binary):
    env = dict(
        os.environ,
        PATH=f"{T}/bin:" + os.environ["PATH"],
        SYSTEMCTL_LOG=f"{T}/systemctl{i}.log",
    )
    return subprocess.Popen(
        [
            binary,
            "-config",
            f"{T}/config.json",
            "-box",
            f"node{i}",
            "-relay",
            "ws://127.0.0.1:8789/relay",
            "-state",
            f"{T}/st{i}",
            "-box-key",
            f"{T}/node{i}.p8",
            "-listen",
            f"127.0.0.1:419{i}",
            "-heartbeat",
            "1s",
            "-submit",
            f"{T}/s{i}.sock",
            "-submit-group",
            GROUP,
            "-builds",
            f"{T}/b{i}.sock",
            "-placement",
            f"{T}/placement.json",
            "-placed-carries",
            f"{T}/carries{i}.json",
            "-placed-dir",
            f"{T}/placed{i}",
            "-test-facts",
            f"{T}/facts{i}.json",
            "-metrics",
            f"{T}/control{i}.prom",
        ],
        env=env,
        stdout=open(f"{T}/n{i}.log", "a"),
        stderr=subprocess.STDOUT,
    )


def b64(b):
    return base64.b64encode(b).decode()


def pub(k):
    return b64(k.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw))


dee = ed.Ed25519PrivateKey.from_private_bytes(open(f"{T}/dee.key", "rb").read())
DEE = hashlib.sha256(pub(dee).encode()).hexdigest()
dev = ed.Ed25519PrivateKey.generate()
FP = base64.b64decode(pub(dev))[:6].hex()


def gate(i):
    os.makedirs(f"{T}/g{i}/keys", exist_ok=True)
    env = dict(
        os.environ,
        VERIFY_DIR=f"{T}/g{i}/keys",
        VERIFY_BIND=f"127.0.0.1:428{i}",
        VERIFY_DOMAIN="commonty.test",
        VERIFY_MEMBERS=json.dumps({"members": [DEE], "revoked": []}),
        VERIFY_PAGES_DIR=os.environ["PAGES"],
        VERIFY_CONTROL=f"http://127.0.0.1:419{i}",
        VERIFY_CONTROL_SUBMIT=f"{T}/s{i}.sock",
        VERIFY_CONTROL_CONFIG=f"{T}/config.json",
        VERIFY_IDENTITY_FROM="agreed",
        VERIFY_BOX=f"node{i}",
    )
    return subprocess.Popen(
        [f"{T}/verify"],
        env=env,
        stdout=open(f"{T}/g{i}.log", "a"),
        stderr=subprocess.STDOUT,
    )


def http(method, url, body=None, headers=None, form=None, timeout=30):
    data = None
    h = dict(headers or {})
    if body is not None:
        data = json.dumps(body).encode()
        h["content-type"] = "application/json"
    if form is not None:
        data = form.encode()
        h["content-type"] = "application/x-www-form-urlencoded"

    class Keep(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, *a, **k):
            return None

    req = urllib.request.Request(url, method=method, data=data, headers=h)
    try:
        r = urllib.request.build_opener(Keep).open(req, timeout=timeout)
        return r.status, r.read(), r.headers
    except urllib.error.HTTPError as e:
        return e.code, e.read(), e.headers
    except Exception as e:
        return 0, str(e).encode(), {}


def identity(i):
    c, b, _ = http("GET", f"http://127.0.0.1:419{i}/identity", timeout=5)
    return json.loads(b) if c == 200 else {}


def height(i):
    try:
        return json.load(
            urllib.request.urlopen(f"http://127.0.0.1:419{i}/state", timeout=3)
        ).get("height", 0)
    except Exception:
        return 0


def mint():
    exp = int(time.time()) + 3600
    hexkey = dev.private_bytes(
        S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption()
    ).hex()
    return (
        subprocess.check_output(
            ["biscuit", "generate", "--private-key", "ed25519-private/" + hexkey, "-"],
            input=f'user("dee");\ndevice("{FP}");\ncheck if time($t), $t < {exp};\n'.encode(),
        )
        .decode()
        .strip()
    )


def sign_in(i):
    """a session on box i, as the app's window makes one: (cookie, its id)"""
    c, _, h = http(
        "POST", f"http://127.0.0.1:428{i}/_dd/app/signin", form=f"token={TOKEN}"
    )
    set_cookie = (h.get("set-cookie") or "") if h else ""
    if c not in (302, 303, 307) or not set_cookie:
        ok(False, f"signed in on node{i} ({c})")
        return "", ""
    cookie = set_cookie.split(";")[0]
    return cookie, cookie.split("~", 1)[1].split(".", 1)[0]


def live(i, cookie):
    return (
        http("GET", f"http://127.0.0.1:428{i}/_dd/signins", headers={"cookie": cookie})[
            0
        ]
        == 200
    )


def seal(kind, value, key):
    body = json.dumps(value, separators=(",", ":")).encode()
    sig = key.sign(f"commonty {kind} v2\0".encode() + body)
    return {"v": 2, "kind": kind, "body": b64(body), "sig": b64(sig)}


def put_signout(i, env):
    return http(
        "PUT",
        f"http://127.0.0.1:428{i}/_dd/boxes/signout",
        body=env,
        headers={"authorization": f"Bearer {TOKEN}"},
    )


def dee_of(i):
    return identity(i).get("members", {}).get("dee", {})


# ---- the fleet up, node3 on the old dd-control
procs = {
    1: control(1, f"{T}/dd-control"),
    2: control(2, f"{T}/dd-control"),
    3: control(3, f"{T}/old/dd-control"),
}
until("the validators deciding", lambda: all(height(i) > 3 for i in (1, 2, 3)), 90)
gates = {i: gate(i) for i in (1, 2, 3)}
until(
    "the gates up",
    lambda: all(
        http("GET", f"http://127.0.0.1:428{i}/_dd/signins")[0] == 401 for i in (1, 2, 3)
    ),
)

entry = {
    "name": "dee",
    "root": pub(dee),
    "recovery": pub(ed.Ed25519PrivateKey.generate()),
    "devices": [{"fingerprint": FP, "public_key": pub(dev), "added": 1}],
    "terms": "2026-10-11",
    "version": 1,
    "updated": int(time.time()),
}
signed = {
    "entry": entry,
    "signature": b64(dee.sign(json.dumps(entry, separators=(",", ":")).encode())),
}
c, b, _ = http(
    "PUT", "http://127.0.0.1:4281/_dd/directory/dee", body=signed, timeout=70
)
ok(c == 200, f"dee's name agreed ({c} {b[:120]!r})")
until(
    "dee a member on every box",
    lambda: all(dee_of(i).get("status") == "member" for i in (1, 2, 3)),
    60,
)
TOKEN = mint()

# ---- signed in on every box; node2 off
s = {}
until(
    "a session on every box",
    lambda: all(
        http("POST", f"http://127.0.0.1:428{i}/_dd/app/signin", form=f"token={TOKEN}")[
            0
        ]
        in (302, 303)
        for i in (1, 2, 3)
    ),
    60,
)
for i in (1, 2, 3):
    s[i] = sign_in(i)
kept = sign_in(1)
ok(
    all(live(i, s[i][0]) for i in (1, 2, 3)) and live(1, kept[0]),
    "dee is signed in on every box",
)

for p in (gates[2], procs[2]):
    p.terminate()
    p.wait()
before = time.time()
until("a second past the sessions", lambda: time.time() >= before + 1.5, 5)

# ---- she signs out everywhere from node1, keeping that browser's session
forged = seal(
    "signout", {"member": DEE, "at": int(time.time())}, ed.Ed25519PrivateKey.generate()
)
ok(
    put_signout(1, forged)[0] == 403,
    "a sign-out not signed by her root is refused at the gate",
)

at = int(time.time())
env = seal("signout", {"member": DEE, "at": at, "keep": kept[1]}, dee)
c, b, _ = put_signout(1, env)
ok(c == 202, f"her sign-out is taken ({c} {b[:120]!r})")
item = json.loads(b).get("item", "") if c == 202 else ""
until("node1 has it", lambda: dee_of(1).get("signed_out", 0) > 0, 30)
ok(
    dee_of(1).get("signed_out") == at * 1000
    and dee_of(1).get("signed_out_keep") == kept[1],
    "node1's /identity has her sign-out, and the session it keeps",
)
until("node1's session ends", lambda: not live(1, s[1][0]), 30)
ok(not live(1, s[1][0]), "node1: the session from before is over")
ok(live(1, kept[0]), "node1: the session she asked from is kept")
after1 = sign_in(1)
ok(live(1, after1[0]), "node1: signing in again after it works")

# ---- node3, on the old code: decides with the rest and ignores it
h = height(3)
until("node3 keeps deciding", lambda: height(3) > h + 3 and height(1) > h + 3, 30)
ok(height(3) > h + 3, "node3 on the old dd-control keeps deciding with the others")
v3 = dee_of(3)
ok("signed_out" not in v3, "node3's old judge has no sign-out")
outcome = identity(3).get("outcomes", {}).get(item, "")
ok(
    "no such item" in outcome,
    f"node3's old judge refused it as an item it does not know ({outcome!r})",
)
v1 = dict(dee_of(1))
v1.pop("signed_out", None)
v1.pop("signed_out_keep", None)
ok(
    {k: v for k, v in v1.items() if k != "agreed"}
    == {k: v for k, v in v3.items() if k != "agreed"},
    "and holds dee the same otherwise: entry, status, access",
)
ok(live(3, s[3][0]), "node3: her session there goes on, as before")
late3 = sign_in(3)
ok(live(3, late3[0]), "node3: a session made there after the sign-out")

# ---- node2 back: it ends its session from before, not one made since
procs[2] = control(2, f"{T}/dd-control")
gates[2] = gate(2)
until("node2 has the sign-out", lambda: dee_of(2).get("signed_out") == at * 1000, 90)
until("node2's old session ends", lambda: not live(2, s[2][0]), 30)
ok(not live(2, s[2][0]), "node2, back from being off: the session from before is over")
after2 = sign_in(2)
ok(live(2, after2[0]), "node2: a session made after it stands")

# ---- node3 moves to this release: its judge replays the log
procs[3].terminate()
procs[3].wait()
procs[3] = control(3, f"{T}/dd-control")
until("node3 has the sign-out", lambda: dee_of(3).get("signed_out") == at * 1000, 90)
until("node3's old session ends", lambda: not live(3, s[3][0]), 30)
ok(not live(3, s[3][0]), "node3, on this release: the session from before is over")
ok(live(3, late3[0]), "node3: the session made after the sign-out stands")
ok(
    len({dee_of(i).get("signed_out") for i in (1, 2, 3)}) == 1,
    "every box holds the same sign-out",
)

# ---- replay can't undo it
c, _, _ = put_signout(2, env)
ok(c in (202, 409), f"the same sign-out again ({c})")
older = seal("signout", {"member": DEE, "at": at - 60}, dee)
c, b, _ = put_signout(2, older)
ok(c == 202, f"an older one is handed in ({c})")
older_item = json.loads(b).get("item", "") if c == 202 else ""
until("the older one judged", lambda: older_item in identity(1).get("outcomes", {}), 30)
ok("older" in identity(1).get("outcomes", {}).get(older_item, ""), "and refused")
ok(
    all(
        dee_of(i).get("signed_out") == at * 1000
        and dee_of(i).get("signed_out_keep") == kept[1]
        for i in (1, 2, 3)
    ),
    "the sign-out on every box is as it was",
)
ok(
    live(1, kept[0])
    and live(1, after1[0])
    and live(2, after2[0])
    and live(3, late3[0]),
    "and every session since, and the kept one, still stand",
)
ok(not any(live(i, s[i][0]) for i in (1, 2, 3)), "and none from before came back")

for p in list(gates.values()) + list(procs.values()):
    p.terminate()
if failed:
    raise SystemExit(f"{len(failed)} failed: {failed}")
print("signout: all passed")
