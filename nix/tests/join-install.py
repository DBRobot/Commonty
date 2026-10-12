# The joining installer: refuses two disks unnamed, wipes only the one it
# is told, makes the box's key and name in its TPM, installs.
import json

start_all()
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
payload = json.dumps({"counter": 1, "rev": rev, "issued": 1, "boxes": {}})
forge.succeed(f"cat > /root/payload.json <<'EOF'\n{payload}\nEOF")
forge.succeed(
    f"{env} {nix['dd']} release sign /root/payload.json --out /srv/release/current.json"
)

# a marker on the disk that must survive
blank.succeed("echo keep | dd of=/dev/vdc bs=512 seek=2048 conv=notrunc 2>/dev/null")

# two internal disks and none named: nothing is touched
out = blank.fail("sudo -E dd-join-install 2>&1")
assert "2 internal disks" in out, out

# named: that one is wiped and installed to, the other is not
out = blank.succeed("DISK=/dev/vdb dd-join-install 2>&1")
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
# the system the release's commit built is the one on the disk
blank.succeed(f'[ "$(readlink -f /mnt/nix/var/nix/profiles/system)" = {system} ]')

# a release not signed by the release key: nothing installed
forge.succeed('sed -i \'s/"counter": 1/"counter": 2/\' /srv/release/current.json')
out = blank.fail("DISK=/dev/vdc dd-join-install 2>&1")
assert "no release the release key signed" in out, out
assert (
    blank.succeed("dd if=/dev/vdc bs=512 skip=2048 count=1 2>/dev/null | head -c4")
    == "keep"
)
