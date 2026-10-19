# The join key across updates, against swtpm (nix/tests/boot-key.nix).
# OVMF measures into PCR 4 each boot application it starts: systemd-boot,
# then the kernel systemd-boot loads. A new boot image here is a new
# systemd-boot, as lzbt's update of it is on a real box.
k = "/boot/dd/unlock"
opts = f"-keys {k} -dir /run/bk"
loaders = ["/boot/EFI/systemd/systemd-bootx64.efi", "/boot/EFI/BOOT/BOOTX64.EFI"]


def up():
    box.wait_for_unit("multi-user.target")


def signs():
    return box.execute("bash /etc/sign.sh")[0] == 0


def epoch():
    return box.succeed(
        "tpm2_nvread 0x01500DD0 -C o 2>/dev/null | od -An -tx1 | tr -d ' \\n'"
    )


def digest(f):
    return box.succeed(f"dd-boot-key digest {f}").strip()


def boot_with(f):
    for l in loaders:
        box.succeed(f"cp {f} {l}")
    box.shutdown()
    box.start()
    up()
    apps = box.succeed("dd-boot-key apps").split()
    assert apps == [digest(f), digest(kernel)], f"{apps}: not booted through {f}"


box.start()
up()
box.succeed(f"dd-boot-key enrol -keys {k} -dir /run/bk")
assert signs(), "the join key does not open on the boot it was made in"
entry = box.succeed(
    "bootctl status 2>/dev/null | sed -n 's/^ *Current Entry: *//p'"
).strip()
kernel = (
    "/boot"
    + box.succeed(f"sed -n 's/^linux *//p' /boot/loader/entries/{entry}").strip()
)
apps = box.succeed("dd-boot-key apps").split()
assert apps == [digest(loaders[0]), digest(kernel)], apps

# the boot images: this one, and two others (systemd-boot with bytes
# after it: another Authenticode digest, the same program), one approved
# ahead and one never
box.succeed(f"cp {loaders[0]} /root/sd-old.efi")
for name, pad in (("new", 4096), ("bad", 8192)):
    box.succeed(
        f"cp /root/sd-old.efi /root/sd-{name}.efi && head -c {pad} /dev/zero >>/root/sd-{name}.efi"
    )
box.succeed(f"dd-boot-key approve {opts} -boot /root/sd-new.efi,{kernel}")
assert signs(), "approving the next boot closed this one"
before = epoch()

# 1. the update: the key opens under the new PCR 4, approved ahead
boot_with("/root/sd-new.efi")
assert signs(), "the join key does not open on the approved new boot"
box.succeed(f"dd-boot-key settle {opts}")
assert epoch() != before, "settle did not move the epoch on"
assert signs(), "the join key does not open after settling"

# 2. rollback: the first boot is still among the good ones
boot_with("/root/sd-old.efi")
assert signs(), "the join key does not open on the rolled-back boot"
box.succeed(f"dd-boot-key settle {opts}")

# 3. a boot never approved (any other loader in PCR 4): no key, and it
# cannot approve itself - the approver is sealed under the same rule
boot_with("/root/sd-bad.efi")
out = box.fail("bash /etc/sign.sh 2>&1")
assert "not one the box approved" in out, out
box.fail(f"dd-boot-key approve {opts} -boot /root/sd-bad.efi,{kernel}")
assert "not approved" in box.succeed(f"dd-boot-key settle {opts}")

# 4. and back on a good boot, the key opens again
boot_with("/root/sd-old.efi")
assert signs(), "the join key does not open on a good boot after a bad one"
