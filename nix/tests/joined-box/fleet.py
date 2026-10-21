# The fleet a joined box joins, in nix/tests/joined-box.nix: the relay and
# witness under wrangler, three validators with test keys, and a gate on
# each keeping the directory as the agreed log (as nix/tests/control). It
# makes a member, ada, with a device, and writes her join ticket and a
# token of hers for the test, then stays up. Run from its working
# directory, which holds dd-control, verify and the relay's files (w/).
import base64, hashlib, json, os, subprocess, sys, time, urllib.request, urllib.error
from cryptography.hazmat.primitives.asymmetric import ec, ed25519 as ed
from cryptography.hazmat.primitives import serialization as S

T = os.getcwd()
b64 = lambda b: base64.b64encode(b).decode()

if sys.argv[1] == "item":
    # `fleet.py item <enlist|unenlist> <box> [ticket]`: ada's root signs it,
    # as a v2 envelope, and her device's token puts it to her gate
    # (PUT /_dd/boxes/<kind>), which checks it and hands it to the log.
    # The body is written as the judge reads it (state/ident enlist).
    # `fleet.py item computer <id>`: a remote computer of hers, signed by
    # her root as the computers judge reads it (state/computers Computer,
    # its fields in that order), put to PUT /_dd/computers/computer
    kind, box = sys.argv[2], sys.argv[3]
    root = ed.Ed25519PrivateKey.from_private_bytes(
        bytes.fromhex(open("root.hex").read())
    )
    rpub = b64(root.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw))
    if kind == "computer":
        fields = {
            "owner": "ada",
            "id": box,
            "name": "dev",
            "config": "default",
            "memory": 1024,
            "threads": 1,
            "disk": 32,
            "nix": "",
            "ports": [],
            "players": [],
            "keys": [],
            "consent": "any",
            "anywhere": True,
            "version": 1,
        }
    else:
        fields = {"member": hashlib.sha256(rpub.encode()).hexdigest(), "box": box}
        if kind == "enlist":
            fields["ticket"] = sys.argv[4]
        fields["at"] = int(time.time())
    body = json.dumps(fields, separators=(",", ":")).encode()
    msg = f"commonty {kind} v2".encode() + b"\0" + body
    env = {"v": 2, "kind": kind, "body": b64(body), "sig": b64(root.sign(msg))}
    subprocess.run(
        [
            "curl",
            "-sSf",
            "-X",
            "PUT",
            "-H",
            "Authorization: Bearer " + open("token").read().strip(),
            "-H",
            "content-type: application/json",
            "--data-binary",
            json.dumps(env),
            f"http://127.0.0.1:4281/_dd/computers/{kind}"
            if kind == "computer"
            else f"http://127.0.0.1:4281/_dd/boxes/{kind}",
        ],
        check=True,
        stdout=sys.stderr,
    )
    # the item's hash, which the judge's outcome for it is kept under
    # (state/ident ItemHash: the signed message, then each signature)
    print(hashlib.sha256(msg + b"\0" + env["sig"].encode()).hexdigest())
    sys.exit(0)

OUT = sys.argv[1]
PAGES = sys.argv[2]

cfg = {
    "boxes": {},
    "validators": [],
    "rules": {"leaseTtlMs": 15000, "goneAfterMs": 60000, "leases": {}},
}
for i, n in enumerate(["node1", "node2", "node3"], start=1):
    k = ec.generate_private_key(ec.SECP256R1())
    open(f"{n}.p8", "wb").write(
        k.private_bytes(S.Encoding.DER, S.PrivateFormat.PKCS8, S.NoEncryption())
    )
    cfg["boxes"][n] = {
        "id": 100 + i,
        "house": f"h{i}",
        "key": b64(
            k.public_key().public_bytes(
                S.Encoding.DER, S.PublicFormat.SubjectPublicKeyInfo
            )
        ),
    }
    os.makedirs(f"st{i}", exist_ok=True)
    pub = (
        subprocess.check_output(
            ["./dd-control", "-print-validator-key", "-state", f"st{i}"]
        )
        .decode()
        .strip()
    )
    cfg["validators"].append({"id": i, "name": n, "key": pub})
w = ed.Ed25519PrivateKey.generate()
cfg["validators"].append(
    {
        "id": 4,
        "name": "cloudflare",
        "key": b64(w.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)),
    }
)
# the identity judge's part, so dd-control serves /identity: a release
# key, and nobody on its list - ada comes in by her box alone (she is on
# the replaying gates' VERIFY_MEMBERS, which her agreed gate never reads)
rel = ed.Ed25519PrivateKey.generate()
cfg["identity"] = {
    "release": b64(rel.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)),
    "members": {"members": [], "revoked": []},
}
# members' remote computers, so a member by a box makes one (state/computers)
cfg["computers"] = {
    "memory": 16384,
    "threads": 8,
    "disk": 200,
    "portBase": 27200,
    "portCount": 8,
}
json.dump(cfg, open("config.json", "w"))
json.dump(cfg, open(f"{OUT}/config.json", "w"))
open("w/config.js", "w").write("export default " + json.dumps(cfg) + ";\n")
open("w/.dev.vars", "w").write(
    "WITNESS_KEY="
    + b64(w.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption()))
    + "\n"
)
json.dump(
    {
        "boxes": {},
        "services": {},
        "roles": {},
        "damping": {"base": 0.3, "perGiB": 0.01},
    },
    open("placement.json", "w"),
)
log = lambda n: open(f"{T}/{n}.log", "a")
subprocess.Popen(
    ["wrangler", "dev", "--port", "8789", "--ip", "0.0.0.0"],
    cwd="w",
    stdout=log("relay"),
    stderr=subprocess.STDOUT,
)


def up(url):
    try:
        urllib.request.urlopen(url, timeout=3)
        return True
    except urllib.error.HTTPError:
        return True
    except Exception:
        return False


def until(what, cond, timeout=180):
    end = time.time() + timeout
    while time.time() < end:
        if cond():
            print(f"fleet: {what}", flush=True)
            return
        time.sleep(0.5)  # sleep-ok: this is the wait, polling cond until it holds
    for f in sorted(os.listdir(T)):
        if f.endswith(".log"):
            print(f"--- {f}\n" + open(f).read()[-1500:], flush=True)
    raise SystemExit(f"timed out: {what}")


until("the relay up", lambda: up("http://127.0.0.1:8789/health"))


# a stand-in for headscale's API, for the member gate's network door: a
# user per name, and a key for each ask (network.rs)
def headscale():
    import http.server, threading

    class H(http.server.BaseHTTPRequestHandler):
        def answer(self, v):
            b = json.dumps(v).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)

        def do_GET(self):
            if self.path.startswith("/api/v1/user"):
                name = self.path.partition("name=")[2]
                self.answer({"users": [{"id": "7", "name": name}]})
            else:
                self.answer({"nodes": []})

        def do_POST(self):
            n = int(self.headers.get("content-length") or 0)
            self.rfile.read(n)
            self.answer({"preAuthKey": {"id": "9", "key": "test-preauth-key"}})

        def log_message(self, *a):
            pass

    s = http.server.ThreadingHTTPServer(("127.0.0.1", 8790), H)
    threading.Thread(target=s.serve_forever, daemon=True).start()


headscale()
open("headscale.key", "w").write("test")
for i in (1, 2, 3):
    os.makedirs(f"placed{i}", exist_ok=True)
    json.dump({}, open(f"carries{i}.json", "w"))
    subprocess.Popen(
        [
            "./dd-control",
            "-config",
            "config.json",
            "-box",
            f"node{i}",
            "-relay",
            "ws://127.0.0.1:8789/relay",
            "-state",
            f"st{i}",
            "-box-key",
            f"node{i}.p8",
            "-listen",
            f"127.0.0.1:419{i}",
            "-heartbeat",
            "1s",
            "-roles",
            f"r{i}",
            "-submit",
            f"s{i}.sock",
            "-submit-group",
            "root",
            "-builds",
            f"b{i}.sock",
            "-placement",
            "placement.json",
            "-placed-carries",
            f"carries{i}.json",
            "-placed-dir",
            f"placed{i}",
        ],
        stdout=log(f"n{i}"),
        stderr=subprocess.STDOUT,
    )
# ada, a member (on the fleet's list), with one device
root = ed.Ed25519PrivateKey.generate()
dev = ed.Ed25519PrivateKey.generate()
open("root.hex", "w").write(
    root.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption()).hex()
)
pub = lambda k: b64(k.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw))
fp = base64.b64decode(pub(dev))[:6].hex()
members = json.dumps([hashlib.sha256(pub(root).encode()).hexdigest()])
for i in (1, 2, 3):
    env = dict(
        os.environ,
        # the first gate whole, for the member's pages (/_dd/boxes/mine)
        VERIFY_ROLE="full" if i == 1 else "directory",
        # who may use the services, and whose a joined box is: the agreed
        # identity state alone, on the member's gate
        VERIFY_IDENTITY_FROM="agreed" if i == 1 else "replay",
        VERIFY_PAGES_DIR=PAGES,
        VERIFY_DIR=f"{T}/g{i}/keys",
        VERIFY_BIND=f"0.0.0.0:428{i}",
        VERIFY_CONTROL=f"http://127.0.0.1:419{i}",
        VERIFY_CONTROL_SUBMIT=f"{T}/s{i}.sock",
        VERIFY_CONTROL_CONFIG=f"{T}/config.json",
        VERIFY_BOX=f"node{i}",
        VERIFY_DOMAIN="commonty.test",
        VERIFY_MEMBERS=members,
    )
    if i == 1:
        env.update(
            VERIFY_HEADSCALE_API="http://127.0.0.1:8790",
            VERIFY_HEADSCALE_KEY_FILE=f"{T}/headscale.key",
            VERIFY_HEADSCALE_URL="https://net.commonty.test",
        )
    os.makedirs(f"g{i}/keys", exist_ok=True)
    subprocess.Popen(
        ["./verify"], env=env, stdout=log(f"g{i}"), stderr=subprocess.STDOUT
    )
until(
    "the gates up",
    lambda: all(up(f"http://127.0.0.1:428{i}/_dd/directory") for i in (1, 2, 3)),
)

# ada's entry put to the directory
entry = {
    "name": "ada",
    "root": pub(root),
    "recovery": pub(ed.Ed25519PrivateKey.generate()),
    "devices": [{"fingerprint": fp, "public_key": pub(dev), "added": 1}],
    "terms": "2026-10-11",
    "version": 1,
    "updated": int(time.time()),
}
sig = root.sign(json.dumps(entry, separators=(",", ":")).encode())
body = json.dumps({"entry": entry, "signature": b64(sig)}).encode()
for _ in range(120):
    try:
        req = urllib.request.Request(
            "http://127.0.0.1:4281/_dd/directory/ada",
            method="PUT",
            data=body,
            headers={"content-type": "application/json"},
        )
        if urllib.request.urlopen(req, timeout=70).status == 200:
            break
    except Exception:
        time.sleep(1)  # sleep-ok: a retry's pause; the loop is the wait
else:
    raise SystemExit("ada's entry was not agreed")

# her join ticket and a token of hers, as her device mints them
# (client/auth device.rs: mint_join_ticket, mint)
hexkey = dev.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption()).hex()
exp = int(time.time()) + 7 * 24 * 3600


def mint(datalog):
    return (
        subprocess.check_output(
            ["biscuit", "generate", "--private-key", "ed25519-private/" + hexkey, "-"],
            input=datalog.encode(),
        )
        .decode()
        .strip()
    )


ticket = mint(
    f'user("ada");\ndevice("{fp}");\nticket("00112233445566778899aabbccddeeff");\n'
    f'purpose("join");\ncheck if time($t), $t < {exp};\ncheck if operation("join");\n'
)
token = mint(f'user("ada");\ndevice("{fp}");\ncheck if time($t), $t < {exp};\n')
open(f"{OUT}/ticket", "w").write(ticket + "\n")
open(f"{T}/token", "w").write(token)
open(f"{T}/ready", "w").write("ok")
while True:
    time.sleep(3600)  # sleep-ok: no wait here; the fleet stays up for the test
