# A security.txt as RFC 9116 has it: the fields we promise, and an Expires
# that is a date. Whether it is near is CI's to say, at run time (entry_point.yml).
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
datetime.datetime.strptime(fields["Expires"], "%Y-%m-%dT%H:%M:%SZ")
print(f"security.txt for {domain}: expires {fields['Expires']}")
