# The net-follow test: the network's database moves from a to b, sealed so
# only the boxes able to run the server can open it.
import json

start_all()
for m in (a, b, c):
    m.wait_for_unit("dd-store-setup.service")
    m.wait_for_unit("nginx.service")
    # the plan's copies, as dd-control writes them: c keeps one too
    m.succeed("""echo '{"headscale":["a","b","c"]}' > /run/dd-placed/copies.json""")


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
a.succeed("systemctl start dd-headscale-copy.service")
a.fail("journalctl -u dd-headscale-copy.service | grep -q 'not copied this time'")

# a hands over, its copy out to b's and c's stores
a.succeed("systemctl stop headscale.service")
a.succeed("rm /run/dd-placed/headscale")
a.succeed("systemctl start dd-store-copy.service")

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
assert "tom" in users(b), users(b)
ka = a.succeed("cat /var/lib/headscale/noise_private.key")
kb = b.succeed("cat /var/lib/headscale/noise_private.key")
assert ka != kb, "b runs on a's server key"
