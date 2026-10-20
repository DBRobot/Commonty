# The pg-follow test: postgres moves with its lease, by the archive alone.
import time

from waits import journal_has, journal_of

start_all()
for m in (a, b):
    m.wait_for_unit("garage.service")
    m.wait_for_open_port(3901)
a_id = a.succeed(f"set -a; . {nix['rpc']}; garage node id -q").strip().split("@")[0]
b.succeed(f"set -a; . {nix['rpc']}; garage node connect {a_id}@a:3901")
for m in (a, b):
    m.succeed("systemctl restart garage-setup.service")
for m in (a, b):
    m.wait_until_succeeds(
        f"set -a; . {nix['rpc']}; garage layout show > /tmp/l && grep -q 'Current cluster layout version: [1-9]' /tmp/l"
    )
# the bucket and each box's key, once the layout has settled
for m in (a, b):
    m.wait_until_succeeds(
        f"systemctl restart garage-setup.service && set -a && . {nix['rpc']} "
        f'&& grep -q pg-{m.name} <<<"$(garage bucket info postgres-wal)"',
        timeout=180,
    )

# no flag, no postgres
for m in (a, b):
    m.fail("systemctl is-active postgresql.service")


def lease(m, epoch, base=True):
    m.succeed(
        f'mkdir -p /run/dd-placed && echo \'{{"epoch": {epoch}, "until": 9999999999999}}\' > /run/dd-placed/postgresql'
    )
    # its units, as dd-control would start them (carries.json)
    m.succeed("systemctl start postgresql.service postgresql-setup.service")
    m.wait_for_unit("postgresql-setup.service")
    if base:
        m.wait_until_succeeds(
            "systemctl show -p ActiveState --value dd-pg-base | grep -qx inactive"
        )
        m.succeed("systemctl show -p Result --value dd-pg-base | grep -qx success")


def sql(m, q):
    return m.succeed(f'sudo -u postgres psql -d app -tAc "{q}"').strip()


def archived(m):
    seg = sql(m, "select pg_walfile_name(pg_switch_wal())")
    m.wait_until_succeeds(
        f"sudo -u postgres psql -tAc \"select last_archived_wal >= '{seg}' from pg_stat_archiver\" | grep -qx t",
        timeout=60,
    )


def give_back(m):
    m.succeed("systemctl stop postgresql.service")
    m.succeed("rm /run/dd-placed/postgresql")


# epoch 1 on a: from nothing; its key made, and sealed to both boxes
lease(a, 1)
sql(a, "create table t (n int)")
sql(a, "insert into t select generate_series(1, 100)")
a.succeed(
    "set -a; . $(systemctl show -p EnvironmentFiles --value postgresql | cut -d' ' -f1); "
    "AWS_ENDPOINT=http://127.0.0.1:3900 AWS_S3_FORCE_PATH_STYLE=true AWS_REGION=us-east-1 "
    "WALG_S3_PREFIX=s3://postgres-wal/main wal-g st ls keys/ | tee /dev/stderr | grep -q b.age"
)
fingerprint = a.succeed(
    "set -a; . $(systemctl show -p EnvironmentFiles --value postgresql | cut -d' ' -f1); "
    "AWS_ENDPOINT=http://127.0.0.1:3900 AWS_S3_FORCE_PATH_STYLE=true AWS_REGION=us-east-1 "
    "WALG_S3_PREFIX=s3://postgres-wal/main wal-g st cat keys/fingerprint"
).strip()
# what the store has of it is ciphertext: no row of it is readable there
a.fail("grep -rq 'create table t' /srv/garage")
# handed over cleanly: the last rows go with the drain at stop
sql(a, "insert into t select generate_series(101, 120)")
give_back(a)

# a segment the archive has and b cannot fetch stops b's recovery, rather
# than end it early without the rows in it and after it
st = (
    "set -a; . $(systemctl show -p EnvironmentFiles --value postgresql | cut -d' ' -f1); "
    "AWS_ENDPOINT=http://127.0.0.1:3900 AWS_S3_FORCE_PATH_STYLE=true AWS_REGION=us-east-1 "
    "WALG_S3_PREFIX=s3://postgres-wal/main/e1 wal-g st"
)
last = a.succeed(
    f"{st} ls wal_005/ | grep -o '[0-9A-F]\\{{24\\}}\\.zst' | sort | tail -n1"
).strip()
assert last, "no segment archived"
a.succeed(f"{st} get --no-decrypt --no-decompress wal_005/{last} /tmp/kept")
a.succeed(
    f"echo unreadable > /tmp/bad && {st} put --no-encrypt --no-compress -f /tmp/bad wal_005/{last}"
)
b.succeed(
    'mkdir -p /run/dd-placed && echo \'{"epoch": 2, "until": 9999999999999}\' > /run/dd-placed/postgresql'
)
b.fail("systemctl start postgresql.service")
# it is restarted after it fails (Restart=), and each try stops at the same
# segment: a try that has ended (between tries, or given up) is read
end = time.monotonic() + 180
while True:
    sub = b.succeed("systemctl show -p SubState --value postgresql.service").strip()
    inv = b.succeed("systemctl show -p InvocationID --value postgresql.service").strip()
    if sub in ("auto-restart", "failed", "dead") and journal_has(
        b, "postgresql.service", inv, "could not be fetched: recovery stops here"
    ):
        break
    assert time.monotonic() < end, (sub, journal_of(b, "postgresql.service", inv))
    b.sleep(1)
# its tries ran into systemd's start limit: cleared for the start below
b.succeed(
    "systemctl stop postgresql.service && systemctl reset-failed postgresql.service"
)
a.succeed(f"{st} put --no-encrypt --no-compress -f /tmp/kept wal_005/{last}")

# epoch 2 on b, from the archive: every row, a's included, with the key
# a sealed to it - the same key, not a new one
# each fetch from the store slowed: postgres says it is up (read-only) once
# the base backup is consistent, while it still replays what came after;
# what it starts on then may not cut the replay short
b.succeed("tc qdisc add dev lo root netem delay 200ms")
lease(b, 2)
b.succeed("tc qdisc del dev lo root")
assert (
    b.succeed("sha256sum < /var/lib/dd-pg/key | cut -d' ' -f1").strip() == fingerprint
)
assert sql(b, "select count(*) from t") == "120", sql(b, "select count(*) from t")
sql(b, "insert into t select generate_series(121, 150)")
archived(b)

# a postgres whose lease moved on archives nothing more
b.succeed('echo \'{"epoch": 9, "until": 9999999999999}\' > /run/dd-placed/postgresql')
failed = sql(b, "select failed_count from pg_stat_archiver")
sql(b, "insert into t values (999)")
b.succeed("sudo -u postgres psql -tAc 'select pg_switch_wal()'")
b.wait_until_succeeds(
    f"sudo -u postgres psql -tAc 'select failed_count > {failed} from pg_stat_archiver' | grep -qx t"
)

# b dies. a takes epoch 3: what b archived at epoch 2, not the row it
# wrote after its lease moved on
b.crash()
# the two-box store takes no writes with b away; reads, and so the
# restore, go on (each box's own store, storage/store.nix, ends that)
lease(a, 3, base=False)
assert sql(a, "select count(*) from t") == "150", sql(a, "select count(*) from t")
assert sql(a, "select count(*) from t where n = 999") == "0"
# a's own data from epoch 1 is set aside, not deleted
a.succeed("ls -d /var/lib/postgresql/*.before-e3.* | grep -q .")
# and it writes on; its archive catches up once the store takes writes
sql(a, "insert into t values (151)")
# b is away a minute, as on a loaded runner (CI run 767): the store write a
# sent before it saw b go waits out garage's rpc timeout, not b's return
a.sleep(60)
b.start()
b.wait_for_unit("garage.service")
archived(a)
