#!/usr/bin/env python3
"""Each dedicated server's installed size, from Steam's own depot list (the
public branch's manifests, via api.steamcmd.net): the linux depots, or the
windows ones for a server that runs under wine. Written into the catalogue
as disk_bytes; nix/modules/games/needs.nix sizes a world's disk from it.
Run after resolve.py; the result is committed.

  data/games/sizes.py data/games/catalogue.json
"""

import json
import sys
import time
import urllib.request

out = sys.argv[1]
cat = json.load(open(out))
for g in cat["games"]:
    want = "windows" if g.get("wine") else "linux"
    try:
        with urllib.request.urlopen(
            "https://api.steamcmd.net/v1/info/%d" % g["app"], timeout=30
        ) as r:
            info = json.load(r)["data"][str(g["app"])]
    except Exception as e:
        print(g["id"], "no answer:", e, file=sys.stderr)
        continue
    total = 0
    for depot in (info.get("depots") or {}).values():
        if not isinstance(depot, dict) or "manifests" not in depot:
            continue
        oslist = (depot.get("config") or {}).get("oslist")
        if oslist and want not in oslist.split(","):
            continue
        size = (depot["manifests"].get("public") or {}).get("size")
        if size:
            total += int(size)
    if total:
        g["disk_bytes"] = total
    time.sleep(1)
with open(out, "w") as f:
    json.dump(cat, f, indent=1)
    f.write("\n")
known = [g for g in cat["games"] if "disk_bytes" in g]
print(len(known), "of", len(cat["games"]), "sized")
