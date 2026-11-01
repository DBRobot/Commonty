import json

from waits import start_fresh, wait_socket, wait_unit_active

start_all()
s3 = "AWS_DEFAULT_REGION=us-east-1 aws"
for m in (a, b):
    wait_unit_active(m, "nginx.service")
    m.wait_until_succeeds("test -s /var/lib/dd-store/keys/ours-a.env", timeout=120)
    m.succeed("mkdir -p /run/standin")
    m.succeed(
        "systemd-run --unit=standin python3 -m http.server --directory /run/standin 4195"
    )
    wait_socket(m, 4195)


def word(m, plan_boxes=None, copies=None, leases=None, age=0):
    """what the stand-in says: the plan, and an agreed state `age` seconds old"""
    now = int(m.succeed("date +%s").strip())
    st = {"time": (now - age) * 1000, "boxes": {}, "leases": leases or {}}
    plan = {"boxes": plan_boxes or {}, "copies": copies or {}}
    for name, body in (("state", st), ("plan", plan)):
        m.succeed(
            f"cat > /run/standin/{name}.tmp <<'EOF'\n{json.dumps(body)}\nEOF\n"
            f"mv /run/standin/{name}.tmp /run/standin/{name}"
        )


def own(m, bucket, cmd):
    return m.execute(
        f"set -a; . /var/lib/dd-store/keys/{bucket}.env; set +a; "
        f"{s3} --endpoint-url http://127.0.0.1:3910 s3 {cmd}"
    )


def put(m, bucket, name):
    status, out = own(m, bucket, f"cp /etc/hostname s3://{bucket}/{name}")
    assert status == 0, out


def there(m, bucket, name):
    return own(m, bucket, f"ls s3://{bucket}/{name}")[0] == 0


# b, gone bad, with the key a's store handed it, at a's door
b.succeed(
    "curl -sf http://store.a.test.invalid:3443/_keys/b.age -o /tmp/k"
    " && age -d -i /etc/age-identity /tmp/k > /tmp/key-at-a"
)


def bad_delete(bucket, name):
    return b.execute(
        f"set -a; . /tmp/key-at-a; set +a; {s3} --endpoint-url "
        f"http://store.a.test.invalid:3443 s3 rm s3://{bucket}/{name}"
    )[0]


# the plan: dd-mail is a's; each box's bucket kept on both
both = {"ours-a": ["a", "b"], "ours-b": ["a", "b"], "mailbox": ["a", "b"]}
mail_on_a = {"a": [{"service": "dd-mail"}], "b": []}
word(a, mail_on_a, both)
word(b, mail_on_a, both)
for name in ("ours-a", "ours-b", "mailbox"):
    put(a, name, "kept")
start_fresh(a, "dd-store-grants.service")
start_fresh(b, "dd-store-grants.service")

# a bad box deletes nothing at another store but its own
assert bad_delete("ours-a", "kept") != 0, "b deleted a's own backup at a's store"
assert bad_delete("mailbox", "kept") != 0, "b deleted a copy of a service a runs"
assert there(a, "ours-a", "kept") and there(a, "mailbox", "kept")
assert bad_delete("ours-b", "kept") == 0, "its own bucket is its to write"
put(a, "ours-b", "kept")

# a stale state moves no grant either way
word(a, {"a": [], "b": [{"service": "dd-mail"}]}, both, age=3600)
start_fresh(a, "dd-store-grants.service")
assert bad_delete("mailbox", "kept") != 0, "a stale state gave b the write"

# the agreed lease gives it, with the service mid-move and not in the plan yet
word(a, mail_on_a, both, leases={"svc/dd-mail": {"holder": "b"}})
start_fresh(a, "dd-store-grants.service")
assert bad_delete("mailbox", "kept") == 0
put(a, "mailbox", "kept")
# and the plan's turning back takes it away
word(a, mail_on_a, both)
start_fresh(a, "dd-store-grants.service")
assert bad_delete("mailbox", "kept") != 0

# a's prune reaches its copy at b: a's grants at b say a writes its own
start_fresh(b, "dd-store-grants.service")
put(a, "ours-a", "pruned")
put(a, "ours-a", "stays")
start_fresh(a, "dd-store-copy.service")


def on_b(name):
    return (
        b.execute(
            "set -a; . /var/lib/dd-store/keys/ours-a.env; set +a; "
            f"{s3} --endpoint-url http://127.0.0.1:3910 s3 ls s3://ours-a/{name}"
        )[0]
        == 0
    )


assert on_b("stays") and on_b("pruned")
own(a, "ours-a", "rm s3://ours-a/pruned")
# not on the plan's say: it stays
word(a, mail_on_a, {"ours-a": ["a"]})
start_fresh(a, "dd-store-copy.service")
assert on_b("pruned"), "deleted without the plan keeping it there"
# not from an old agreed state
word(a, mail_on_a, both, age=3600)
start_fresh(a, "dd-store-copy.service")
assert on_b("pruned"), "deleted on an old agreed state"
# on both: gone there, what stays stays
word(a, mail_on_a, both)
start_fresh(a, "dd-store-copy.service")
assert not on_b("pruned"), "the prune did not reach the copy"
assert on_b("stays")

# a store come back empty deletes nothing
own(a, "ours-a", "rm --recursive s3://ours-a/")
start_fresh(a, "dd-store-copy.service")
assert on_b("stays"), "an emptied bucket emptied its copy"
