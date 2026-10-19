# The net-follow test: the network's database moves from a to b, sealed so
# only the boxes able to run the server can open it.
import json
import time

from waits import journal_has, journal_of, start_fresh

start_all()
for m in (a, b, c):
    m.wait_for_unit("dd-store-setup.service")
    m.wait_for_unit("nginx.service")
    # the plan's copies, as dd-control writes them: c keeps one too
    m.succeed("""echo '{"headscale":["a","b","c"]}' > /run/dd-placed/copies.json""")


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


def hold(m, epoch):
    flag = json.dumps({"epoch": epoch, "until": 10**12})
    m.succeed(f"echo '{flag}' > /run/dd-placed/headscale")


def up(m):
    m.succeed("systemctl start headscale.service headscale-seed.service")
    m.wait_until_succeeds("headscale users list >/dev/null", timeout=120)


def users(m):
    return {
        u["name"]
        for u in json.loads(m.succeed("headscale users list -o json") or "[]") or []
    }


# a holds epoch 1: a member's user and a key on it
hold(a, 1)
up(a)
a.succeed("headscale users create tom")
a.succeed(
    "headscale preauthkeys create --user $(headscale users list -o json | jq -r '.[] | select(.name == \"tom\") | .id') --expiration 1h"
)
inv = start_fresh(a, "dd-headscale-copy.service")
assert not journal_has(a, "dd-headscale-copy.service", inv, "not copied this time")
# copied to b's and c's stores while a holds the flag: the store copies
# only for the box that writes
copy_out(a)
# from here only the handover copies anything: no timer does it behind it
a.succeed(
    "systemctl stop dd-store-copy.timer dd-store-copy-recent.timer dd-headscale-copy.timer"
)
# made the moment before the handover: in no copy yet
a.succeed("headscale users create late")

# a hands over as dd-control does (placed.go): stopped, its database into
# the store and out to b's and c's while its flag is up, then the flag gone
a.succeed("systemctl stop headscale.service")
a.succeed(
    "jq -c '.handing = true' /run/dd-placed/headscale > /run/dd-placed/headscale.new "
    "&& mv /run/dd-placed/headscale.new /run/dd-placed/headscale"
)
# marked handed over: a start now is refused
a.succeed("systemctl start headscale.service")
a.fail("systemctl is-active headscale.service")
# no timer starts it, so no run of it is under way; it fails unless all of
# it was copied
a.succeed("systemctl start dd-store-copy-final@headscale.service")
a.succeed("rm /run/dd-placed/headscale")

s3 = "aws --endpoint-url http://127.0.0.1:3910"
fetch = (
    "set -a; . /var/lib/dd-store/keys/headscale.env; export AWS_DEFAULT_REGION=us-east-1; "
    f"{s3} s3 cp s3://headscale/db.sqlite.age /tmp/db.age"
)
# c keeps the sealed copy and cannot open it
c.succeed(fetch)
c.fail("age -d -i /etc/age-identity -o /tmp/db.sqlite /tmp/db.age")

# b takes epoch 2: the network as a left it, with a server key of its own
hold(b, 2)
up(b)
b.succeed("journalctl -u headscale.service | grep -q 'the network as epoch 1 left it'")
assert {"tom", "late"} <= users(b), users(b)
ka = a.succeed("cat /var/lib/headscale/noise_private.key")
kb = b.succeed("cat /var/lib/headscale/noise_private.key")
assert ka != kb, "b runs on a's server key"
