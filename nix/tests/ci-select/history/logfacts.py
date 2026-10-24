#!/usr/bin/env python3
# What each CI job's log says, read once from the forge's job logs (a copy
# of /vault/forgejo/data/actions_log): the derivations it built (none: the
# answer was already in that box's store), the check's own derivation when
# it was built, and why a failed job failed.
#
#   logfacts.py LOGDIR tasks.csv > logfacts.json
import csv
import json
import os
import re
import subprocess
import sys
from concurrent.futures import ProcessPoolExecutor

BUILDING = re.compile(r"building '/nix/store/([a-z0-9]{32})-([^']+)\.drv'")
WILL = re.compile(
    r"these (\d+) derivations will be built|this derivation will be built"
)
# why a job failed, the first that matches, in this order
WHY = [
    (
        "killed",
        re.compile(r"context canceled|interrupted by the user|signal: terminated"),
    ),
    (
        "oom",
        re.compile(
            r"[Oo]ut of memory|oom-kill|OOM|Killed process|\bKilled\s+nix |qemu.*Killed|exit code 137"
        ),
    ),
    ("disk", re.compile(r"[Nn]o space left on device")),
    (
        "network",
        re.compile(
            r"Could not resolve host|unable to download|Connection (refused|reset|timed out)|HTTP error 5\d\d|cannot connect to"
        ),
    ),
    ("timeout", re.compile(r"action timed out|timed out after|Timeout|timeout")),
    (
        "test",
        re.compile(
            r"Traceback|AssertionError|Test \S+ failed|!!! |test result: FAILED|FAIL "
        ),
    ),
    (
        "build",
        re.compile(
            r"error: builder for|error: Cannot build|error\[E\d+\]|error: could not compile"
        ),
    ),
    (
        "eval",
        re.compile(
            r"error: (attribute|undefined|infinite|cannot coerce|value is)|evaluation (aborted|error)"
        ),
    ),
]


def one(row):
    path = os.path.join(sys.argv[1], row["log_filename"])
    if not row["log_filename"] or not os.path.exists(path):
        return row["id"], None
    text = subprocess.run(
        ["zstdcat", path], capture_output=True, text=True, errors="replace"
    ).stdout
    built = [f"{h}-{n}" for h, n in BUILDING.findall(text)]
    out = {
        "built": len(built),
        "drvs": [
            d
            for d in built
            if re.search(
                r"vm-test-run-|-e2e|site-|worker|page|ci-select|tests|clippy|installer|nixos-system",
                d,
            )
        ][-6:],
    }
    if row["status"] == "2":
        # every sign found, in WHY's order: the reader decides (a style or
        # lint job's failure is a finding whatever its log says)
        out["why"] = [name for name, rx in WHY if rx.search(text)] or ["other"]
    return row["id"], out


def main():
    rows = list(csv.DictReader(open(sys.argv[2])))
    with ProcessPoolExecutor(8) as ex:
        facts = dict(ex.map(one, rows, chunksize=64))
    json.dump({k: v for k, v in facts.items() if v is not None}, sys.stdout)


if __name__ == "__main__":
    main()
