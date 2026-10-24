import time

from waits import start_fresh

# The store test: a box's store is its own, and its copies are elsewhere.
start_all()
for m in (a, b):
    m.wait_for_unit("dd-store-setup.service")
    m.wait_for_unit("nginx.service")
    # every copy is one the test starts: a timer's run (a's recent copy every
    # 30 s, b's photos back to a) undid the drop, plan and full mark below
    m.wait_for_unit("timers.target")
    m.succeed(
        "systemctl stop dd-store-copy.timer dd-store-copy-recent.timer "
        "dd-store-copy.service dd-store-copy-recent.service"
    )

# each made its own secret, and they differ
sa = a.succeed("cat /var/lib/dd-store/rpc-secret").strip()
sb = b.succeed("cat /var/lib/dd-store/rpc-secret").strip()
assert len(sa) == 64 and sa != sb
# and the store listens for this box alone
a.fail("curl -sf --max-time 5 http://b:3910/")

s3 = "aws --endpoint-url http://127.0.0.1:3910"


def put(m, bucket, name, body):
    m.succeed(
        f"set -a; . /var/lib/dd-store/keys/{bucket}.env; echo '{body}' > /tmp/x; "
        f"AWS_DEFAULT_REGION=us-east-1 {s3} s3 cp /tmp/x s3://{bucket}/{name}"
    )


def copied(bucket, name, body):
    """a copies until b has it: a copy that finds b not ready is next time"""
    cmd = (
        f"set -a; . /var/lib/dd-store/keys/{bucket}.env; "
        f"AWS_DEFAULT_REGION=us-east-1 {s3} s3 cp s3://{bucket}/{name} -"
    )
    for _ in range(40):
        start_fresh(a, "dd-store-copy.service")
        status, out = b.execute(cmd)
        if status == 0 and out.strip() == body:
            return
        time.sleep(3)
    raise AssertionError(f"{bucket}/{name} never reached b")


def logged(run, text):
    # that run's lines, once journald has them all
    a.wait_until_succeeds(
        f"journalctl -o cat _SYSTEMD_INVOCATION_ID={run} | grep -cF '{text}'",
        timeout=30,
    )


def get(m, bucket, name):
    return m.succeed(
        f"set -a; . /var/lib/dd-store/keys/{bucket}.env; "
        f"AWS_DEFAULT_REGION=us-east-1 {s3} s3 cp s3://{bucket}/{name} -"
    ).strip()


put(a, "photos", "p1", "a photo, ciphertext")
put(a, "scratch", "s1", "kept on a alone")
# b's key for a is at b's store, sealed: a opens it, b's own copy of it
# (not the sealed one) is root's alone
a.succeed(
    "curl -sf http://store.b.test.invalid:3443/_keys/a.age -o /tmp/k && age -d -i /etc/age-identity /tmp/k | grep -c AWS_ACCESS_KEY_ID"
)
b.fail(
    "curl -sf http://store.b.test.invalid:3443/_keys/a.age -o /tmp/k && age -d -i /etc/age-identity /tmp/k"
)
assert b.succeed("stat -c %a /var/lib/dd-store/from").strip() == "700"

# the copy: photos goes to b, scratch does not
copied("photos", "p1", "a photo, ciphertext")
b.fail(
    "set -a; . /var/lib/dd-store/keys/scratch.env; AWS_DEFAULT_REGION=us-east-1 "
    f"{s3} s3 ls s3://scratch/s1"
)
# a's key at b writes what a copies and nothing else
a.fail(
    "set -a; . /var/lib/dd-store-copy/b.env; AWS_DEFAULT_REGION=us-east-1 "
    "aws --endpoint-url http://store.b.test.invalid:3443 s3 ls s3://scratch/"
)
# and through b's door, its signature holds: the Host it signed, port and
# all, is the one b's store checks (a dropped port was "Invalid signature")
a.succeed(
    "set -a; . /var/lib/dd-store-copy/b.env; AWS_DEFAULT_REGION=us-east-1 "
    "aws --endpoint-url http://store.b.test.invalid:3443 s3 ls s3://photos/ > /tmp/ls && grep -c p1 /tmp/ls"
)
# nothing is deleted there: gone here, still there after the next copy
a.succeed(
    "set -a; . /var/lib/dd-store/keys/photos.env; AWS_DEFAULT_REGION=us-east-1 "
    f"{s3} s3 rm s3://photos/p1"
)
a.succeed("systemctl start dd-store-copy.service")
assert get(b, "photos", "p1") == "a photo, ciphertext"

# the store says its room, and what it keeps of each bucket
a.succeed("systemctl start dd-store-facts.service")
facts = a.succeed("cat /var/lib/dd-facts/store.prom")
assert "dd_store_size_bytes 1073741824" in facts, facts
assert 'dd_store_bucket_bytes{bucket="photos"}' in facts, facts
a.succeed("test ! -e /run/dd-store/full")

# the plan decides where copies go: photos kept on a alone, then on a and b
a.succeed("""echo '{"photos":["a"]}' > /run/dd-placed/copies.json""")
put(a, "photos", "p2", "the plan keeps it here")
a.succeed("systemctl start dd-store-copy.service")
b.fail(
    "set -a; . /var/lib/dd-store/keys/photos.env; AWS_DEFAULT_REGION=us-east-1 "
    f"{s3} s3 ls s3://photos/p2"
)
a.succeed("""echo '{"photos":["a","b"]}' > /run/dd-placed/copies.json""")
# the full and the recent copy side by side, as their timers run them: both
# read the plan into the same state directory
a.succeed("systemctl start dd-store-copy.service dd-store-copy-recent.service")
copied("photos", "p2", "the plan keeps it here")

# b nearly full: what a sends is refused, what b keeps still read
# the timer stopped, and a run of it already going: one ending after the
# touch would see the room and take the mark away
b.succeed(
    "systemctl stop dd-store-facts.timer dd-store-facts.service; touch /run/dd-store/full"
)
put(a, "photos", "p3", "no room for it there")
run = start_fresh(a, "dd-store-copy.service")
b.fail(
    "set -a; . /var/lib/dd-store/keys/photos.env; AWS_DEFAULT_REGION=us-east-1 "
    f"{s3} s3 ls s3://photos/p3"
)
logged(run, "b: photos not copied this time")
assert get(b, "photos", "p2") == "the plan keeps it here"
# room again: it goes
b.succeed("rm /run/dd-store/full")
copied("photos", "p3", "no room for it there")

# b drops what it was sent: the read back finds it out, and the plan hears
b.succeed(
    "set -a; . /var/lib/dd-store/keys/photos.env; export AWS_DEFAULT_REGION=us-east-1; "
    f"for o in p1 p2 p3; do {s3} s3 rm s3://photos/$o; done"
)
logged(start_fresh(a, "dd-store-copy.service"), "read back is not what was sent")
a.succeed(
    """grep -q 'dd_store_lost{bucket="photos",box="b"} 1' /var/lib/dd-facts/store-copy.prom"""
)
# sent again, read back whole: no longer lost
copied("photos", "p3", "no room for it there")
a.succeed("! grep -q dd_store_lost /var/lib/dd-facts/store-copy.prom")

# a recent copy takes each object's age from b's listing, and asks b about
# no object one at a time: node2's asked node1 15000 times a run (2026-10-24)
b.succeed("test -s /var/log/nginx/access.log")


def heads():
    return int(
        b.succeed("grep -c '\"HEAD /photos/' /var/log/nginx/access.log || true").strip()
    )


asked = heads()
start_fresh(a, "dd-store-copy-recent.service")
assert heads() == asked, f"a recent copy sent b {heads() - asked} HEADs"

# a bucket a box writes alone: a's copy goes to b, and what b holds of it
# never comes back, so what a deletes stays deleted
put(a, "ours", "o1", "written by a alone")
a.succeed(
    """echo '{"photos":["a","b"],"ours":["a","b"]}' > /run/dd-placed/copies.json"""
)
copied("ours", "o1", "written by a alone")
put(b, "ours", "o2", "written by b only")
b.succeed("systemctl start dd-store-copy.service")
a.fail(
    "set -a; . /var/lib/dd-store/keys/ours.env; AWS_DEFAULT_REGION=us-east-1 "
    f"{s3} s3 ls s3://ours/o2"
)

# a shared bucket, kept in a's store: b writes it there with its key to a's
# store, which dd-store-at hands it; at home, a's own key
b.wait_until_succeeds(
    "systemctl restart dd-store-at.service && test -s /var/lib/dd-store/at/blocks.env",
    timeout=120,
)
a.succeed(
    "systemctl restart dd-store-at.service && test -s /var/lib/dd-store/at/blocks.env"
)
b.succeed(
    "set -a; . /var/lib/dd-store/at/blocks.env; echo 'a block of b' > /tmp/x; "
    "AWS_DEFAULT_REGION=us-east-1 aws --endpoint-url http://store.a.test.invalid:3443 "
    "s3 cp /tmp/x s3://blocks/b1"
)
assert get(a, "blocks", "b1") == "a block of b"
assert b.succeed("stat -c %a /var/lib/dd-store/at/blocks.env").strip() == "400"

# a is gone; b serves what a wrote
a.crash()
assert get(b, "photos", "p3") == "no room for it there"
