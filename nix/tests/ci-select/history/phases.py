#!/usr/bin/env python3
# Where a VM test job's minutes go, from its log's timestamps: before the
# test's own derivation starts building (checkout, evaluation, the boxes'
# systems and VM images: the part tests could share), and the test itself
# (boot and script, inside the vm-test-run build).
#
#   phases.py DIR > phases.json
import csv
import datetime
import glob
import json
import os
import re
import subprocess
import sys
from concurrent.futures import ProcessPoolExecutor

TS = re.compile(r"^(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d)")
RUN = re.compile(r"building '/nix/store/[a-z0-9]{32}-vm-test-run-")


def ts(line):
    m = TS.match(line)
    return (
        datetime.datetime.fromisoformat(m.group(1))
        .replace(tzinfo=datetime.timezone.utc)
        .timestamp()
        if m
        else None
    )


def one(a):
    task, path, start, stop = a
    if not os.path.exists(path):
        return task, None
    text = subprocess.run(
        ["zstdcat", path], capture_output=True, text=True, errors="replace"
    ).stdout.split("\n")
    first = next((ts(x) for x in text if ts(x)), None)
    run = next((ts(x) for x in text if RUN.search(x)), None)
    if run is None:
        return task, {"built_test": False}
    return task, {
        "built_test": True,
        "prefix": round((run - start) / 60, 2),
        "test": round((stop - run) / 60, 2),
        "first": round(((first or start) - start) / 60, 2),
    }


def main():
    d = sys.argv[1]
    logs = {
        t["id"]: t["log_filename"]
        for t in csv.DictReader(open(os.path.join(d, "tasks.csv")))
    }
    rows = [
        r
        for f in sorted(glob.glob(os.path.join(d, "history-jobs-*.csv")))
        for r in csv.DictReader(open(f))
        if r["kind"] == "vm" and r["arg"] and r["status"] == "success"
    ]
    args = [
        (
            r["task"],
            os.path.join(d, "L", logs[r["task"]]),
            int(r["start"]),
            int(r["stop"]),
        )
        for r in rows
        if logs.get(r["task"])
    ]
    with ProcessPoolExecutor(4) as ex:
        out = dict(ex.map(one, args, chunksize=32))
    json.dump(out, sys.stdout)
    print()


if __name__ == "__main__":
    main()
