# The metrics-ui test: the page for whoever is signed in, and nothing
# through to Thanos but the read-only query calls, from someone signed in,
# with their credential taken out.
import json

from waits import wait_fetch_has

start_all()

box.wait_for_unit("nginx.service")
box.wait_for_unit("thanos-stand-in.service")
box.wait_for_open_port(10903, addr="127.0.0.1")
box.wait_for_open_port(4181)

site = "https://metrics.test.invalid"
curl = "curl -sk --resolve metrics.test.invalid:443:127.0.0.1 --resolve grafana.test.invalid:443:127.0.0.1"


def status(args):
    return box.succeed(f"{curl} -o /dev/null -w '%{{http_code}}' {args}").strip()


def location(args):
    return box.succeed(f"{curl} -o /dev/null -w '%{{redirect_url}}' {args}").strip()


# nobody signed in: the page and the API both send them to sign in, and
# Thanos hears nothing
# (the page's answer is the gate's own redirect, the API's nginx's @login)
assert status(f"{site}/") in ("302", "303"), status(f"{site}/")
assert "/_dd/login" in location(f"{site}/"), location(f"{site}/")
assert status(f"'{site}/api/v1/query?query=up'") == "302"
assert "/_dd/login" in location(f"'{site}/api/v1/query?query=up'")
box.succeed("test ! -e /tmp/thanos.log")

# a member, with a session as the app's browser has
dirs = "--directory http://127.0.0.1:4181/_dd/directory"
box.succeed(
    f"DD_KEYRING_FILE=/root/sarah.json {nix['dd']} identity new --born 1990-01 --agree-terms --name sarah {dirs}"
)
box.succeed("mkdir -p /root/fleet/fleet")
box.succeed(
    f"DD_KEYRING_FILE=/root/sarah.json {nix['dd']} member add sarah --repo /root/fleet {dirs} --confirm $(DD_KEYRING_FILE=/root/sarah.json {nix['dd']} identity show {dirs} | sed -n 's/^member id: *//p')"
)
box.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    '"$(jq -c . /root/fleet/fleet/members.json)" '
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)
token = box.succeed(f"DD_KEYRING_FILE=/root/sarah.json {nix['dd']} token").strip()
head = box.succeed(
    f"curl -s -D - -o /dev/null -X POST --data-urlencode token={token} http://127.0.0.1:4181/_dd/app/signin"
)
cookie = (
    next(h for h in head.splitlines() if h.lower().startswith("set-cookie:"))
    .split(":", 1)[1]
    .split(";")[0]
    .strip()
)
assert cookie.startswith("dd_session="), cookie
sarah = f"-H 'Cookie: {cookie}'"

# the page is the gate's
page = box.succeed(f"{curl} -f {sarah} {site}/")
assert "metrics.js" in page, page

# each read-only call the page makes reaches Thanos, without her cookie or
# any Authorization she sent
for path in (
    "/api/v1/query?query=up",
    "/api/v1/query_range?query=up&start=0&end=60&step=15",
    "/api/v1/series?match[]=up",
    "/api/v1/labels",
    "/api/v1/label/job/values",
):
    out = box.succeed(
        f"{curl} -f -g {sarah} -H 'Authorization: Bearer not-for-thanos' '{site}{path}'"
    )
    assert '"success"' in out, (path, out)
box.succeed(f"{curl} -f {sarah} -X POST -d query=up {site}/api/v1/query")
seen = box.succeed("cat /tmp/thanos.log")
assert "GET /api/v1/query?query=up" in seen, seen
assert "GET /api/v1/label/job/values" in seen, seen
assert "POST /api/v1/query" in seen, seen
assert "dd_session" not in seen, seen
assert "not-for-thanos" not in seen, seen

# nothing else of Thanos: no other method, no admin API, no other path.
# Those land on the page or are refused, and Thanos never hears them
box.succeed("rm /tmp/thanos.log")
# refused, and like any refusal there, sent home
assert status(f"{sarah} -X DELETE '{site}/api/v1/query?query=up'") == "302"
assert status(f"{sarah} -X PUT '{site}/api/v1/labels'") == "302"
for path in (
    "/api/v1/admin/tsdb/delete_series?match[]=up",
    "/api/v1/admin/tsdb/clean_tombstones",
    "/api/v1/status/config",
    "/api/v1/stores",
    "/api/v1/query/../admin/tsdb/snapshot",
    "/-/reload",
):
    box.succeed(f"{curl} -g --path-as-is -X POST {sarah} '{site}{path}' -o /dev/null")
box.succeed("test ! -e /tmp/thanos.log")

# the demo gets neither the page nor a single query, and Thanos hears nothing
head = box.succeed("curl -s -D - -o /dev/null http://127.0.0.1:4181/_dd/demo")
jar = (
    next(h for h in head.splitlines() if h.lower().startswith("set-cookie:"))
    .split(":", 1)[1]
    .split(";")[0]
    .strip()
)
assert jar.startswith("dd_session="), jar
as_demo = f"-H 'Cookie: {jar}'"
assert status(f"{as_demo} {site}/") == "403", status(f"{as_demo} {site}/")
# the gate's 403 is nginx's @waiting: back to the home page
for args in (
    f"{as_demo} '{site}/api/v1/query?query=up'",
    f"{as_demo} -X POST -d query=up {site}/api/v1/query",
):
    assert status(args) == "302", (args, status(args))
    assert location(args) == "https://home.test.invalid/", location(args)
box.succeed("test ! -e /tmp/thanos.log")

# the old name sends people to the new one
assert status("https://grafana.test.invalid/d/x") == "301"
assert location("https://grafana.test.invalid/d/x") == f"{site}/d/x"

# ---- a joined box's metrics (joined-metrics.nix)
box.wait_for_unit("thanos-receive-joined.service")
box.wait_for_unit("thanos-query-joined.service")
box.wait_for_unit("control-stand-in.service")
box.wait_for_open_port(4195, addr="127.0.0.1")
for w in ("a", "b"):
    box.succeed(f"age-keygen -o /root/{w}.key")
ra = box.succeed("age-keygen -y /root/a.key").strip()
rb = box.succeed("age-keygen -y /root/b.key").strip()
box.succeed(
    "cat > /tmp/state.json <<EOF\n"
    + json.dumps(
        {
            "joined": {
                "jaaaaaaaaaa": {"recipient": ra},
                "jbbbbbbbbbb": {"recipient": rb},
            }
        }
    )
    + "\nEOF"
)
box.succeed("systemctl start dd-metrics-joined.service")
store = "https://store.box.test.invalid:3443"
scurl = "curl -sk --resolve store.box.test.invalid:3443:127.0.0.1"
# each token sealed to its own box, served beside the store's keys
for w, j in (("a", "jaaaaaaaaaa"), ("b", "jbbbbbbbbbb")):
    box.succeed(
        f"{scurl} -f {store}/_keys/{j}-metrics.age -o /tmp/{w}.age"
        f" && age -d -i /root/{w}.key /tmp/{w}.age > /var/lib/dd-metrics/{w}"
    )
box.fail("age -d -i /root/b.key /tmp/a.age")

# no token, or one nobody was given: refused
w = f"{scurl} -o /dev/null -w '%{{http_code}}' -X POST --data-binary x {store}/_metrics/write"
assert box.succeed(w).strip() == "403"
assert box.succeed(f"{w} -H 'Authorization: Bearer nope'").strip() == "403"

# what each sends is kept under its own name, whatever it says it is
box.systemctl("restart prometheus.service")


def series(tenant):
    out = box.succeed(
        f"curl -sf -H 'THANOS-TENANT: {tenant}' "
        "'http://127.0.0.1:10913/api/v1/query?query=up'"
    )
    return json.loads(out)["data"]["result"]


wait_fetch_has(
    box,
    "http://127.0.0.1:10913/api/v1/query?query=up",
    '"sender":"b"',
    timeout=180,
    curl_args="-sf -H 'THANOS-TENANT: jbbbbbbbbbb'",
)
wait_fetch_has(
    box,
    "http://127.0.0.1:10913/api/v1/query?query=up",
    '"sender":"a"',
    timeout=180,
    curl_args="-sf -H 'THANOS-TENANT: jaaaaaaaaaa'",
)
for tenant, sender in (("jaaaaaaaaaa", "a"), ("jbbbbbbbbbb", "b")):
    got = series(tenant)
    assert got, tenant
    for r in got:
        assert r["metric"].get("box") == tenant, (tenant, r)
        assert r["metric"].get("sender") == sender, (tenant, r)
# with no box named, nothing at all
out = box.succeed("curl -sf 'http://127.0.0.1:10913/api/v1/query?query=up'")
assert json.loads(out)["data"]["result"] == [], out

# the fleet's admin sees every box, a joined one too: through to its query,
# which answers for that box alone
args = f"{sarah} '{site}/joined/jaaaaaaaaaa/api/v1/query?query=up'"
got = json.loads(box.succeed(f"{curl} -f {args}"))
assert got["data"]["result"], got
assert all(r["metric"]["box"] == "jaaaaaaaaaa" for r in got["data"]["result"]), got

# another member, whose box it is not, is refused and sent home
box.succeed(
    f"DD_KEYRING_FILE=/root/bob.json {nix['dd']} identity new --born 1990-01 --agree-terms --name bob {dirs}"
)
box.succeed(
    f"DD_KEYRING_FILE=/root/sarah.json {nix['dd']} member add bob --repo /root/fleet {dirs}"
)
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    '"$(jq -c . /root/fleet/fleet/members.json)" '
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)
btoken = box.succeed(f"DD_KEYRING_FILE=/root/bob.json {nix['dd']} token").strip()
head = box.succeed(
    f"curl -s -D - -o /dev/null -X POST --data-urlencode token={btoken} http://127.0.0.1:4181/_dd/app/signin"
)
bcookie = (
    next(h for h in head.splitlines() if h.lower().startswith("set-cookie:"))
    .split(":", 1)[1]
    .split(";")[0]
    .strip()
)
bob = f"-H 'Cookie: {bcookie}'"
args = f"{bob} '{site}/joined/jaaaaaaaaaa/api/v1/query?query=up'"
assert status(args) == "302", status(args)
assert location(args) == "https://home.test.invalid/", location(args)
# and the admin, asking the same box by a path read two ways, is refused
args = f"{sarah} --path-as-is '{site}/api/v1/../../joined/jaaaaaaaaaa/api/v1/query?query=up'"
assert status(args) == "302", status(args)
