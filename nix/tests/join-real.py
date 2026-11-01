# See join-real.nix. Reports what the build took: join-real.json in the
# driver's output directory, and in the log.
import atexit
import json
import os
import shutil
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

work = Path(tempfile.mkdtemp(prefix="join-real-"))
atexit.register(shutil.rmtree, work, ignore_errors=True)
out_dir = Path(os.environ.get("out", os.getcwd()))
minutes = int(os.environ.get("JOIN_REAL_MINUTES", "60"))
report = {}


def run(*args):
    subprocess.run([str(a) for a in args], check=True)


# the fleet: what it holds is read-only and signed by its own key
web = subprocess.Popen(
    [nix["python"], "-m", "http.server", "--bind", "127.0.0.1", "8080"],
    cwd=nix["fleet"],
    stdout=subprocess.DEVNULL,
    stderr=subprocess.DEVNULL,
)
atexit.register(web.kill)
if os.environ.get("JOIN_REAL_CACHE"):
    cache = subprocess.Popen(
        [nix["nixServe"], "--listen", "127.0.0.1:8081"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    atexit.register(cache.kill)
# this run's own, not one a run before it left on the port
want = Path(nix["fleet"], "release.pub").read_bytes()
for _ in range(100):
    assert web.poll() is None, "the port is taken"
    try:
        with urllib.request.urlopen("http://127.0.0.1:8080/release.pub") as r:
            assert r.read() == want
        break
    except OSError:
        time.sleep(0.1)  # sleep-ok: a server of ours starting, in this process's sight

# a USB stick of 16 GB, as image.nix makes it and the app writes it
image = next(Path(nix["drive"]).glob("*.raw.zst"))
run(nix["zstd"], "-q", "-d", image, "-o", work / "usb.raw")
os.chmod(work / "usb.raw", 0o644)
os.truncate(work / "usb.raw", 16 * 10**9)
# an internal disk at the minimum
run(nix["qemuImg"], "create", "-q", "-f", "qcow2", work / "disk.qcow2", "130G")
run("cp", "--no-preserve=mode", nix["vars"], work / "vars.fd")
run("cp", "--no-preserve=mode", nix["factoryVars"], work / "factory.fd")
(work / "tpm").mkdir()


def machine_at(name, vars_file, usb, network):
    tpm = subprocess.Popen(  # ends with QEMU's connection (terminate)
        [
            nix["swtpm"],
            "socket",
            "--tpm2",
            "--tpmstate",
            f"dir={work}/tpm",
            "--ctrl",
            f"type=unixio,path={work}/tpm.sock,terminate",
        ]
    )
    cmd = (
        f"{nix['qemu']} -enable-kvm -machine q35,smm=on -cpu max -m 8192 -smp 4"
        " -global driver=cfi.pflash01,property=secure,value=on"
        f" -drive if=pflash,format=raw,unit=0,readonly=on,file={nix['code']}"
        f" -drive if=pflash,format=raw,unit=1,file={vars_file}"
        f" -chardev socket,id=chrtpm,path={work}/tpm.sock"
        " -tpmdev emulator,id=tpm0,chardev=chrtpm -device tpm-tis,tpmdev=tpm0"
        f" -drive if=none,id=disk,format=qcow2,file={work}/disk.qcow2"
        " -device nvme,serial=joinreal,drive=disk,bootindex=2"
        f" -netdev user,id=net0{network} -device virtio-net-pci,netdev=net0"
    )
    if usb:
        cmd += (
            f" -drive if=none,id=usb,format=raw,file={work}/usb.raw"
            " -device qemu-xhci -device usb-storage,drive=usb,removable=on,bootindex=1"
        )
    return create_machine(cmd, name=name), tpm


# a PC as it comes: its firmware refuses the drive until its keys are cleared,
# which is why the app's steps say so
factory, tpm = machine_at("factory", work / "factory.fd", True, "")
factory.start()
factory.wait_for_console_text("Access Denied")
factory.crash()
tpm.wait()

# keys cleared: it starts from the drive, to its screen
box, tpm = machine_at("box", work / "vars.fd", True, "")
box.start()
box.wait_for_unit("multi-user.target")
box.wait_for_unit("dd-box-setup.service")
box.wait_until_succeeds("pgrep -x firefox || pgrep -f firefox-bin", timeout=300)
box.wait_for_open_port(8090)
box.succeed("findmnt -no SOURCE / | grep -cE '/dev/sd|by-partlabel'")
ticket = Path(nix["fleet"], "ticket").read_text()
box.succeed(
    f"install -d /boot/commonty && printf %s '{ticket}' > /boot/commonty/ticket"
)
box.succeed("head -c 32 /dev/urandom | base64 > /boot/commonty/backup.key")

token = box.succeed("cat /run/dd-setup-token/token").strip()
mine = f"-H 'x-setup-token: {token}'"


def api(path, body=None):
    if body is None:
        out = box.succeed(f"curl -sf {mine} http://127.0.0.1:8090/api/{path}")
    else:
        out = box.succeed(
            f"curl -sf {mine} -H 'content-type: application/json' "
            f"-d '{json.dumps(body)}' http://127.0.0.1:8090/api/{path}"
        )
    return json.loads(out)


c = api("checks")
assert c["checks"]["uefi"] and c["checks"]["tpm"] and c["checks"]["setup_mode"], c
assert not c["checks"]["secure_boot"], c
assert c["drive"]["ticket"], c
# the internal disk, and never the stick it runs from
disks = [d["name"] for d in api("disks")["disks"]]
assert disks == ["/dev/nvme0n1"], disks

# what memory the build leaves, every five seconds, from outside its service
box.succeed(
    """cat > /run/sample.sh <<'EOF_SAMPLE'
PATH=/run/current-system/sw/bin
while :; do
  m=$(awk '/^(MemAvailable|SwapFree|SwapTotal):/ { printf "%s ", $2 }' /proc/meminfo)
  top=$(ps -eo rss=,comm= --sort=-rss | head -1)
  echo "$(date +%s) $m$top $(df --output=used /tmp | tail -1) $(df --output=used /mnt | tail -1)" >> /run/memory.log
  sleep 5
done
EOF_SAMPLE
systemd-run --unit=sample --collect bash /run/sample.sh"""
)

began = time.monotonic()
api("install", {"disk": disks[0]})
built_at = None
shown = 0.0
seen_numbers = False
while True:
    j = api("job")
    b = j["build"]
    if b["to_build"] or b["to_fetch"]:
        seen_numbers = True
    if built_at is None and len(j["stages"]) >= 4:
        built_at = time.monotonic()
    now = time.monotonic()
    if now - shown > 300:
        shown = now
        print(
            f"after {int(now - began)}s: {j['done']:.3f} eta={j['eta']} {b}", flush=True
        )
        # what it is doing, for when it does not move
        print(
            box.succeed(
                "{ ps -eo pcpu,rss,etimes,args --sort=-pcpu | head -6 | cut -c1-300; "
                "free -m; df -m /tmp /mnt; } 2>&1 || true"
            ),
            flush=True,
        )
    if j["state"] != "running":
        break
    assert now - began < minutes * 60, f"not installed after {minutes} minutes: {j}"
    time.sleep(10)  # sleep-ok: a long build's own poll
print(j)
assert j["state"] == "done", j
assert seen_numbers, "the screen had no numbers for the build"
assert any("installed" in s for s in j["stages"]), j

# what the build took
log = box.succeed("cat /run/memory.log")
(out_dir / "memory.log").write_text(log)
# time, SwapTotal, SwapFree, MemAvailable (as /proc/meminfo orders them), the
# biggest process's rss and name, /tmp's and /mnt's used
samples = [line.split() for line in log.splitlines()]
samples = [s for s in samples if len(s) == 8 and s[7].isdigit()]
biggest = max(samples, key=lambda s: int(s[4]))
report.update(
    seconds=int(time.monotonic() - began),
    built=b["built"],
    to_build=b["to_build"],
    fetched=b["fetched"],
    to_fetch=b["to_fetch"],
    min_available_kb=min(int(s[3]) for s in samples),
    max_rss_kb=int(biggest[4]),
    biggest=biggest[5],
    swap_total_kb=int(samples[0][1]),
    min_swap_free_kb=min(int(s[2]) for s in samples),
    max_tmp_kb=max(int(s[6]) for s in samples),
    disk_used_kb=max(int(s[7]) for s in samples),
)
print("join-real:", json.dumps(report))
(out_dir / "join-real.json").write_text(json.dumps(report))

# what was installed: the real system's, signed, with the ticket beside it
system = box.succeed("readlink -f /mnt/nix/var/nix/profiles/system").strip()
box.succeed(f"test -e /mnt{system}/init")
assert json.loads(box.succeed("cat /mnt/var/lib/dd/box/box.json"))["owner"] is None
box.succeed("test -s /mnt/var/lib/dd/join-ticket")
box.succeed('test -z "$(ls -A /mnt/var/tmp/dd-build 2>/dev/null)"')
box.succeed("sync; umount -R /mnt && zpool export tank")
box.shutdown()
tpm.wait()
web.terminate()
web.wait()

# the drive out, and no network to any fleet: nothing here may reach a real one
target, tpm = machine_at("target", work / "vars.fd", False, ",restrict=on")
target.start()
target.wait_for_unit("multi-user.target")
target.succeed(
    "[ \"$(od -An -tu1 -j4 -N1 /sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c | tr -d ' ')\" = 1 ]"
)
# what it started is what the release built, under its own keys
assert target.succeed("readlink -f /run/current-system").strip() == system
target.succeed("sbctl status | grep -cE 'Setup Mode.*Disabled'")
target.succeed("journalctl -b -u dd-disk-key --no-pager | grep -c 'first boot'")
target.wait_until_succeeds(
    "journalctl -b -u dd-unlock-seal --no-pager | grep -c 'sealed under this box'",
    timeout=300,
)
assert target.succeed("zfs get -H -o value keystatus tank/enc").strip() == "available"
target.succeed("test -s /var/lib/dd/box-key.spki")
# its ticket reads as a claim: it would otherwise exit, and restart for ever
target.wait_for_unit("dd-control.service")
assert "/var/lib/dd/join-ticket" not in target.succeed(
    "journalctl -b -u dd-control --no-pager"
)
print(target.succeed("systemctl --failed --no-legend --no-pager || true"))
target.shutdown()
tpm.wait()
