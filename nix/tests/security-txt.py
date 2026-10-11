# A security.txt as RFC 9116 has it: the fields we promise, and an Expires
# that has not passed and is under a year and a day away.
import datetime
import sys

path, domain = sys.argv[1:3]
fields = {}
for line in open(path).read().splitlines():
    k, _, v = line.partition(": ")
    fields[k] = v
want = {
    "Contact": f"mailto:security@{domain}",
    "Preferred-Languages": "en",
    "Canonical": f"https://{domain}/.well-known/security.txt",
    "Policy": f"https://{domain}/docs/policies/privacy",
}
for k, v in want.items():
    assert fields.get(k) == v, f"{k}: {fields.get(k)!r}, wanted {v!r}"
expires = datetime.datetime.strptime(fields["Expires"], "%Y-%m-%dT%H:%M:%SZ").replace(tzinfo=datetime.timezone.utc)
now = datetime.datetime.now(datetime.timezone.utc)
assert now < expires <= now + datetime.timedelta(days=366), f"Expires {expires} is not within a year of {now}"
print(f"security.txt for {domain}: expires {fields['Expires']}")
