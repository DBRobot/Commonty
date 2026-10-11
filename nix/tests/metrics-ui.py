# The metrics-ui test: the page for whoever is signed in, and nothing
# through to Thanos but the read-only query calls, from someone signed in,
# with their credential taken out.
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
    f"DD_KEYRING_FILE=/root/sarah.json {nix['dd']} identity new --name sarah {dirs}"
)
box.succeed("mkdir -p /root/fleet/fleet")
box.succeed(
    f"DD_KEYRING_FILE=/root/sarah.json {nix['dd']} member add sarah --repo /root/fleet {dirs}"
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
