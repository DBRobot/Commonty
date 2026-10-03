# The disks' key at boot, end to end (nix/modules/box/disk-unlock.nix)
import json

worker.start()
worker.wait_for_unit("fake-worker.service")
worker.wait_for_open_port(8000)
box.start()
box.wait_for_unit("multi-user.target")

def state():
    return json.loads(worker.succeed("curl -s http://127.0.0.1:8000/state"))
def post(path, body):
    worker.succeed(f"curl -sf -X POST http://127.0.0.1:8000{path} -H 'content-type: application/json' -d '{json.dumps(body)}'")

# not enrolled yet: the initrd found nothing on the boot partition and went on
box.succeed("journalctl -b -o cat | grep -q 'no enrolment on the boot partition'")

# enrol: the TPM's key and half on the boot partition, the key to the service
out = box.succeed("dd-unlock-enrol keys")
key = out.strip().splitlines()[-1]
post("/test/box", {"box": "box", "key": key})
box.succeed("test -s /boot/dd/unlock/sig.priv && test -s /boot/dd/unlock/half.priv")
box.succeed("dd-unlock-enrol share")
box.succeed("test $(stat -c %s /run/dd/disk.key) = 32 && test -s /boot/dd/box-disk.age")

# a pool encrypted to the key
box.succeed("zpool create -O mountpoint=none data /dev/vdb")
box.succeed("zfs create -o encryption=aes-256-gcm -o keyformat=raw -o keylocation=file:///run/dd/disk.key -o mountpoint=/data data/enc")
box.succeed("echo kept > /data/proof")

# 1. at home: reboots and unlocks by itself
box.shutdown()
box.start()
box.wait_for_unit("multi-user.target")
box.succeed("grep -q kept /data/proof")
box.succeed("zfs get -H -o value keystatus data/enc | grep -q available")
box.wait_until_succeeds("test ! -e /run/dd/disk.key")
box.succeed("journalctl -b -o cat | grep -q 'dd-unlock: unlocked'")

# 2. somewhere new: it waits, and a member lets it in
post("/test/away", {"box": "box"})
box.shutdown()
box.start()
worker.wait_until_succeeds("curl -s http://127.0.0.1:8000/state | grep -q '\"pending\": {\"box\"'", timeout=300)
# (the box is in its initrd, waiting: nothing on it answers yet)
post("/test/approve", {"box": "box"})
box.wait_for_unit("multi-user.target", timeout=300)
box.succeed("grep -q kept /data/proof")

# 3. marked stolen: it stays locked, even at home
post("/test/stolen", {"box": "box", "stolen": True})
box.shutdown()
box.start()
worker.wait_until_succeeds("curl -s http://127.0.0.1:8000/state | grep -q '\"asked\": [1-9]'", timeout=300)
asked = state()["asked"]
worker.wait_until_succeeds(f"test $(curl -s http://127.0.0.1:8000/state | jq .asked) -gt {asked}", timeout=400)
post("/test/stolen", {"box": "box", "stolen": False})
box.wait_for_unit("multi-user.target", timeout=600)
box.succeed("grep -q kept /data/proof")

# 4. under another boot state (PCR 7 moved) the TPM neither signs nor unseals
sign = (
    "cd $(mktemp -d) && export TPM2TOOLS_TCTI=device:/dev/tpmrm0 && tpm2_createprimary -Q -C o -G ecc256 -c srk.ctx"
    " && tpm2_load -Q -C srk.ctx -u /boot/dd/unlock/{0}.pub -r /boot/dd/unlock/{0}.priv -c k.ctx"
    " && tpm2_startauthsession -Q --policy-session -S s.ctx && tpm2_policypcr -Q -S s.ctx -l sha256:7"
)
box.succeed((sign + " && echo hi > m && tpm2_sign -Q -c k.ctx -g sha256 -s ecdsa -f plain -p session:s.ctx -o sig m").format("sig"))
box.succeed("tpm2_pcrextend 7:sha256=0000000000000000000000000000000000000000000000000000000000000000")
box.fail((sign + " && echo hi > m && tpm2_sign -Q -c k.ctx -g sha256 -s ecdsa -f plain -p session:s.ctx -o sig m").format("sig"))
box.fail((sign + " && tpm2_unseal -c k.ctx -p session:s.ctx -o half").format("half"))

# 5. the paper key opens the copy on the boot partition
box.succeed(f"printf '%s\\n' '{nix['paper']}' | script -qc 'dd disk recover /boot/dd/box-disk.age /root/recovered.key' /dev/null")
box.succeed("zfs unmount data/enc && zfs unload-key data/enc && zfs load-key -L file:///root/recovered.key data/enc && zfs mount data/enc && grep -q kept /data/proof")
