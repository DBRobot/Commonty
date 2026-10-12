# The takeover test: photos moves from a to b with nothing shared between
# them but ciphertext in each other's stores, and its accounts come along.
import json

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

# a hands over: a base backup and the key sealed for b, its last segment
# archived, everything copied to b's store
a.succeed("systemctl start dd-pg-base.service")
a.succeed("systemctl stop ente.service postgresql.service")
a.succeed("rm /run/dd-placed/postgresql /run/dd-placed/ente-museum")
a.succeed("systemctl start dd-store-copy.service")

# b takes epoch 2 from its own store: postgres restored, museum on keys of
# b's own that a never had
hold(b, 2)
b.succeed("systemctl start postgresql.service ente.service")
b.wait_until_succeeds("curl -sf http://127.0.0.1:8080/ping", timeout=180)
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
