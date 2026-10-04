# The control plane end to end, with no network and no vm: the relay and
# witness under wrangler (workerd), three dd-control validators with test
# box keys, agreeing; one killed and the rest carrying on; it restarted and
# catching up from its log; a hello signed by the wrong key refused. Then
# a gate on each box with the directory as the agreed log: a new name is
# its claimant's once every voting box's gate has acknowledged it, and not
# before; an update by its owner reaches every box.
import json, os, subprocess, sys, time, urllib.request, base64

T = os.getcwd()
GROUP = subprocess.check_output(["id", "-gn"]).decode().strip()
failed = []


def ok(c, m):
    print(("PASS " if c else "FAIL ") + m, flush=True)
    if not c:
        failed.append(m)


def state(i):
    try:
        return json.load(
            urllib.request.urlopen(f"http://127.0.0.1:419{i}/state", timeout=3)
        )
    except Exception:
        return {}


def run(i):
    return subprocess.Popen(
        [
            f"{T}/dd-control",
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
            "-roles",
            f"r{i}",
            "-submit",
            f"{T}/s{i}.sock",
            "-submit-group",
            GROUP,
        ],
        stdout=open(f"{T}/n{i}.log", "a"),
        stderr=subprocess.STDOUT,
    )


def until(what, cond, timeout=60):
    end = time.time() + timeout
    while time.time() < end:
        if cond():
            return True
        time.sleep(0.5)
    ok(False, f"timed out: {what}")
    return False


procs = {i: run(i) for i in (1, 2, 3)}
agree = lambda ids, h: (
    all(state(i).get("height", 0) >= h for i in ids)
    and len({json.dumps(state(i).get("boxes", {}).keys().__len__()) for i in ids}) == 1
)
if until(
    "first agreement",
    lambda: all(len(state(i).get("boxes", {})) == 3 for i in (1, 2, 3)),
):
    s = [state(i) for i in (1, 2, 3)]
    ok(True, f"three validators and the witness agree, height {s[0]['height']}")
    ok(
        all(x["boxes"]["node2"]["roles"] == ["r2"] for x in s),
        "each box's heartbeat and roles are in the agreed state",
    )
    h = json.load(urllib.request.urlopen("http://127.0.0.1:8789/health"))
    ok(h["witness"] and h["connected"] == 3, f"relay health {h}")

procs[1].terminate()
procs[1].wait()
h0 = state(2).get("height", 0)
until(
    "the others carry on without node1",
    lambda: state(2).get("height", 0) > h0 + 3 and state(3).get("height", 0) > h0 + 3,
)
ok(
    state(2).get("height", 0) > h0 + 3,
    "with one validator down the other two and the witness carry on",
)

procs[1] = run(1)
until(
    "node1 catches up",
    lambda: (
        state(1).get("height", 0) >= state(2).get("height", 0) - 1
        and state(1).get("height", 0) > h0 + 3
    ),
)
ok(
    state(1).get("height", 0) > h0 + 3,
    "a restarted validator replays its log and catches up",
)

req = urllib.request.Request(
    f"http://127.0.0.1:8789/relay?id=1&at={int(time.time())}&sig=AAAA",
    headers={
        "Upgrade": "websocket",
        "Connection": "Upgrade",
        "Sec-WebSocket-Version": "13",
        "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
    },
)
try:
    urllib.request.urlopen(req)
    code = 101
except urllib.error.HTTPError as e:
    code = e.code
ok(code == 403, f"a hello not signed by that address's key is refused ({code})")

# ---- the directory as the agreed log, through each box's gate
from cryptography.hazmat.primitives.asymmetric import ed25519 as ed
from cryptography.hazmat.primitives import serialization as S


def gate(i):
    os.makedirs(f"{T}/g{i}", exist_ok=True)
    env = dict(
        os.environ,
        VERIFY_ROLE="directory",
        VERIFY_DIR=f"{T}/g{i}/keys",
        VERIFY_BIND=f"127.0.0.1:428{i}",
        VERIFY_CONTROL=f"http://127.0.0.1:419{i}",
        VERIFY_CONTROL_SUBMIT=f"{T}/s{i}.sock",
        VERIFY_CONTROL_CONFIG=f"{T}/config.json",
        VERIFY_BOX=f"node{i}",
    )
    return subprocess.Popen(
        [f"{T}/verify"],
        env=env,
        stdout=open(f"{T}/g{i}.log", "a"),
        stderr=subprocess.STDOUT,
    )


def http(method, url, body=None, timeout=70):
    req = urllib.request.Request(
        url,
        method=method,
        data=None if body is None else json.dumps(body).encode(),
        headers={"content-type": "application/json"},
    )
    try:
        r = urllib.request.urlopen(req, timeout=timeout)
        return r.status, r.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read()
    except Exception as e:
        return 0, str(e).encode()


b64 = lambda b: base64.b64encode(b).decode()
pub = lambda k: b64(k.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw))


# one paper key per person, the same in every version (a new one is a
# recovery-key swap, which only the old paper key may sign)
RECOVERY = {}


def signed(name, key, version):
    dev = ed.Ed25519PrivateKey.generate()
    entry = {
        "name": name,
        "root": pub(key),
        "recovery": RECOVERY.setdefault(name, pub(ed.Ed25519PrivateKey.generate())),
        "devices": [
            {
                "fingerprint": base64.b64decode(pub(dev))[:6].hex(),
                "public_key": pub(dev),
                "added": 1,
            }
        ],
        "version": version,
        "updated": int(time.time()),
    }
    sig = key.sign(json.dumps(entry, separators=(",", ":")).encode())
    return {"entry": entry, "signature": b64(sig)}


def entry_on(i, name):
    c, b = http("GET", f"http://127.0.0.1:428{i}/_dd/directory/{name}", timeout=5)
    return json.loads(b) if c == 200 else None


gates = {i: gate(i) for i in (1, 2, 3)}
until(
    "the gates up",
    lambda: all(
        http("GET", f"http://127.0.0.1:428{i}/_dd/directory", timeout=3)[0] == 200
        for i in (1, 2, 3)
    ),
)

ann = ed.Ed25519PrivateKey.generate()
c, b = http("PUT", "http://127.0.0.1:4281/_dd/directory/ann", signed("ann", ann, 1))
ok(
    c == 200,
    f"a new name, put to one gate, is agreed: every voting box's gate acknowledged it ({c} {b[:120]!r})",
)
until("ann on every box", lambda: all(entry_on(i, "ann") for i in (1, 2, 3)), 30)
ok(
    all(
        (entry_on(i, "ann") or {}).get("entry", {}).get("root") == pub(ann)
        for i in (1, 2, 3)
    ),
    "every box's directory has the same entry",
)

c, b = http("PUT", "http://127.0.0.1:4282/_dd/directory/ann", signed("ann", ann, 2))
ok(c == 200, f"its owner's update, put to another box, lands ({c} {b[:120]!r})")
until(
    "version 2 on every box",
    lambda: all(
        (entry_on(i, "ann") or {}).get("entry", {}).get("version") == 2
        for i in (1, 2, 3)
    ),
    30,
)
ok(
    all(
        (entry_on(i, "ann") or {}).get("entry", {}).get("version") == 2
        for i in (1, 2, 3)
    ),
    "the update reached every box",
)

eve = ed.Ed25519PrivateKey.generate()
c, _ = http("PUT", "http://127.0.0.1:4283/_dd/directory/ann", signed("ann", eve, 3))
ok(c in (403, 409), f"someone else's update to ann is refused ({c})")

# a voting box's gate down: a new name waits, it is nobody's, and when the
# gate is back it is agreed
gates[3].terminate()
gates[3].wait()
bob = ed.Ed25519PrivateKey.generate()
c, b = http("PUT", "http://127.0.0.1:4281/_dd/directory/bob", signed("bob", bob, 1))
ok(
    c == 202,
    f"with a voting box's gate down, a new name is sent and not yet anyone's ({c} {b[:120]!r})",
)
ok(entry_on(1, "bob") is None and entry_on(2, "bob") is None, "and no box holds it")
gates[3] = gate(3)
until(
    "bob agreed once every gate is back",
    lambda: all(entry_on(i, "bob") for i in (1, 2, 3)),
    60,
)
ok(
    all(entry_on(i, "bob") for i in (1, 2, 3)),
    "the gate back, the name is agreed on every box",
)
for g in gates.values():
    g.terminate()

for p in procs.values():
    p.terminate()
if failed:
    sys.exit(f"{len(failed)} failed")
print("all passed")
