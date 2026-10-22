# The control plane end to end, with no network and no vm: the relay and
# witness under wrangler (workerd), three dd-control validators with test
# box keys, agreeing; one killed and the rest carrying on; it restarted and
# catching up from its log; a hello signed by the wrong key refused. Then
# a gate on each box with the directory as the agreed log: a new name is
# its claimant's once every voting box's gate has acknowledged it, and not
# before; an update by its owner reaches every box. Last, a placed
# service: on one of the two boxes that carry it, and on the other once
# that one is gone. And a writer, which only the lease's holder runs, and
# another box only once that lease has lapsed.
import json, os, subprocess, sys, time, urllib.request, urllib.parse, urllib.error, base64

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


# what the fleet wants: one "web", which node2 and node3 carry (their
# carries file) and node1 does not; systemctl a stand-in that writes down
# what it was asked
# (control/placement.json, $PLACEMENT; the relay's witness plans with it too)
with open(os.environ["PLACEMENT"]) as f:
    open(f"{T}/placement.json", "w").write(f.read())
for i in (2, 3):
    json.dump(
        {"web": ["web.service"], "db": ["db.service"]},
        open(f"{T}/carries{i}.json", "w"),
    )
# what a host of computers says of itself (computer-facts.sh), as each box
# would: KVM on x86_64, room for them
for i in (1, 2, 3):
    json.dump(
        {
            "kvm": 1,
            "x86_64": 1,
            "computer_max_mem_bytes": 16 * 2**30,
            "computer_max_threads": 7,
            "computer_max_disk_bytes": 400 * 2**30,
        },
        open(f"{T}/facts{i}.json", "w"),
    )
os.makedirs(f"{T}/bin", exist_ok=True)
with open(f"{T}/bin/systemctl", "w") as f:
    f.write('#!/bin/sh\necho "$@" >> "$SYSTEMCTL_LOG"\n')
os.chmod(f"{T}/bin/systemctl", 0o755)


def run(i):
    env = dict(
        os.environ,
        PATH=f"{T}/bin:" + os.environ["PATH"],
        SYSTEMCTL_LOG=f"{T}/systemctl{i}.log",
    )
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
        ],
        env=env,
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
        all(
            x["boxes"]["node2"].get("facts") and not x["boxes"]["node2"].get("roles")
            for x in s
        ),
        "each box's heartbeat is in the agreed state, and names no roles",
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
        # the terms agreed to at sign-up, in the entry's own order
        "terms": "2026-10-11",
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
        # the list is members' (401 to this script): up is answering at all
        http("GET", f"http://127.0.0.1:428{i}/_dd/directory", timeout=3)[0] == 401
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

# ---- the agreed identity state: judged on each box from the same log


def identity(i, q=""):
    c, b = http("GET", f"http://127.0.0.1:419{i}/identity{q}", timeout=5)
    return c, (json.loads(b) if c == 200 else None)


def ann_v2(i):
    c, v = identity(i)
    return c == 200 and v["members"].get("ann", {}).get("version") == 2


until(
    "ann's version 2 judged on every box", lambda: all(ann_v2(i) for i in (1, 2, 3)), 30
)
views = [identity(i)[1] for i in (1, 2, 3)]
anns = [v["members"]["ann"] for v in views if v]
ok(
    len(anns) == 3
    and all(a == anns[0] for a in anns)
    and anns[0]["root"] == pub(ann)
    and anns[0]["status"] == "none",
    "every box's /identity has ann the same: version 2, her root, not let in",
)
ok(
    all(set(v["members"]) == {"ann"} for v in views if v),
    "and nobody else: eve's update was no one's",
)
c, one = identity(1, "?name=ann")
ok(c == 200 and one["member"]["version"] == 2, f"?name= answers for one ({c})")
c, _ = identity(1, f"?since={views[0]['changed']}")
ok(c == 304, f"?since= the last change: 304 ({c})")

# ---- build results: handed in on one box's socket, in the agreed log on all


def built_post(i, body):
    import socket

    c = socket.socket(socket.AF_UNIX)
    c.connect(f"{T}/b{i}.sock")
    c.sendall(
        b"POST /built HTTP/1.0\r\nContent-Type: application/json\r\n"
        + f"Content-Length: {len(body)}\r\n\r\n".encode()
        + body
    )
    answer = b""
    while chunk := c.recv(4096):
        answer += chunk
    return int(answer.split()[1])


def built(i):
    try:
        return json.load(
            urllib.request.urlopen(f"http://127.0.0.1:419{i}/built?since=0", timeout=3)
        )["items"]
    except Exception:
        return []


result = json.dumps(
    {"statement": '{"order":"o1","job":"linux"}', "signature": "s"}
).encode()
ok(built_post(2, result) == 202, "a build result is taken on the box's socket")
ok(built_post(2, b"{nope") == 400, "and anything that is not json is not")
until(
    "the result is in every box's log",
    lambda: all(
        [(x["box"], x["item"]["statement"]) for x in built(i)]
        == [("node2", '{"order":"o1","job":"linux"}')]
        for i in (1, 2, 3)
    ),
)
ok(
    all(len(built(i)) == 1 and built(i)[0]["box"] == "node2" for i in (1, 2, 3)),
    "a build result reaches the agreed log on every box, as the box that sent it",
)
ok(
    all(x["boxes"]["node2"]["facts"].get("cores", 0) > 0 for x in [state(1)]),
    "heartbeats still carry the box's facts",
)

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


# ---- a member's remote computer: judged from the log, placed, run under
# its lease on a box placement may give it, and stopped when powered off
def submit(i, item):
    import socket

    body = json.dumps(item).encode()
    c = socket.socket(socket.AF_UNIX)
    c.connect(f"{T}/s{i}.sock")
    c.sendall(
        b"POST /submit HTTP/1.0\r\nContent-Type: application/json\r\n"
        + f"Content-Length: {len(body)}\r\n\r\n".encode()
        + body
    )
    answer = b""
    while chunk := c.recv(4096):
        answer += chunk
    return int(answer.split()[1])


cara = ed.Ed25519PrivateKey.from_private_bytes(open(f"{T}/cara.key", "rb").read())


def seal(kind, value):
    body = json.dumps(value, separators=(",", ":")).encode()
    sig = cara.sign(f"commonty {kind} v2\0".encode() + body)
    return {"v": 2, "kind": kind, "body": b64(body), "sig": b64(sig)}


c, b = http("PUT", "http://127.0.0.1:4282/_dd/directory/cara", signed("cara", cara, 1))
ok(c == 200, f"cara's name agreed ({c} {b[:120]!r})")


def cara_member(i):
    c, v = identity(i)
    return c == 200 and v["members"].get("cara", {}).get("status") == "member"


until("cara a member on every box", lambda: all(cara_member(i) for i in (1, 2, 3)), 30)

KEY = (
    "ssh-ed25519 "
    + b64(b"\0\0\0\x0bssh-ed25519\0\0\0\x20" + bytes(32))
    + " cara@laptop"
)


def computer(cid, name, consent, version=1):
    return {
        "owner": "cara",
        "id": cid,
        "name": name,
        "config": "default",
        "memory": 2048,
        "threads": 2,
        "disk": 32,
        "nix": "",
        "ports": [],
        "players": [],
        "keys": [KEY],
        "consent": consent,
        "anywhere": consent == "any",
        "version": version,
    }


def computers(i):
    c, b = http("GET", f"http://127.0.0.1:419{i}/computers", timeout=5)
    return json.loads(b)["computers"] if c == 200 else {}


OWN = "00000000000000c3"
ok(
    submit(1, seal("computer", computer(OWN, "dev", "own"))) == 202,
    "cara's computer handed in",
)
until("it is agreed up", lambda: computers(2).get(OWN, {}).get("up") is True, 30)
ok(
    computers(3).get(OWN, {}).get("keys") == [KEY],
    "agreed with the key her device made, on every box",
)

comp_flag = lambda i, cid: f"{T}/placed{i}/computer-{cid}"
lease_of = lambda i, cid: state(i).get("leases", {}).get(f"svc/computer-{cid}", {})
until(
    "it runs on cara's box under its lease",
    lambda: (
        os.path.exists(comp_flag(3, OWN)) and lease_of(1, OWN).get("holder") == "node3"
    ),
    90,
)
ok(
    os.path.exists(comp_flag(3, OWN))
    and json.load(open(comp_flag(3, OWN)))["epoch"] == lease_of(1, OWN).get("epoch"),
    "cara's own box holds its lease and runs it at that epoch",
)
# dd-control raises the flag, then starts the unit (its condition is the
# flag): wait for the start, not just the flag
started = lambda i, unit: (
    f"start --no-block {unit}\n" in open(f"{T}/systemctl{i}.log").readlines()
)
until("its unit started", lambda: started(3, f"dd-computer@{OWN}.service"), 30)
ok(
    started(3, f"dd-computer@{OWN}.service"),
    "and started its unit",
)
time.sleep(11)  # sleep-ok: never-runs check, two placement rounds
ok(
    not os.path.exists(comp_flag(2, OWN))
    and f"dd-computer@{OWN}" not in open(f"{T}/systemctl2.log").read(),
    "a host that is not cara's never runs her my-boxes-only computer",
)

off = {"id": OWN, "up": False, "at": int(time.time())}
ok(submit(2, seal("computer-power", off)) == 202, "powered off")
until(
    "it stops and its lease goes back",
    lambda: not os.path.exists(comp_flag(3, OWN)) and not lease_of(1, OWN),
    60,
)
ok(
    f"stop dd-computer@{OWN}.service\n" in open(f"{T}/systemctl3.log").readlines()
    and not os.path.exists(comp_flag(3, OWN))
    and not lease_of(1, OWN),
    "powered off: stopped, waited for, its lease given back",
)

for g in gates.values():
    g.terminate()

# ---- a placed service: switched on where the plan puts it, and moved
# when that box is gone
placed = lambda i: os.path.exists(f"{T}/placed{i}/web")
until("web placed", lambda: placed(2) != placed(3), 60)
ok(placed(2) != placed(3) and not placed(1), "web runs on one box that carries it")
first = 2 if placed(2) else 3
other = 5 - first


# the relay's agreed placement, which the DNS Worker points names by: the
# witness's own plan on the agreed state, the box the boxes' plans agree on
def relay_placed():
    try:
        return json.load(
            urllib.request.urlopen("http://127.0.0.1:8789/leases", timeout=3)
        ).get("placed", {})
    except Exception:
        return {}


until(
    "the relay's placement has web",
    lambda: relay_placed().get("web") == [f"node{first}"],
    30,
)
ok(
    relay_placed().get("web") == [f"node{first}"] and "db" in relay_placed(),
    "the relay's agreed placement is the plan's",
)
until("its units started", lambda: started(first, "web.service"), 30)
ok(
    started(first, "web.service"),
    "its box started its units",
)
ok(
    all(
        state(i)["boxes"][f"node{first}"]["facts"].get("runs/web") == 1
        for i in (1, 2, 3)
    ),
    "and says so in its heartbeat",
)
procs[first].terminate()
procs[first].wait()
until("web moves", lambda: placed(other), 60)
ok(placed(other), f"node{first} gone, web runs on node{other}")

# the writer: placed where it was, it runs only where the lease is
procs[first] = run(first)
writer = lambda i: os.path.exists(f"{T}/placed{i}/db")
flag = lambda i: json.load(open(f"{T}/placed{i}/db"))


def holding():
    for i in (2, 3):
        lease = state(i).get("leases", {}).get("svc/db", {})
        if lease.get("holder") == f"node{i}" and writer(i):
            if flag(i).get("epoch") == lease["epoch"]:
                return i, lease
    return None, {}


until("db held and running", lambda: holding()[0], 90)
holder, lease = holding()
ok(holder is not None, f"db runs on the lease's holder, at its epoch ({lease})")
holder = holder or 2
ok(not writer(5 - holder), "and nowhere else")
gone = flag(holder)
procs[holder].terminate()
procs[holder].wait()
other = 5 - holder
until("db moves", lambda: writer(other), 90)
ok(writer(other), f"node{holder} gone, db runs on node{other}")
ok(
    float(open("/proc/uptime").read().split()[0]) * 1000 >= gone["until"]
    and flag(other)["epoch"] > gone["epoch"],
    "only after the lease it had could have lapsed, at a higher epoch",
)

# A box the release does not list joins on its own key, under the name its
# key gives (state/join.go), and its heartbeats count from then. A hello
# for its address signed by any other key is refused.
import hashlib
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives import serialization as S

sk = ec.generate_private_key(ec.SECP256R1())
open(f"{T}/stranger.p8", "wb").write(
    sk.private_bytes(S.Encoding.DER, S.PrivateFormat.PKCS8, S.NoEncryption())
)
der = sk.public_key().public_bytes(S.Encoding.DER, S.PublicFormat.SubjectPublicKeyInfo)
spki = base64.b64encode(der).decode()
open(f"{T}/stranger.spki", "w").write(spki + "\n")
digest = hashlib.sha256(der).digest()
sname = "j" + digest[:5].hex()
sid = (1 << 40) | int.from_bytes(digest[:5], "big")
# it came with a join ticket (biscuit-cli's, state/ident ticket_test.go),
# which stays on the box: the join carries its claim, and the agreed state
# its id. Its own report says it meets the hardware minimum
TICKET = (
    "EqMCCrgBCgNhbm4KBmRldmljZQoBZgoGdGlja2V0CiAwMDExMjIzMzQ0NTU2Njc3ODg5OWFhYmJjY2RkZWVmZgoHcHVycG9zZQoE"
    "am9pbgoBdBgDIgkKBwgKEgMYgAgiCgoICIEIEgMYgggiCgoICIMIEgMYhAgiCgoICIUIEgMYhggyKAomCgIIGxIHCAUSAwiHCBoX"
    "CgUKAwiHCAoICgYQwOWy1gYKBBoCCAAyDwoNCgIIGxIHCAMSAxiGCBIkCAASIKbGi1Im0RWOiZK_7j48dq3cqzLyqaZjCtsO0C0z"
    "lBicGkCt620JFzrYN2cTNjm0sU3-FPj5sBM1s7q5nyEqZTVhqC2MxgP2nvit0QI7J5XDkTjFK5SWGgVvTQIedukXI3kHIiIKIPiU"
    "raeqCqTMMTgjg1f-AJ13kTHwuDXDeVacBd7SkdXX"
)
open(f"{T}/ticket", "w").write(TICKET + "\n")
json.dump(
    {
        "firmware": {"uefi": True, "secureBootCapable": True, "tpm": {"version": 2}},
        "cpu": {"threads": 4},
        "memoryBytes": 8 << 30,
        "bootDisk": "/dev/disk/by-id/sys",
        "disks": [{"id": "/dev/disk/by-id/sys", "bytes": 256e9}],
        "nics": [{"name": "eth0"}],
    },
    open(f"{T}/report.json", "w"),
)
stranger = subprocess.Popen(
    [
        f"{T}/dd-control",
        "-config",
        f"{T}/config.json",
        "-box",
        sname,
        "-join-key",
        f"{T}/stranger.spki",
        "-join-recipient",
        "age1stranger",
        "-join-house",
        "hx",
        "-join-ticket-file",
        f"{T}/ticket",
        "-report",
        f"{T}/report.json",
        "-relay",
        "ws://127.0.0.1:8789/relay",
        "-state",
        f"{T}/st9",
        "-box-key",
        f"{T}/stranger.p8",
        "-listen",
        "127.0.0.1:4199",
        "-heartbeat",
        "1s",
        "-placement",
        f"{T}/placement.json",
    ],
    stdout=open(f"{T}/n9.log", "a"),
    stderr=subprocess.STDOUT,
)
live = [i for i in (1, 2, 3) if procs[i].poll() is None]
until(
    "the stranger joined",
    lambda: sname in state(live[0]).get("joined", {}),
    90,
)
j = state(live[0]).get("joined", {}).get(sname, {})
ok(
    j.get("key") == spki and j.get("id") == sid,
    f"a stranger joined as {sname}, on its key",
)
ok(
    j.get("ticket") == "00112233445566778899aabbccddeeff",
    "the id of the ticket it came with is kept",
)
ok(
    TICKET[-40:] not in json.dumps(state(live[0])),
    "and the ticket itself is nowhere in the agreed state",
)
seen = lambda: state(live[0]).get("boxes", {}).get(sname, {}).get("lastSeen", 0)
first = seen()
until("its heartbeats count", lambda: seen() > first, 30)
ok(seen() > first, "and its heartbeats count")
ok(
    "svc/db"
    not in [
        l
        for l, v in state(live[0]).get("leases", {}).items()
        if v.get("holder") == sname
    ],
    "it holds no lease",
)
# its address, with another key: refused
other = ec.generate_private_key(ec.SECP256R1())
at = int(time.time())
msg = f"commonty relay v1\0{sid}\0{at}".encode()
from cryptography.hazmat.primitives import hashes
from cryptography.hazmat.primitives.asymmetric.utils import decode_dss_signature

r_, s_ = decode_dss_signature(other.sign(msg, ec.ECDSA(hashes.SHA256())))
sig = (
    base64.urlsafe_b64encode(r_.to_bytes(32, "big") + s_.to_bytes(32, "big"))
    .decode()
    .rstrip("=")
)
okey = base64.b64encode(
    other.public_key().public_bytes(S.Encoding.DER, S.PublicFormat.SubjectPublicKeyInfo)
).decode()
code = 0
try:
    urllib.request.urlopen(
        urllib.request.Request(
            f"http://127.0.0.1:8789/relay?id={sid}&at={at}&sig={sig}&key={urllib.parse.quote(okey)}",
            headers={
                "Upgrade": "websocket",
                "Connection": "Upgrade",
                "Sec-WebSocket-Version": "13",
                "Sec-WebSocket-Key": "dGhlIHNhbXBsZSBub25jZQ==",
            },
        ),
        timeout=10,
    )
except urllib.error.HTTPError as e:
    code = e.code
ok(
    code == 403,
    f"a hello for the stranger's address with another key is refused ({code})",
)
stranger.terminate()

for p in procs.values():
    p.terminate()
if failed:
    sys.exit(f"{len(failed)} failed")
print("all passed")
