# Every box's report in fleet/boxes/<box>/ is the one the box itself made:
# hardware.json and hardware-configuration.nix, signed by its TPM-held key
# (the unlockKey in fleet/boxes.json) over the message dd-control
# -sign-report builds (box/control/cmd/dd-control/main.go, reportMessage).
import base64, hashlib, json, os, sys
from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.asymmetric.utils import encode_dss_signature
from cryptography.exceptions import InvalidSignature

root = sys.argv[1]
boxes = json.load(open(os.path.join(root, "fleet/boxes.json")))
bad = []
for name in sorted(os.listdir(os.path.join(root, "fleet/boxes"))):
    d = os.path.join(root, "fleet/boxes", name)
    if name not in boxes:
        bad.append(f"{name}: has a folder but is not in fleet/boxes.json")
        continue
    try:
        sig = open(os.path.join(d, "report.sig")).read().strip()
    except FileNotFoundError:
        bad.append(f"{name}: no report.sig; its report was not signed by the box")
        continue
    msg = b"commonty box report v1\0" + name.encode()
    for f in ("hardware.json", "hardware-configuration.nix"):
        msg += b"\0" + hashlib.sha256(open(os.path.join(d, f), "rb").read()).hexdigest().encode()
    raw = base64.urlsafe_b64decode(sig + "=" * (-len(sig) % 4))
    if len(raw) == 64:
        raw = encode_dss_signature(int.from_bytes(raw[:32], "big"), int.from_bytes(raw[32:], "big"))
    key = serialization.load_der_public_key(base64.b64decode(boxes[name]["unlockKey"]))
    try:
        key.verify(raw, msg, ec.ECDSA(hashes.SHA256()))
        print(f"{name}: signed by its TPM")
    except InvalidSignature:
        bad.append(f"{name}: report.sig does not match its report and key; edit nothing here by hand, ask the box (scripts/box-report {name})")
for b in boxes:
    if not os.path.isdir(os.path.join(root, "fleet/boxes", b)):
        bad.append(f"{b}: in fleet/boxes.json but has no folder in fleet/boxes/")
if bad:
    print("\n".join(bad), file=sys.stderr)
    sys.exit(1)
