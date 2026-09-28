# The curl config for every range not already fetched into the directory
# given: two lines each, a million in all on a first run.
import os
import sys

d = sys.argv[1]
have = {f for f in os.listdir(d) if os.path.getsize(os.path.join(d, f)) > 0}
out = sys.stdout
for i in range(1 << 20):
    p = "%05X" % i
    if p not in have:
        out.write(f'url = "https://api.pwnedpasswords.com/range/{p}"\noutput = "{p}"\n')
