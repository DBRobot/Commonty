from waits import start_fresh, wait_unit_active

unit = "restic-backups-dd-computers.service"
home = "/var/lib/private/dd-computer/0123456789abcdef"
env = (
    "set -a; . /var/lib/dd-store/keys/backups-a.env; set +a; "
    f"export AWS_DEFAULT_REGION=us-east-1 RESTIC_PASSWORD_FILE={nix['password']} "
    "RESTIC_REPOSITORY=s3:http://127.0.0.1:3910/backups-a/computers; "
)

a.start()
a.wait_for_unit("multi-user.target")
wait_unit_active(a, "dd-store.service")
a.wait_until_succeeds("test -s /var/lib/dd-store/keys/backups-a.env", timeout=120)
a.wait_until_succeeds(
    "systemctl start dd-store-facts; test -s /var/lib/dd-facts/store.prom", timeout=120
)

# no computer here: nothing to do, and no failure (a skip is no run of its own)
a.succeed(f"systemctl start {unit}")
a.fail("test -e /var/lib/dd-facts/computers-backup.prom")

a.succeed(
    "zpool create -f -O com.sun:auto-snapshot=false tank /dev/vdb"
    " && mkdir -p /var/lib/private && chmod 700 /var/lib/private"
    " && zfs create -o mountpoint=/var/lib/private/dd-computer tank/computers"
    f" && mkdir {home}"
)

# disks bigger than the room it has: skipped, said, nothing written
a.succeed(f"dd if=/dev/urandom of={home}/home.qcow2 bs=1M count=300 && sync")
a.wait_until_succeeds(
    f"du -B1 {home}/home.qcow2 | awk '$1 > 200000000 {{ f = 1 }} END {{ exit !f }}'",
    timeout=60,
)
a.succeed(f"systemctl start {unit}")
a.succeed(
    """grep -c 'dd_store_copy_skipped{bucket="backups-a",box="a"} 1' /var/lib/dd-facts/computers-backup.prom"""
)
a.fail(f"{env} restic cat config")
assert a.succeed("zfs list -H -t snapshot tank/computers").strip() == ""

# room: its disk and record as of the snapshot, its system disk left out
a.succeed(
    f"dd if=/dev/urandom of={home}/home.qcow2 bs=1M count=4"
    f" && dd if=/dev/urandom of={home}/system.qcow2 bs=1M count=1"
    f' && echo \'{{"id":"x"}}\' > {home}/record.json'
)
want = a.succeed(f"sha256sum < {home}/home.qcow2").split()[0]
start_fresh(a, unit)
a.fail("test -e /var/lib/dd-facts/computers-backup.prom")
files = a.succeed(f"{env} restic ls latest")
assert "home.qcow2" in files and "record.json" in files, files
assert "system.qcow2" not in files, files
a.succeed(f"{env} restic restore latest --target /tmp/back")
got = a.succeed("find /tmp/back -name home.qcow2 -exec sha256sum {} +").split()[0]
assert got == want, (got, want)
# the snapshot was for the run, and the disk is as it was
assert a.succeed("zfs list -H -t snapshot tank/computers").strip() == ""
assert a.succeed(f"sha256sum < {home}/home.qcow2").split()[0] == want
