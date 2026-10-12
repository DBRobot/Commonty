# The joining installer: refuses two disks unnamed, wipes only the one it
# is told, makes the box's key and name in its TPM, installs.
import json

# target is this same machine later, booting what was installed
forge.start()
blank.start()
forge.wait_for_unit("nginx.service")
blank.wait_for_unit("multi-user.target")

# a stand-in for the fleet's repository: a flake whose joined box is a
# system already in this machine's store
system = blank.succeed("cat /etc/installed").strip()
flake = (
    "{ outputs = { self }: { lib.joinedBox = { name, dir }: "
    f'{{ config.system.build.toplevel = builtins.storePath "{system}"; }}; }}; }}'
)
forge.succeed(
    "mkdir -p /srv/release /root/flake && cd /root/flake && git init -q && "
    f"cat > flake.nix <<'EOF'\n{flake}\nEOF\n"
    " git add flake.nix && git -c user.email=t@t -c user.name=t commit -qm flake && "
    "git clone -q --bare /root/flake /srv/flake.git && cd /srv/flake.git && git update-server-info"
)
rev = forge.succeed("git -C /root/flake rev-parse HEAD").strip()

# the release, signed by a release key the installer holds the public half of
env = "DD_KEYRING_FILE=/root/keys.json"
forge.succeed("git init -q /root/repo && mkdir -p /root/repo/fleet")
pub = forge.succeed(
    f"{env} {nix['dd']} release init --repo /root/repo | tail -1"
).strip()
blank.succeed(f"echo '{pub}' > /root/release.pub")
payload = json.dumps({"counter": 1, "rev": rev, "issued": 1000, "boxes": {}})
forge.succeed(f"cat > /root/payload.json <<'EOF'\n{payload}\nEOF")
forge.succeed(
    f"{env} {nix['dd']} release sign /root/payload.json --out /srv/release/current.json"
)

# the member's join ticket, made as the app makes it; the fake gate takes it
# for its own folder, where the app already left its sealed key
tid = nix["ticketId"]
forge.succeed(
    "biscuit keypair --only-private-key > /root/tk && "
    f'printf \'user("ada");\\nticket("{tid}");\\npurpose("join");\\n\' '
    "| biscuit generate --private-key-file /root/tk - | tr -d '\\n' > /srv/ticket && "
    'printf \'set $want "Bearer %s";\\n\' "$(cat /srv/ticket)" > /run/gate/want.conf && '
    f"mkdir -p /srv/gate/{tid} && echo sealed > /srv/gate/{tid}/dd-devices.json && "
    "systemctl reload nginx"
)

# the drive's commonty/, as the app wrote it
blank.succeed(
    "mkdir -p /root/drive && curl -sf http://forge/ticket > /root/drive/ticket"
    " && head -c 32 /dev/urandom | base64 > /root/drive/backup.key"
    ' && echo \'{"ssid": "house", "psk": "not-a-real-psk"}\' > /root/drive/wifi.json'
    " && echo test > /root/drive/version"
)
blank.systemctl("restart dd-box-setup")
blank.wait_for_open_port(8090)


token = blank.succeed("cat /run/dd-setup-token/token").strip()
mine = f"-H 'x-setup-token: {token}'"


def api(path, body=None):
    if body is None:
        out = blank.succeed(f"curl -sf {mine} http://127.0.0.1:8090/api/{path}")
    else:
        out = blank.succeed(
            f"curl -sf {mine} -H 'content-type: application/json' "
            f"-d '{json.dumps(body)}' http://127.0.0.1:8090/api/{path}"
        )
    return json.loads(out)


# only its own page drives the screen: without the secret, addressed by
# another name, or asked from another page, nothing
def status(args):
    return blank.succeed(
        f"curl -s -o /dev/null -w '%{{http_code}}' {args} http://127.0.0.1:8090/api/checks"
    ).strip()


assert status("") == "403"
assert status("-H 'x-setup-token: 00'") == "403"
assert status(f"{mine} -H 'Host: rebound.example:8090'") == "403"
assert status(f"{mine} -H 'Origin: http://portal.example'") == "403"
assert status(f"{mine} -H 'Origin: http://127.0.0.1:8090'") == "200"
assert (
    blank.succeed(
        "curl -s -o /dev/null -w '%{http_code}' -X POST -H 'content-type: application/json' "
        '-d \'{"disk": "/dev/vdc"}\' http://127.0.0.1:8090/api/install'
    ).strip()
    == "403"
)


def finish(kind):
    for _ in range(900):
        j = api("job")
        if j["kind"] == kind and j["state"] != "running":
            assert j["state"] == "done", j
            return j
        blank.sleep(1)
    raise AssertionError(f"{kind} did not finish")


# a marker on the disk that must survive
blank.succeed("echo keep | dd of=/dev/vdc bs=512 seek=2048 conv=notrunc 2>/dev/null")

# what the screen says of this machine: UEFI, a TPM, Secure Boot off, its
# disks seen; no firmware driver here, so nothing to change from it
c = api("checks")
assert c["checks"]["uefi"] and c["checks"]["tpm"], c
assert not c["checks"]["secure_boot"] and not c["checks"]["raid"], c
assert c["firmware"]["driver"] is None and c["firmware"]["change"] == [], c
assert c["drive"]["ticket"] and c["drive"]["backupKey"], c

# the disk this machine runs from is never one: named, refused
out = blank.fail("DISK=/dev/vda dd-join-install 2>&1")
assert "not an internal disk here" in out, out
# two blank internal disks and none named: nothing is touched
out = blank.fail("dd-join-install 2>&1")
assert "2 internal disks" in out, out

# what is on the disk to be wiped: a filesystem with something in it
blank.succeed(
    "parted -s /dev/vdb mklabel gpt mkpart old ext4 1MiB 512MiB && udevadm settle"
    " && mkfs.ext4 -q /dev/vdb1 && mkdir -p /tmp/old && mount /dev/vdb1 /tmp/old"
    " && echo 'a letter' > /tmp/old/letter.txt && umount /tmp/old"
)
disks = [d["name"] for d in api("disks")["disks"]]
assert disks == ["/dev/vdb", "/dev/vdc"], disks

# backed up through the screen: to the member's pending backup, on the
# ticket alone, encrypted here with the drive's key; the app's file kept
api("backup", {"disk": "/dev/vdb"})
finish("backup")
assert forge.succeed(f"cat /srv/gate/{tid}/dd-devices.json").strip() == "sealed"
key = blank.succeed("cat /root/drive/backup.key").strip()
forge.succeed(f"echo '{key}' > /root/bk")
listing = forge.succeed(f"restic -r /srv/gate/{tid} --password-file /root/bk ls latest")
assert "letter.txt" in listing, listing
forge.fail(f"grep -rq 'a letter' /srv/gate/{tid}")

# installed through the screen: that disk wiped and installed to, the
# other not, with the drive's Wi-Fi and the member's ticket
api("install", {"disk": "/dev/vdb"})
out = "\n".join(finish("install")["stages"])
print(out)
assert "installed" in out, out
name = blank.succeed(
    "cat /mnt/var/lib/dd/box-key.spki | base64 -d | sha256sum | cut -c1-10"
).strip()
assert f"joins the fleet as j{name}" in out, out
assert (
    blank.succeed("dd if=/dev/vdc bs=512 skip=2048 count=1 2>/dev/null | head -c4")
    == "keep"
)
# its keys are in its TPM, its report and recipient where the box reads them
blank.succeed(
    "test -s /mnt/boot/dd/unlock/sig.priv -a -s /mnt/var/lib/dd/secrets-key.txt"
)
blank.succeed("jq -e '.bootDisk' /mnt/var/lib/dd/box/hardware.json")
blank.succeed("grep -q '^age1' /mnt/var/lib/dd/secrets-recipient")
assert blank.succeed("jq -r .owner /mnt/var/lib/dd/box/box.json").strip() == "null"
# its pool encrypted, under the key its TPM's half makes; nothing in the
# clear but the pool itself
assert blank.succeed("zfs get -H -o value encryption tank/enc").strip() == "aes-256-gcm"
blank.succeed("test -s /mnt/boot/dd/unlock/half.priv")
blank.fail("test -e /run/dd/disk.key")
blank.succeed("jq -e '.encrypted and .unlock == \"tpm\"' /mnt/var/lib/dd/box/box.json")
# the Wi-Fi sealed to the box, never in the clear on its disk; the ticket kept
assert (
    blank.succeed("jq -r .house.ssid /mnt/var/lib/dd/box/box.json").strip() == "house"
)
blank.succeed("test -s /mnt/var/lib/dd/wifi.age")
blank.fail("grep -rq not-a-real-psk /mnt/var/lib/dd")
blank.succeed("cmp /mnt/var/lib/dd/join-ticket /root/drive/ticket")
# the system the release's commit built is the one on the disk
blank.succeed(f'[ "$(readlink -f /mnt/nix/var/nix/profiles/system)" = {system} ]')
# and its agent starts at that release: none signed before it takes it back
assert blank.succeed("cat /mnt/var/lib/dd-agent/counter").strip() == "1"

# an old release, signed as it was: refused
forge.succeed('sed -i \'s/"issued": 1000/"issued": 50/\' /root/payload.json')
forge.succeed(
    f"{env} {nix['dd']} release sign /root/payload.json --out /srv/release/current.json"
)
out = blank.fail("DISK=/dev/vdc dd-join-install 2>&1")
assert "no release the release key signed" in out, out
assert "an old release, refused" in out, out
# a release not signed by the release key: nothing installed
forge.succeed('sed -i \'s/"counter": 1/"counter": 2/\' /srv/release/current.json')
out = blank.fail("DISK=/dev/vdc dd-join-install 2>&1")
assert "no release the release key signed" in out, out
assert (
    blank.succeed("dd if=/dev/vdc bs=512 skip=2048 count=1 2>/dev/null | head -c4")
    == "keep"
)

# the installed disk boots on its own: the initrd opens the pool with the
# TPM's half alone, no network, nobody at the screen
blank.succeed("umount -R /mnt && zpool export tank && sync")
blank.shutdown()
target.state_dir = blank.state_dir
target.start()
target.wait_for_unit("multi-user.target")
assert target.succeed("zfs get -H -o value keystatus tank/enc").strip() == "available"
target.succeed(
    "journalctl -b -u dd-disk-key --no-pager | grep -q 'dd-unlock: unlocked'"
)

# the same unlock, run again on the box, makes the key; under a boot the
# TPM does not know (PCR 7 changed), it makes none: never a key from less
target.succeed("dd-unlock-now && test -s /run/dd/disk.key && shred -u /run/dd/*.key")
target.succeed(
    "TPM2TOOLS_TCTI=device:/dev/tpmrm0 tpm2_pcrextend "
    "7:sha256=0000000000000000000000000000000000000000000000000000000000000001"
)
out = target.fail("dd-unlock-now 2>&1")
assert "the pools stay locked" in out, out
target.fail("test -e /run/dd/disk.key")
