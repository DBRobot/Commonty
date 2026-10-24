#!/usr/bin/env python3
# Every real failure in the history, job by job: a failure history.py did
# not put down to a flake, the box, or a cancellation, and that a code change
# then fixed (the same pull request's job passed later on another commit; on
# main, the next main commit it passed on). For each, the files the breaking
# change touched and the files the fix touched: each job's empirical "can
# break me" set.
#
#   realfail.py DIR > realfail.json   (run from the repository; DIR as history.py's)
import collections
import csv
import glob
import json
import os
import re
import subprocess
import sys

KEYS = ("kind", "arg")


def git(*a):
    return subprocess.run(
        ["git", *a], capture_output=True, text=True, errors="replace"
    ).stdout


def full(sha, cache={}):
    if sha not in cache:
        cache[sha] = git("rev-parse", "--verify", "-q", sha + "^{commit}").strip()
    return cache[sha]


def names(a, b):
    if not a or not b:
        return None
    return sorted(f for f in git("diff", "--name-only", a, b).split("\n") if f)


def pr_files(base, head):
    mb = git("merge-base", base, head).strip() if base else ""
    return names(mb, head), mb


def patch_id(a, b):
    p = subprocess.run(["git", "diff", a, b], capture_output=True).stdout
    return subprocess.run(
        ["git", "patch-id", "--stable"], input=p, capture_output=True
    ).stdout.split(b" ")[0]


def job_of(r):
    k, a = r["kind"], r["arg"]
    if k == "systems" and "," not in a and a not in ("",):
        a = f"{a},?"
    if k == "browser" and not a:
        a = "mail-worker"  # the one Worker job of the first weeks
    return f"{k}({a})" if a else k


def lineages(d):
    """pull requests that carry the same change on: the same title, or a
    branch of the same name but its date and retry number (a change closed
    and opened again, or retried as -2, -3)"""
    prs = list(csv.DictReader(open(os.path.join(d, "prs.csv"))))
    up = {p["id"]: p["id"] for p in prs}

    def root(x):
        while up[x] != x:
            up[x] = up[up[x]]
            x = up[x]
        return x

    by = collections.defaultdict(list)
    for p in prs:
        t = re.sub(r"^(release:\s*|wip:\s*)+", "", p["translate"].strip().lower())
        by["t:" + t].append(p["id"])
        stem = re.sub(r"\d*(-\d{4})?$", "", p["head_branch"].lower())
        if len(stem) >= 5:
            by["b:" + stem].append(p["id"])
    for ids in by.values():
        for x in ids[1:]:
            up[root(x)] = root(ids[0])
    return {x: root(x) for x in up}


def load(d):
    rows = [
        r
        for f in sorted(glob.glob(os.path.join(d, "history-jobs-*.csv")))
        for r in csv.DictReader(open(f))
    ]
    runs = {r["id"]: r for r in csv.DictReader(open(os.path.join(d, "runs.csv")))}
    return rows, runs


def main():
    d = sys.argv[1]
    rows, runs = load(d)
    for r in rows:
        r["job"] = job_of(r)
        r["created"] = int(runs[r["run"]]["created"])
        r["ref"] = runs[r["run"]]["ref"]
    # every run of a job, in order, per pull request (or main)
    line = lineages(d)
    lane = collections.defaultdict(list)
    for r in rows:
        if r["event"].startswith("pull_request"):
            lane[("pr", line.get(r["pr"], r["pr"]), r["job"])].append(r)
        elif r["ref"] == "refs/heads/main":
            lane[("main", "", r["job"])].append(r)
    out, never, unconfirmed = [], [], collections.Counter()
    lane_all = collections.defaultdict(list)
    for r in rows:
        if r["event"].startswith("pull_request"):
            lane_all[line.get(r["pr"], r["pr"])].append(r)
    merged = {
        p["id"]
        for p in csv.DictReader(open(os.path.join(d, "prs.csv")))
        if p["has_merged"] == "t"
    }
    seen = set()
    for key, rs in lane.items():
        rs.sort(key=lambda r: (r["created"], int(r["task"])))
        for i, r in enumerate(rs):
            if r["status"] != "failure" or r["class"] != "real":
                continue
            if r["kind"] in ("style", "lint", "ci_ok", "files_modified", "agree"):
                # findings about the change's text, or a summary of others
                continue
            fix = next(
                (
                    s
                    for s in rs[i + 1 :]
                    if s["status"] == "success" and s["sha"] != r["sha"]
                ),
                None,
            )
            same_passed = any(
                s["status"] == "success" and s["sha"] == r["sha"] for s in rs
            )
            if same_passed:
                unconfirmed["passed on the same commit"] += 1
                continue
            if not fix:
                unconfirmed[f"never passed later ({key[0]})"] += 1
                later = [s for s in lane_all[key[1]] if s["created"] > r["created"]]
                never.append(
                    {
                        "job": r["job"],
                        "pr": int(r["pr"]),
                        "run_no": int(r["run_no"]),
                        "sha": r["sha"],
                        "why": r["why"],
                        "later_runs": len({s["run"] for s in later}),
                        "later_job_runs": sum(1 for s in later if s["job"] == r["job"]),
                        "merged": any(
                            p in merged for p, l in line.items() if l == key[1]
                        ),
                    }
                )
                continue
            fs, ff = full(r["sha"]), full(fix["sha"])
            if not fs or not ff:
                unconfirmed["commit not in git"] += 1
                continue
            if key[0] == "pr":
                bf, bp = full(r["base"]), full(fix["base"])
                files, mb = pr_files(bf, fs)
                files_p, mbp = pr_files(bp, ff)
                rebase_only = (
                    mb and mbp and mb != mbp and patch_id(mb, fs) == patch_id(mbp, ff)
                )
            else:
                # main: from the last main commit this job passed on
                prev = next(
                    (
                        s
                        for s in reversed(rs[:i])
                        if s["status"] == "success" and s["sha"] != r["sha"]
                    ),
                    None,
                )
                mb = full(prev["sha"]) if prev else ""
                files = names(mb, fs) if mb else None
                files_p, rebase_only, mbp = None, False, mb
            fixed = names(fs, ff) or []
            if not fixed:
                # the fix was a commit that changed nothing: a rerun
                unconfirmed["fixed by a commit that changed nothing (a rerun)"] += 1
                continue
            mine = set(files or []) | set(files_p or [])
            inc = (key[0], key[1], r["job"], fix["sha"])
            out.append(
                {
                    "job": r["job"],
                    "kind": r["kind"],
                    "lane": key[0],
                    "pr": int(r["pr"]),
                    "run": int(r["run"]),
                    "run_no": int(r["run_no"]),
                    "task": int(r["task"]),
                    "when": r["created"],
                    "sha": r["sha"],
                    "fixed_by": fix["sha"],
                    "fixed_run_no": int(fix["run_no"]),
                    "why": r["why"],
                    "minutes": float(r["min"]),
                    # the same break seen again before the fix: counted once
                    "repeat": inc in seen,
                    "fixed_by_rebase": bool(rebase_only),
                    "changed": files,
                    # what main brought in between the failing run's base and
                    # the passing run's: a fix that came from main shows here
                    "main_moved": names(mb, mbp) if mb and mbp and mb != mbp else [],
                    "fix_changed": sorted(f for f in fixed if f in mine)
                    if mine
                    else fixed[:200],
                }
            )
            seen.add(inc)
    json.dump(
        {
            "real_failures": len(out),
            "incidents": sum(1 for f in out if not f["repeat"]),
            "fixed_by_rebase": sum(1 for f in out if f["fixed_by_rebase"]),
            "unconfirmed": unconfirmed,
            "failures": out,
            "never_passed_later": never,
        },
        sys.stdout,
        indent=1,
    )
    print()


if __name__ == "__main__":
    main()
