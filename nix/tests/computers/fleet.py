# The control plane a member's computer goes through, for the computers
# test's last part (nix/tests/computers.py): the relay and its witness
# under wrangler, and the box itself the one box and the other validator
# (its own dd-control unit, with the key written here in place of its
# TPM's). tom is a member on the release's list, with a root and a device
# in a keyring file as `dd` keeps them, so `dd computer` signs as he would.
# Writes /run/dd-test (the box's config, its key, the placement inputs),
# /root/tom.json and /root/f/entry.json, then /root/f/ready, and stays up.
# Run from /root/f, which holds the relay's files (w/).
import base64, hashlib, json, os, subprocess, sys, time, urllib.request, urllib.error
from cryptography.hazmat.primitives.asymmetric import ec, ed25519 as ed
from cryptography.hazmat.primitives import serialization as S

control = sys.argv[1]
b64 = lambda b: base64.b64encode(b).decode()
raw = lambda k: k.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption())
pub = lambda k: b64(k.public_key().public_bytes(S.Encoding.Raw, S.PublicFormat.Raw))


def private(path, data):
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    os.write(fd, data)
    os.close(fd)


os.makedirs("/run/dd-test", exist_ok=True)
k = ec.generate_private_key(ec.SECP256R1())
private(
    "/run/dd-test/box.p8",
    k.private_bytes(S.Encoding.DER, S.PrivateFormat.PKCS8, S.NoEncryption()),
)
spki = b64(
    k.public_key().public_bytes(S.Encoding.DER, S.PublicFormat.SubjectPublicKeyInfo)
)
# the validator's key, in the state directory its unit will keep it in
os.makedirs("/var/lib/dd-control", mode=0o700, exist_ok=True)
vote = (
    subprocess.check_output(
        [control, "-print-validator-key", "-state", "/var/lib/dd-control"]
    )
    .decode()
    .strip()
)
w = ed.Ed25519PrivateKey.generate()
rel = ed.Ed25519PrivateKey.generate()
root = ed.Ed25519PrivateKey.generate()
dev = ed.Ed25519PrivateKey.generate()
cfg = {
    "boxes": {"box": {"id": 101, "house": "h1", "key": spki}},
    "validators": [
        {"id": 1, "name": "box", "key": vote},
        {"id": 2, "name": "cloudflare", "key": pub(w)},
    ],
    # computers only on the box, tom's by the release
    "rules": {
        "leaseTtlMs": 15000,
        "goneAfterMs": 60000,
        "leases": {},
        "computerHosts": {"box": "tom"},
    },
    # tom on the release's list of members
    "identity": {
        "release": pub(rel),
        "members": {
            "members": [hashlib.sha256(pub(root).encode()).hexdigest()],
            "revoked": [],
        },
    },
    "computers": {
        "memory": 16384,
        "threads": 8,
        "disk": 200,
        "portBase": 27200,
        "portCount": 200,
        "owners": ["tom"],
    },
}
# what the box can run, as the release's placement inputs say it
placement = {
    "boxes": {
        "box": {
            "site": "h1",
            "region": "r",
            "can": {"computers": 1},
            "owner": "tom",
            "memoryGiB": 4,
            "threads": 2,
            "poolGiB": 16,
        }
    },
    "services": {},
    "damping": {"base": 0.3, "perGiB": 0.01},
}
json.dump(placement, open("/run/dd-test/placement.json", "w"))
open("w/placement.js", "w").write(f"export default {json.dumps(placement)};\n")
open("w/config.js", "w").write(f"export default {json.dumps(cfg)};\n")
open("w/.dev.vars", "w").write(f"WITNESS_KEY={b64(raw(w))}\n")

# tom's keyring, as `dd` writes one (client/auth keystore.rs FileStore),
# and his entry with this device on it, for the test to put to the gate
private(
    "/root/tom.json",
    json.dumps(
        {"identity-root": b64(raw(root)), "device-key": b64(raw(dev)), "user": "tom"}
    ).encode(),
)
entry = {
    "name": "tom",
    "root": pub(root),
    "recovery": pub(ed.Ed25519PrivateKey.generate()),
    "devices": [
        {
            "fingerprint": base64.b64decode(pub(dev))[:6].hex(),
            "public_key": pub(dev),
            "added": 1,
        }
    ],
    "terms": "2026-10-11",
    "version": 1,
    "updated": int(time.time()),
}
sig = root.sign(json.dumps(entry, separators=(",", ":")).encode())
json.dump({"entry": entry, "signature": b64(sig)}, open("entry.json", "w"))

relay = subprocess.Popen(
    ["wrangler", "dev", "--port", "8789", "--ip", "127.0.0.1"],
    cwd="w",
    stdout=open("relay.log", "a"),
    stderr=subprocess.STDOUT,
)
for _ in range(600):
    try:
        urllib.request.urlopen("http://127.0.0.1:8789/health", timeout=3)
        break
    except urllib.error.HTTPError:
        break
    except Exception:
        time.sleep(0.5)  # sleep-ok: a retry's pause; the loop is the wait
else:
    print(open("relay.log").read()[-3000:], flush=True)
    raise SystemExit("the relay did not come up")
# last: the box's dd-control waits for this file
json.dump(cfg, open("/run/dd-test/config.json", "w"))
open("ready", "w").write("ok")
relay.wait()
