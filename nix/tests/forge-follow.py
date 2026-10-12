# The forge-follow test: git moves from a to b by copies in each other's
# stores, and nothing deleted comes back.
import json

start_all()
for m in (a, b):
    m.wait_for_unit("dd-store-setup.service")
    m.wait_for_unit("zfs-datasets.service")
    m.wait_for_unit("nginx.service")
    # the plan's copies, as dd-control writes them
    m.succeed(
        """echo '{"forge":["a","b"],"postgres-wal":["a","b"]}' > /run/dd-placed/copies.json"""
    )

sock, dir = nix["sock"], nix["dir"]


def hold(m, epoch):
    flag = json.dumps({"epoch": epoch, "until": 10**12})
    for svc in ("postgresql", "forgejo"):
        m.succeed(f"echo '{flag}' > /run/dd-placed/{svc}")


def api(m, method, path, body=None, ok=True):
    data = (
        f"-H 'content-type: application/json' -d '{json.dumps(body)}' " if body else ""
    )
    cmd = (
        f"curl -s --unix-socket {sock} -X {method} -H 'X-WEBAUTH-USER: tester' {data}"
        f"-o /tmp/out -w '%{{http_code}}' http://forgejo/api/v1{path}"
    )
    code = int(m.succeed(cmd).strip())
    if ok:
        assert 200 <= code < 300, (method, path, code, m.succeed("cat /tmp/out"))
    return code


def up(m):
    # a take that finds no base backup copied here yet refuses, rightly:
    # ask again until the copy has arrived
    m.wait_until_succeeds(
        "systemctl reset-failed postgresql.service forgejo.service forgejo-admin.service; "
        "systemctl start postgresql.service forgejo.service forgejo-admin.service",
        timeout=300,
    )
    m.wait_until_succeeds(f"test -S {sock}", timeout=180)
    m.wait_until_succeeds(
        f"curl -sf --unix-socket {sock} http://forgejo/api/healthz", timeout=180
    )


# b had files of its own on its dataset before it ever held the forge
b.succeed(f"echo b-was-here > {dir}/b-own-marker")

# a holds epoch 1: postgres, and forgejo on its dataset
hold(a, 1)
up(a)
for repo in ("kept", "gone"):
    api(
        a,
        "POST",
        "/user/repos",
        {"name": repo, "default_branch": "main", "auto_init": True},
    )
api(
    a,
    "POST",
    "/repos/tester/kept/contents/hello.txt",
    {"content": "aGVsbG8gZnJvbSBhCg==", "message": "on a"},
)
a.succeed("systemctl start dd-pg-base.service")
# (a copy taken before the new repositories were on disk lists them next time)
a.wait_until_succeeds(
    "systemctl start dd-forge-copy.service && grep -q gone.git /var/lib/dd-forge/manifest",
    timeout=120,
)
# a repository deleted on a: gone from the next copy's list
api(a, "DELETE", "/repos/tester/gone")
a.wait_until_succeeds(
    "systemctl start dd-forge-copy.service && ! grep -q gone.git /var/lib/dd-forge/manifest",
    timeout=120,
)
manifest = a.succeed("cat /var/lib/dd-forge/manifest")
assert "kept.git" in manifest and "gone.git" not in manifest, manifest
a.fail("journalctl -u dd-forge-copy.service | grep -q 'not copied this time'")

# a hands over: stopped, its last segment archived, everything copied to b
# stopped first, the flags after, as dd-control does: postgres archives its
# last segment on the way down only while its flag says the lease is its
a.wait_until_succeeds("systemctl stop forgejo.service postgresql.service", timeout=120)
a.succeed("rm /run/dd-placed/postgresql /run/dd-placed/forgejo")
a.wait_until_succeeds(
    # a copy that found the other store not ready says so: until none does
    "systemctl start dd-store-copy.service && "
    "! journalctl _SYSTEMD_INVOCATION_ID=$(systemctl show -p InvocationID --value dd-store-copy.service) "
    "| grep -q 'not copied this time'",
    timeout=180,
)

# b takes epoch 2 from its own store
hold(b, 2)
up(b)
b.succeed("journalctl -u forgejo.service | grep -q 'restoring the copy of epoch 1'")
# what a kept is here, with its history
assert api(b, "GET", "/repos/tester/kept/contents/hello.txt") == 200
b.succeed(f"test -d {dir}/repositories/tester/kept.git")
# what a deleted did not come back, in the records or on disk
assert api(b, "GET", "/repos/tester/gone", ok=False) == 404
b.fail(f"test -e {dir}/repositories/tester/gone.git")
# b's own earlier files: set aside, never deleted
b.succeed(f"cat {dir}/.before-e2.*/b-own-marker | grep -q b-was-here")
# its hooks made again: a commit on b goes through them
b.wait_until_succeeds(
    f"test -x {dir}/repositories/tester/kept.git/hooks/pre-receive", timeout=120
)
api(
    b,
    "POST",
    "/repos/tester/kept/contents/again.txt",
    {"content": "aGVsbG8gZnJvbSBiCg==", "message": "on b"},
)
# and b's forge copies under epoch 2 now
# (a copy that finds the store not ready yet is "next time": ask again)
b.wait_until_succeeds(
    "systemctl start dd-forge-copy.service && "
    "set -a; . /var/lib/dd-store/keys/forge.env; export AWS_DEFAULT_REGION=us-east-1; "
    'test "$(aws --endpoint-url http://127.0.0.1:3910 s3 cp s3://forge/epoch -)" = 2',
    timeout=180,
)
