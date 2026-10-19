# The forge-follow test: git moves from a to b by copies in each other's
# stores, and nothing deleted comes back.
import json
import time

from waits import journal_has, journal_of, start_fresh

start_all()
for m in (a, b):
    m.wait_for_unit("dd-store-setup.service")
    m.wait_for_unit("zfs-datasets.service")
    m.wait_for_unit("nginx.service")
    # the plan's copies, as dd-control writes them
    m.succeed(
        """echo '{"forge":["a","b"],"postgres-wal":["a","b"]}' > /run/dd-placed/copies.json"""
    )

sock, dir, forge_units = nix["sock"], nix["dir"], nix["forgeUnits"]


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


def forge_copy(m):
    """the forge's copy into the store, a run of its own, which copied"""
    inv = start_fresh(m, "dd-forge-copy.service")
    assert not journal_has(m, "dd-forge-copy.service", inv, "not copied this time"), (
        journal_of(m, "dd-forge-copy.service", inv)
    )


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
# a run of its own, begun after they were made: a timer's under way began
# before, and does not list them
forge_copy(a)
assert "gone.git" in a.succeed("cat /var/lib/dd-forge/manifest")
# a repository deleted on a: gone from the next copy's list
api(a, "DELETE", "/repos/tester/gone")
forge_copy(a)
manifest = a.succeed("cat /var/lib/dd-forge/manifest")
assert "kept.git" in manifest and "gone.git" not in manifest, manifest

# everything so far copied to b, while a holds the flags: the store copies
# only for the box that writes
copy_out(a)
# from here only the handover copies anything: no timer does it behind it
a.succeed(
    "systemctl stop dd-store-copy.timer dd-store-copy-recent.timer dd-forge-copy.timer"
)
# made the moment before the handover: in no copy yet
api(
    a,
    "POST",
    "/user/repos",
    {"name": "late", "default_branch": "main", "auto_init": True},
)
api(
    a,
    "POST",
    "/repos/tester/kept/contents/late.txt",
    {"content": "bGF0ZQo=", "message": "late on a"},
)

# a hands over: postgres archives its last segment on the way down, the
# forge's files go to its store, and both out to b
hand_over(a, "forgejo", forge_units, "forgejo.service")
hand_over(a, "postgresql", "postgresql.service", "postgresql.service")

# b takes epoch 2 from its own store
hold(b, 2)
up(b)
b.succeed("journalctl -u forgejo.service | grep -q 'restoring the copy of epoch 1'")
# what a kept is here, with its history
assert api(b, "GET", "/repos/tester/kept/contents/hello.txt") == 200
b.succeed(f"test -d {dir}/repositories/tester/kept.git")
# and what it made the moment before it handed over
# (missing, say what each side did: the tail of a CI log is all there is)
late = api(b, "GET", "/repos/tester/late", ok=False)
assert late == 200, (
    late,
    a.succeed(
        "journalctl -o cat -u postgresql -u dd-pg-handover -u dd-forge-handover "
        "-u dd-store-copy-final@forgejo -u dd-store-copy-final@postgresql | tail -n 40"
    ),
    b.succeed("journalctl -o cat -u postgresql | tail -n 30"),
)
b.succeed(f"test -d {dir}/repositories/tester/late.git")
assert api(b, "GET", "/repos/tester/kept/contents/late.txt") == 200
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
