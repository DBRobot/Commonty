#!/usr/bin/env python3
# The files each pull request run changed (merge-base of its base and its
# commit, to its commit), each named by its path in today's tree: main's
# renames (git's -M) followed forward, so a run from before the repo moved
# things (PR 62, PR 269) is judged against today's paths.
#
#   changes.py DIR > changes.json   (run from the repository)
import csv
import glob
import json
import os
import subprocess
import sys


def git(*a):
    return subprocess.run(
        ["git", *a], capture_output=True, text=True, errors="replace"
    ).stdout


def renames(tip="forge/main"):
    """old path -> today's path, by every rename on main's first parents;
    a file deleted where one of the same name was added in the same commit
    (a move that changed it too much for git to call it one) counts too"""
    to = {}
    log = git(
        "log",
        "--reverse",
        "--first-parent",
        "-M40%",
        "--diff-filter=RAD",
        "--name-status",
        "--format=format:@",
        "--diff-merges=first-parent",
        tip,
    )
    for commit in log.split("@"):
        added, deleted = {}, []
        for line in commit.split("\n"):
            p = line.split("\t")
            if line.startswith("R"):
                to[p[1]] = p[2]
            elif line.startswith("A"):
                added.setdefault(os.path.basename(p[1]), []).append(p[1])
            elif line.startswith("D"):
                deleted.append(p[1])
        for a in deleted:
            b = added.get(os.path.basename(a), [])
            if len(b) == 1:
                to[a] = b[0]
    out = {}
    for a in to:
        b, seen = a, set()
        while b in to and b not in seen:
            seen.add(b)
            b = to[b]
        out[a] = b
    return out


def main():
    d = sys.argv[1]
    rows = [
        r
        for f in sorted(glob.glob(os.path.join(d, "history-jobs-*.csv")))
        for r in csv.DictReader(open(f))
    ]
    runs = {r["id"]: r for r in csv.DictReader(open(os.path.join(d, "runs.csv")))}
    base = {
        b["id"]: b["base"] for b in csv.DictReader(open(os.path.join(d, "base.csv")))
    }
    want = sorted(
        {r["run"] for r in rows if r["event"].startswith("pull_request")}, key=int
    )
    ren = renames()
    out = {}
    for run in want:
        sha, b = runs[run]["commit_sha"], base.get(run)
        if not b:
            continue
        mb = git("merge-base", b, sha).strip()
        if not mb:
            continue
        files = [f for f in git("diff", "--name-only", mb, sha).split("\n") if f]
        out[run] = sorted({ren.get(f, f) for f in files})
    json.dump({"renames": len(ren), "rename_map": ren, "runs": out}, sys.stdout)
    print()


if __name__ == "__main__":
    main()
