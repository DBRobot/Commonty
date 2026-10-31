import json

from waits import start_fresh

j_name, store = nix["name"], nix["storeName"]
start_all()
a.wait_for_unit("dd-store-setup.service")
j.wait_for_unit("dd-store-setup.service")
j.wait_for_unit("nginx.service")
for unit in ["challtestsrv", "pebble", "fake-cloudflare", "dns-worker"]:
    j.wait_for_unit(unit)
j.wait_until_succeeds("curl -s -o /dev/null http://127.0.0.1:8787/", timeout=120)
j.wait_until_succeeds(
    "curl -s -o /dev/null --cacert /etc/ssl/certs/ca-certificates.crt https://localhost:14000/dir",
    timeout=60,
)
# every copy is one the test starts
a.wait_for_unit("timers.target")
a.succeed(
    "systemctl stop dd-store-copy.timer dd-store-copy-recent.timer "
    "dd-store-copy.service dd-store-copy-recent.service"
)

# what a box believes: pebble's own root, as a box believes Let's Encrypt's
j.wait_until_succeeds(
    "curl -sf --cacert /etc/ssl/certs/ca-certificates.crt https://localhost:15000/roots/0 -o /tmp/root.pem",
    timeout=60,
)
root = j.succeed("cat /tmp/root.pem")
a.succeed(f"cat > /run/test-ca.pem <<'EOF'\n{root}\nEOF")

# a keeps photos on j too, which the agreed state has as a box that joined
a.succeed(
    "mkdir -p /run/standin && cat > /run/standin/state <<'EOF'\n"
    + json.dumps({"joined": {j_name: {}}})
    + "\nEOF\n"
)
a.succeed(
    "systemd-run --unit standin python3 -m http.server 4195 --directory /run/standin --bind 127.0.0.1"
)
a.wait_until_succeeds("curl -sf http://127.0.0.1:4195/state", timeout=30)
a.succeed(f"""echo '{{"photos":["a","{j_name}"]}}' > /run/dd-placed/copies.json""")
s3 = "aws --endpoint-url http://127.0.0.1:3910"
a.succeed(
    "set -a; . /var/lib/dd-store/keys/photos.env; echo over-tls > /tmp/x; "
    f"AWS_DEFAULT_REGION=us-east-1 {s3} s3 cp /tmp/x s3://photos/over-tls"
)


def tls_verify():
    """curl's word on j's store from a: 60 is a certificate it does not believe"""
    return a.execute(
        f"curl -s -o /dev/null --max-time 10 --cacert /run/test-ca.pem https://{store}:3443/_keys/a.age"
    )[0]


def order():
    try:
        start_fresh(j, "acme-order-renew-test.invalid.service")
    except Exception:
        print(
            j.execute(
                "journalctl -b --no-pager -o short-monotonic -u 'dd-acme@*' -u dns-worker"
                " -u fake-cloudflare | tail -n 80;"
                " journalctl -b --no-pager -o cat -u acme-order-renew-test.invalid | tail -n 30"
            )[1]
        )
        raise


def agreed(body):
    j.succeed(
        "curl -sf -X POST -H 'content-type: application/json' http://127.0.0.1:8899/control"
        f" --data-binary '{json.dumps(body)}'"
    )


spki = j.succeed(f"cat {nix['spki']}").strip()
ipv4 = (100 << 24) | (64 << 16) | 9

with subtest("not in the agreed state: no certificate, and the copy fails on TLS"):
    agreed({"fresh": True, "joined": {}, "placed": {}})
    j.execute("systemctl start acme-order-renew-test.invalid.service")
    assert tls_verify() == 60, tls_verify()
    a.execute("systemctl start dd-store-copy.service")
    run = a.succeed("systemctl show -p InvocationID --value dd-store-copy").strip()
    a.wait_until_succeeds(
        f"journalctl -o cat _SYSTEMD_INVOCATION_ID={run} | grep -cF 'no key from its store yet'",
        timeout=30,
    )

with subtest(
    "in the agreed state: its store's name, and only that, on its certificate"
):
    agreed(
        {
            "fresh": True,
            "joined": {j_name: {"ipv4": ipv4, "key": spki}},
            "placed": {},
        }
    )
    order()
    cert = "/var/lib/acme/test.invalid/cert.pem"
    assert "Pebble" in j.succeed(f"openssl x509 -in {cert} -noout -issuer")
    sans = j.succeed(f"openssl x509 -in {cert} -noout -ext subjectAltName | tail -n +2")
    assert sans.strip() == f"DNS:{store}", sans
    a.wait_until_succeeds(
        f"curl -sf -o /dev/null --max-time 10 --cacert /run/test-ca.pem https://{store}:3443/_keys/a.age",
        timeout=60,
    )

with subtest("a copy goes in over the store's port, and is there"):
    start_fresh(a, "dd-store-copy.service")
    assert (
        j.succeed(
            "set -a; . /var/lib/dd-store/keys/photos.env; "
            f"AWS_DEFAULT_REGION=us-east-1 {s3} s3 cp s3://photos/over-tls -"
        ).strip()
        == "over-tls"
    )


def asked(name):
    """The Worker's word on a name, past the box's own helper's list"""
    return j.succeed(
        f"BOX={j_name} WORKER=http://127.0.0.1:8787 KEYS=/nonexistent SOFT_KEY={nix['softKey']}"
        f" NAMES={name} bash {nix['ask']} <<<'present _acme-challenge.{name}. {'A' * 43}'"
        " || true"
    ).strip()


with subtest("a certificate for a name it was not given is refused"):
    for name in ["home.test.invalid", "games.test.invalid", "store.a.test.invalid"]:
        out = asked(name)
        assert "not a name this box serves" in out, (name, out)
    assert j.succeed("curl -s http://127.0.0.1:8899/records").strip() == "[]"

with subtest(
    "the plan gives it a service: that service's name, and it goes when the plan does"
):
    placed = {"dd-games": [j_name]}
    joined = {j_name: {"ipv4": ipv4, "key": spki}}
    agreed({"fresh": True, "joined": joined, "placed": placed})
    assert asked("games.test.invalid") == "ok"
    agreed({"fresh": True, "joined": joined, "placed": {}})
    assert "not a name this box serves" in asked("games.test.invalid")
