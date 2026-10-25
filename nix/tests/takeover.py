# The takeover test: photos moves from a to b with nothing shared between
# them but ciphertext in each other's stores, and its accounts come along.
import json
import time

from waits import journal_has, journal_of, start_fresh

start_all()
for m in (a, b):
    m.wait_for_unit("dd-store-setup.service")
    m.wait_for_unit("nginx.service")
    # the plan's copies, as dd-control writes them
    m.succeed(
        """echo '{"ente":["a","b"],"postgres-wal":["a","b"]}' > /run/dd-placed/copies.json"""
    )

# a store sized by its disk says so, room left for the box's own
a.succeed("systemctl start dd-store-facts.service")
size = int(
    a.succeed("awk '/^dd_store_size_bytes/ {print $2}' /var/lib/dd-facts/store.prom")
)
assert 0 < size < 4 * 2**30, size

email = "tom@users.test.invalid"
as_json = "-H 'content-type: application/json'"


def hold(m, epoch):
    flag = json.dumps({"epoch": epoch, "until": 10**12})
    for svc in ("postgresql", "ente-museum"):
        m.succeed(f"echo '{flag}' > /run/dd-placed/{svc}")


def sign_in(m):
    """the code museum takes for a member's own address, as the gate sends it"""
    m.succeed(
        f"curl -sf -X POST {as_json} "
        f'-d \'{{"email": "{email}", "purpose": "signup"}}\' http://127.0.0.1:8080/users/ott'
    )
    ott = m.succeed("cat /var/lib/dd-ente-keys/ott").strip()
    return json.loads(
        m.succeed(
            f"curl -sf -X POST {as_json} "
            f'-d \'{{"email": "{email}", "ott": "{ott}"}}\' http://127.0.0.1:8080/users/verify-email'
        )
    )


def hand_over(m, svc, units, writer):
    """as dd-control hands a writer over (placed.go): stopped; its flag
    marked handed over, so its units refuse to start; its last writes copied
    out while the flag still says the lease is this box's; and only then the
    flag gone"""
    m.succeed(f"systemctl stop {units}")
    flag = f"/run/dd-placed/{svc}"
    m.succeed(f"jq -c '.handing = true' {flag} > {flag}.new && mv {flag}.new {flag}")
    # a start now (a timer's, a dependency's) is refused
    m.succeed(f"systemctl start {writer}")
    m.fail(f"systemctl is-active {writer}")
    # no timer starts it, so no run of it is under way; it fails unless all
    # of it was copied (an instance is unloaded once it ends: nothing to show)
    m.succeed(f"systemctl start dd-store-copy-final@{svc}.service")
    m.fail(f"systemctl is-active {units}")
    m.succeed(f"rm {flag}")


def copy_out(m):
    """a copy run of its own, until one sends everything: one that found the
    other store not ready yet says so"""
    end = time.monotonic() + 180
    while True:
        inv = start_fresh(m, "dd-store-copy.service")
        if not journal_has(m, "dd-store-copy.service", inv, "not copied this time"):
            return
        assert time.monotonic() < end, journal_of(m, "dd-store-copy.service", inv)
        m.sleep(2)


def psql(m, q):
    return m.succeed(f'runuser -u postgres -- psql -d ente -tAc "{q}"').strip()


# a holds epoch 1: postgres and museum, with keys a made
hold(a, 1)
a.succeed("systemctl start postgresql.service ente.service")
a.wait_until_succeeds("curl -sf http://127.0.0.1:8080/ping", timeout=180)
tom = sign_in(a)["id"]
# museum keeps the address beside the account, within a minute
a.wait_until_succeeds(
    f"[ \"$(runuser -u postgres -- psql -d ente -tAc 'select dd_email from users where user_id = {tom}')\" = '{email}' ]",
    timeout=90,
)

# a base backup and the key sealed for b, copied to b's store while a
# holds the flags: the store copies only for the box that writes
a.succeed("systemctl start dd-pg-base.service")
copy_out(a)
# from here only the handover copies anything: no timer does it behind it
a.succeed("systemctl stop dd-store-copy.timer dd-store-copy-recent.timer")
# committed the moment before the handover: in no copy yet
a.succeed("runuser -u postgres -- createdb late")
a.succeed(
    "runuser -u postgres -- psql -d late -c 'create table t (n int); insert into t values (42)'"
)
# stopped as dd-control's handover begins. The hourly dump coming due
# mid-stop (CI, 14:00) once cancelled the stop and brought postgres back: it
# may not start postgres at all
a.succeed("systemctl stop --no-block ente.service postgresql.service")
a.succeed("systemctl start --no-block postgresqlBackup-ente.service")
a.wait_until_succeeds(
    '[ "$(systemctl list-jobs --no-legend | grep -c -e ente -e postgres)" = 0 ]',
    timeout=120,
)
a.succeed('[ "$(systemctl show -P ActiveState postgresql.service)" = inactive ]')
# the dump waited for the stop and found nothing to dump: skipped, not failed
a.succeed(
    '[ "$(systemctl show -P Result postgresqlBackup-ente.service)" = exec-condition ]'
)
# and the last segment went within the stop, not 30s of archive_timeout later
a.succeed(
    "[ $(journalctl -u postgresql.service -o cat | grep -c 'not archived within') = 0 ]"
)

# a hands over: postgres archives its last segment on the way down, and
# that goes out to b with the rest
hand_over(a, "ente-museum", "ente.service ente-health.service", "ente.service")
hand_over(a, "postgresql", "postgresql.service", "postgresql.service")

# b takes epoch 2 from its own store: postgres restored, museum on keys of
# b's own that a never had. Museum's lease comes first and postgres's after,
# as on 2026-10-24: museum, which gave up after five quick tries then and
# kept Photos down, waits postgres out by itself
flag = json.dumps({"epoch": 2, "until": 10**12})
b.succeed(f"echo '{flag}' > /run/dd-placed/ente-museum")
b.succeed("systemctl start --no-block ente.service")
b.wait_until_succeeds(
    '[ "$(systemctl show -P NRestarts ente.service)" -ge 3 ]', timeout=120
)
b.fail("systemctl is-failed --quiet ente.service")
hold(b, 2)
b.succeed("systemctl start postgresql.service")
b.wait_until_succeeds("curl -sf http://127.0.0.1:8080/ping", timeout=180)
assert b.succeed("systemctl show -P Result ente.service").strip() == "success"
ka = a.succeed("cat /var/lib/dd-ente-keys/encryption /var/lib/dd-ente-keys/hash")
kb = b.succeed("cat /var/lib/dd-ente-keys/encryption /var/lib/dd-ente-keys/hash")
assert ka != kb, "b has a's keys"
b.succeed(
    "journalctl -u ente.service | grep -q 'dd rekey: new keys; 1 accounts carried over'"
)
# the same address, the same account
again = sign_in(b)
assert again["id"] == tom, (again, tom)
# and nothing else came along to b but the account and its rows
assert psql(b, "select count(*) from users") == "1"
# the commit made the moment before the handover is here
assert (
    b.succeed("runuser -u postgres -- psql -d late -tAc 'select n from t'").strip()
    == "42"
)
