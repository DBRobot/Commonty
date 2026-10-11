# What a device does before asking a box to transcode, with the file
# already in rclone's crypt format (rclone wrote it): the sealed file goes
# where the box can fetch ranges of it, and the file's block keys are
# sealed to the box's ephemeral x25519 key, a libsodium sealed box. A
# block's key is HSalsa20 of the library's data key (the first 32 bytes
# rclone derives with scrypt from password and salt) over the first 16
# bytes of that block's nonce: the file's nonce plus the block number,
# little-endian (client/library/src/crypt.rs, block_key). The data key
# itself never goes to the box. Prints the session body.
import base64, hashlib, json, os, struct, sys
from nacl.public import PublicKey, SealedBox

M = 0xFFFFFFFF


def hsalsa20(key, n16):
    x = [0] * 16
    x[0], x[5], x[10], x[15] = struct.unpack("<4I", b"expand 32-byte k")
    x[1:5] = struct.unpack("<4I", key[:16])
    x[11:15] = struct.unpack("<4I", key[16:])
    x[6:10] = struct.unpack("<4I", n16)
    rotl = lambda v, c: ((v << c) & M) | (v >> (32 - c))

    def qr(a, b, c, d):
        x[b] ^= rotl((x[a] + x[d]) & M, 7)
        x[c] ^= rotl((x[b] + x[a]) & M, 9)
        x[d] ^= rotl((x[c] + x[b]) & M, 13)
        x[a] ^= rotl((x[d] + x[c]) & M, 18)

    for _ in range(10):
        qr(0, 4, 8, 12), qr(5, 9, 13, 1), qr(10, 14, 2, 6), qr(15, 3, 7, 11)
        qr(0, 1, 2, 3), qr(5, 6, 7, 4), qr(10, 11, 8, 9), qr(15, 12, 13, 14)
    return struct.pack("<8I", *(x[i] for i in (0, 5, 10, 15, 6, 7, 8, 9)))


# NaCl's own vector (tests/core4), so a wrong HSalsa20 fails here, not in ffmpeg
assert hsalsa20(
    bytes.fromhex("1b27556473e985d462cd51197a9a46c76009549eac6474f206c4ee0844f68389"),
    bytes.fromhex("69696ee955b62b73cd62bda875fc73d6"),
) == bytes.fromhex("dc908dda0b9344a953629b733820778880f3ceb421bb61b91cbd4c3e66256ce4")

sealed_path, password, salt, box_key_b64, url = sys.argv[1:6]
key = hashlib.scrypt(
    password.encode(), salt=salt.encode(), n=16384, r=8, p=1, dklen=80
)[:32]
size = os.path.getsize(sealed_path)
with open(sealed_path, "rb") as f:
    nonce = int.from_bytes(f.read(32)[8:], "little")
blocks = -(-(size - 32) // (64 * 1024 + 16))
keys = b"".join(
    hsalsa20(key, ((nonce + n) % (1 << 192)).to_bytes(24, "little")[:16])
    for n in range(blocks)
)
b64 = lambda b: base64.urlsafe_b64encode(b).rstrip(b"=").decode()
pad = lambda s: s + "=" * (-len(s) % 4)
box_pk = PublicKey(base64.urlsafe_b64decode(pad(box_key_b64)))
print(
    json.dumps({"url": url, "keys": b64(SealedBox(box_pk).encrypt(keys)), "size": size})
)
