# A box's join proving its TPM, against swtpm under OVMF's Secure Boot and
# lanzaboote (nix/tests/join-attest.nix)
import os
import subprocess
import tempfile

# the image as lanzaboote's own tests boot it: a copy-on-write layer over
# the built one; its first boot enrols the box's keys and restarts
disk = tempfile.NamedTemporaryFile()
subprocess.run(
    [
        nix["qemuImg"],
        "create",
        "-f",
        "qcow2",
        "-b",
        nix["image"],
        "-F",
        "raw",
        disk.name,
    ],
    check=True,
)
os.environ["NIX_DISK_IMAGE"] = disk.name
box.start(allow_reboot=True)
box.connected = False

K = "/boot/dd/unlock"
FW = "/sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"


def up():
    box.wait_for_unit("multi-user.target")
    box.wait_until_succeeds("test -e /dev/tpmrm0")


def secure_boot():
    return box.succeed(f"od -An -tu1 -j4 -N1 {FW} | tr -d ' \\n'") == "1"


def release(*specs):
    """The release's Boot (flake.nix attestBoot): this system's systemd-boot,
    the release's stub and kernel, and the kernel parameters of each named
    system ("" this one, else a specialisation)"""
    box.succeed("mkdir -p /run/rel")
    files = []
    for i, spec in enumerate(specs):
        sysdir = nix["system"] + (f"/specialisation/{spec}" if spec else "")
        box.succeed(
            f"attest-check boot -loader {sysdir}/systemd/lib/systemd/boot/efi/systemd-bootx64.efi"
            f" -stub /etc/dd-test/stub.efi -kernel {sysdir}/kernel"
            f' -params "$(cat {sysdir}/kernel-params)" > /run/rel/{i}.json'
        )
        files.append(f"/run/rel/{i}.json")
    out = "/run/rel/boot.json"
    box.succeed(
        f"jq -s '{{loaders: (map(.loaders[]) | unique), stubs: (map(.stubs[]) | unique),"
        f" kernels: (map(.kernels[]) | unique), params: (map(.params[]) | unique)}}' {' '.join(files)} > {out}"
    )
    return out


def join(roots, boot, software=True):
    sw = "-software" if software else ""
    code, out = box.execute(
        f"TMPDIR=/run attest-check join -roots /etc/dd-test/{roots} {sw} -boot {boot}"
        f" -keys {K} -dir /run/ak -db /var/lib/lanzaboote-test-fixture/keys/db/db.pem 2>&1"
    )
    print(out)
    return code, out


def signs():
    """the join key signs, in the session every user of it makes"""
    return (
        box.execute(
            "export TPM2TOOLS_TCTI=device:/dev/tpmrm0 && w=$(mktemp -d -p /run) && cd $w && echo hi > m"
            f" && tpm2_createprimary -Q -C o -G ecc256 -c srk.ctx"
            f" && tpm2_load -Q -C srk.ctx -u {K}/sig.pub -r {K}/sig.priv -c sig.ctx"
            " && tpm2_startauthsession -Q --policy-session -S s.ctx"
            f" && dd-tpm-policy s.ctx {K}"
            " && tpm2_sign -Q -c sig.ctx -g sha256 -s ecdsa -f plain -p session:s.ctx -o sig m"
        )[0]
        == 0
    )


up()
assert secure_boot(), box.succeed("bootctl status")
# the firmware's event log, once the TPM's driver is up (securityfs says
# size 0; it is there to read)
box.wait_until_succeeds(
    "test $(head -c 64 /sys/kernel/security/tpm0/binary_bios_measurements | wc -c) = 64"
)

# its key to join with, made in its TPM under its approver, as its first
# own-keys boot makes it (disk-unlock-seal.sh)
box.succeed(f"dd-boot-key enrol -keys {K} -dir /run/bk")
assert signs(), "the join key does not open on the boot it was made in"
v1 = release("")

# 1. an EK not from its maker's CA: refused, by the box's own check and by
# the validators', with why
code, out = join("other", v1)
assert code != 0 and "refused j" in out, out
assert "its TPM is not one a TPM maker certified" in out, out
# the release's own roots (AMD, Intel, Infineon, ...) take no software TPM
code, out = join("fleet-roots", v1, software=False)
assert code != 0 and "its TPM is not one a TPM maker certified" in out, out

# 2. a good box joins: its EK chains to its maker, Secure Boot on under its
# own key, the release's systemd-boot and stub, its key in that TPM under
# this boot's approval
code, out = join("roots", v1)
assert code == 0 and "joined j" in out and '"secureBoot":true' in out, out

# 3. a PCR 4 the release does not list: a release whose boot is only the
# next one's (its kernel parameters differ, so its image does)
code, out = join("roots", release("update"))
assert code != 0 and "it did not start the fleet's own system" in out, out

# 4. an update: the box comes up again (dd-boot-settle approves what its ESP
# starts, unattended), then starts the new image. Its join key opens there,
# with no one doing anything, and it proves itself under the new release -
# not under the old one, which never listed this boot
box.shutdown()
box.start()
up()
assert signs(), "the join key does not open after a restart"
entry = box.succeed(
    "basename $(ls /boot/EFI/Linux/nixos-generation-1-specialisation-update-*.efi)"
).strip()
box.succeed(f"bootctl set-oneshot {entry}")
box.shutdown()
box.start()
up()
assert "dd.test=update" in box.succeed("cat /proc/cmdline")
assert secure_boot()
assert signs(), "the join key does not open on the updated boot"
code, out = join("roots", release(""))
assert code != 0 and "it did not start the fleet's own system" in out, out
code, out = join("roots", release("", "update"))
assert code == 0 and "joined j" in out, out

# 5. Secure Boot off (its keys cleared: setup mode, nothing checked): its
# key opens nothing, and the box says why
box.succeed("rm -rf /boot/loader/keys && sync")
box.shutdown()
os.remove(os.path.join(box.state_dir, "box-efi-vars.fd"))
box.start()
up()
assert not secure_boot()
assert not signs(), "the join key opened with Secure Boot off"
code, out = join("roots", release("", "update"))
assert code != 0 and "the box: Secure Boot is off" in out, out
