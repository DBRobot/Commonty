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

# a is gone; b serves what a wrote
a.crash()
assert get(b, "photos", "p1") == "a photo, ciphertext"
