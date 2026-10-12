# The fleet a joined box joins, in nix/tests/joined-box.nix: the relay and
# witness under wrangler, three validators with test keys, and a gate on
# each keeping the directory as the agreed log (as nix/tests/control). It
# makes a member, ada, with a device, and writes her join ticket and a
# token of hers for the test, then stays up. Run from its working
# directory, which holds dd-control, verify and the relay's files (w/).
import base64, json, os, subprocess, sys, time, urllib.request, urllib.error
from cryptography.hazmat.primitives.asymmetric import ec, ed25519 as ed
from cryptography.hazmat.primitives import serialization as S

T = os.getcwd()
OUT = sys.argv[1]
PAGES = sys.argv[2]
b64 = lambda b: base64.b64encode(b).decode()

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


def until(cond, timeout=180):
    end = time.time() + timeout
    while time.time() < end:
        if cond():
            return
        time.sleep(0.5)
    raise SystemExit("timed out")


until(lambda: up("http://127.0.0.1:8789/health"))
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
for i in (1, 2, 3):
    env = dict(
        os.environ,
        # the first gate whole, for the member's pages (/_dd/boxes/mine)
        VERIFY_ROLE="full" if i == 1 else "directory",
        VERIFY_PAGES_DIR=PAGES,
        VERIFY_DIR=f"{T}/g{i}/keys",
        VERIFY_BIND=f"0.0.0.0:428{i}",
        VERIFY_CONTROL=f"http://127.0.0.1:419{i}",
        VERIFY_CONTROL_SUBMIT=f"{T}/s{i}.sock",
        VERIFY_CONTROL_CONFIG=f"{T}/config.json",
        VERIFY_BOX=f"node{i}",
    )
    os.makedirs(f"g{i}/keys", exist_ok=True)
    subprocess.Popen(
        ["./verify"], env=env, stdout=log(f"g{i}"), stderr=subprocess.STDOUT
    )
until(lambda: all(up(f"http://127.0.0.1:428{i}/_dd/directory") for i in (1, 2, 3)))

# ada, a member, with one device: her entry put to the directory
root = ed.Ed25519PrivateKey.generate()
dev = ed.Ed25519PrivateKey.generate()
pub = lambda k: b64(k.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw))
fp = base64.b64decode(pub(dev))[:6].hex()
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
        time.sleep(1)
else:
    raise SystemExit("ada's entry was not agreed")

# her join ticket and a token of hers, as her device mints them
# (client/auth device.rs: mint_join_ticket, mint)
hexkey = dev.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption()).hex()
exp = int(time.time()) + 7 * 24 * 3600


def mint(datalog):
    return (
        subprocess.check_output(
            ["biscuit", "generate", "--private-key", hexkey, "-"],
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
    time.sleep(3600)
