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

# a marker on the disk that must survive
blank.succeed("echo keep | dd of=/dev/vdc bs=512 seek=2048 conv=notrunc 2>/dev/null")

# the disk this machine runs from is never one: named, refused
out = blank.fail("DISK=/dev/vda dd-join-install 2>&1")
assert "not an internal disk here" in out, out
# two blank internal disks and none named: nothing is touched
out = blank.fail("dd-join-install 2>&1")
assert "2 internal disks" in out, out

# named: that one is wiped and installed to, the other is not
# with the drive's Wi-Fi and the member's ticket, as the app wrote them
blank.succeed(
    'echo \'{"ssid": "house", "psk": "not-a-real-psk"}\' > /root/wifi.json'
    " && echo ticket-bytes > /root/ticket"
)
out = blank.succeed(
    "DISK=/dev/vdb WIFI=/root/wifi.json TICKET=/root/ticket dd-join-install 2>&1"
)
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
assert blank.succeed("cat /mnt/var/lib/dd/join-ticket").strip() == "ticket-bytes"
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
