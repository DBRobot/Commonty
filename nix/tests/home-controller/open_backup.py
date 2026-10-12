# Open the controller's backup as the made-up member would, with their
# device key (the test key of 32 sevens): the sealed backup key, then the
# file. Prints what the backup holds, as JSON.
#   open_backup.py <backup file>
import base64
import json
import struct
import sys
import tarfile
import io

import nacl.bindings
import nacl.public
import nacl.secret
from cryptography.hazmat.primitives import serialization
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


def unb64(s):
    s = s.replace("-", "+").replace("_", "/")
    return base64.b64decode(s + "=" * (-len(s) % 4))


with open(sys.argv[1], "rb") as f:
    data = f.read()
assert data[:4] == b"CBK1", data[:4]
(n,) = struct.unpack(">I", data[4:8])
header = json.loads(data[8 : 8 + n])
body = data[8 + n :]

seed = bytes([7] * 32)
k = Ed25519PrivateKey.from_private_bytes(seed)
pub = k.public_key().public_bytes(
    serialization.Encoding.Raw, serialization.PublicFormat.Raw
)
mine = base64.b64encode(pub).decode()
sealed = [s for s in header["sealed"] if s["to"] == mine]
assert sealed, ("nothing sealed to this device", [s["to"] for s in header["sealed"]])

sk = nacl.bindings.crypto_sign_ed25519_sk_to_curve25519(seed + pub)
key = nacl.public.SealedBox(nacl.public.PrivateKey(sk)).decrypt(unb64(sealed[0]["key"]))
tar = nacl.secret.SecretBox(key).decrypt(body, unb64(header["nonce"]))
names = tarfile.open(fileobj=io.BytesIO(tar)).getnames()
print(json.dumps({"names": names, "made": header.get("made")}))
