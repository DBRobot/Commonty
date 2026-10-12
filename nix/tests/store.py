# The store test: a box's store is its own, and its copies are elsewhere.
start_all()
for m in (a, b):
    m.wait_for_unit("dd-store-setup.service")
    m.wait_for_unit("nginx.service")

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
    "curl -sf http://store.b.test.invalid/_keys/a.age -o /tmp/k && age -d -i /etc/age-identity /tmp/k | grep -q AWS_ACCESS_KEY_ID"
)
b.fail(
    "curl -sf http://store.b.test.invalid/_keys/a.age -o /tmp/k && age -d -i /etc/age-identity /tmp/k"
)
assert b.succeed("stat -c %a /var/lib/dd-store/from").strip() == "700"

# the copy: photos goes to b, scratch does not
a.succeed("systemctl start dd-store-copy.service")
assert get(b, "photos", "p1") == "a photo, ciphertext"
b.fail(
    "set -a; . /var/lib/dd-store/keys/scratch.env; AWS_DEFAULT_REGION=us-east-1 "
    f"{s3} s3 ls s3://scratch/s1"
)
# a's key at b writes what a copies and nothing else
a.fail(
    "set -a; . /var/lib/dd-store-copy/b.env; AWS_DEFAULT_REGION=us-east-1 "
    "aws --endpoint-url http://store.b.test.invalid s3 ls s3://scratch/"
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
a.succeed("systemctl start dd-store-copy.service")
assert get(b, "photos", "p2") == "the plan keeps it here"

# b nearly full: what a sends is refused, what b keeps still read
b.succeed("systemctl stop dd-store-facts.timer; touch /run/dd-store/full")
put(a, "photos", "p3", "no room for it there")
a.succeed("systemctl start dd-store-copy.service")
b.fail(
    "set -a; . /var/lib/dd-store/keys/photos.env; AWS_DEFAULT_REGION=us-east-1 "
    f"{s3} s3 ls s3://photos/p3"
)
a.succeed(
    "journalctl -u dd-store-copy.service | grep -q 'b: photos not copied this time'"
)
assert get(b, "photos", "p2") == "the plan keeps it here"
# room again: it goes
b.succeed("rm /run/dd-store/full")
a.succeed("systemctl start dd-store-copy.service")
assert get(b, "photos", "p3") == "no room for it there"

# b drops what it was sent: the read back finds it out, and the plan hears
b.succeed(
    "set -a; . /var/lib/dd-store/keys/photos.env; AWS_DEFAULT_REGION=us-east-1 "
    f"for o in p1 p2 p3; do {s3} s3 rm s3://photos/$o; done"
)
a.succeed("systemctl start dd-store-copy.service")
a.succeed(
    "journalctl -u dd-store-copy.service | grep -q 'read back is not what was sent'"
)
a.succeed(
    """grep -q 'dd_store_lost{bucket="photos",box="b"} 1' /var/lib/dd-facts/store-copy.prom"""
)
# sent again, read back whole: no longer lost
a.succeed("systemctl start dd-store-copy.service")
a.succeed("! grep -q dd_store_lost /var/lib/dd-facts/store-copy.prom")

# a is gone; b serves what a wrote
a.crash()
assert get(b, "photos", "p1") == "a photo, ciphertext"
