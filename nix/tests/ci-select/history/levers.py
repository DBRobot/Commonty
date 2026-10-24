#!/usr/bin/env python3
# Each change to CI, replayed over every pull request run that finished
# (history-jobs.csv): the job-minutes it saves, the wall time of an average
# run on the runner slots, and the real failures it would have let through.
# A run's wall time is simulated: its jobs, at their measured minutes, handed
# to the boxes' slots in the workflow's order (checked against the real
# runs: within a minute on the mean).
#
#   levers.py DIR [score-covers.json score-faces.json] [--since 2026-10-15] > levers.json
import collections
import csv
import glob
import json
import math
import os
import statistics
import sys

ORDER = [
    "files_modified",
    "flake",
    "keep",
    "browser",
    "programs",
    "systems",
    "agree",
    "vm",
    "style",
    "lint",
    "rust",
    "app",
    "installer",
    "ci_ok",
]
AFTER_PROGRAMS = {"vm", "systems"}


def simulate(jobs, slots, longest=False):
    """list scheduling of [(kind, minutes, box or None)]; the run's minutes"""
    jobs = sorted(
        jobs,
        key=lambda j: (
            ORDER.index(j[0]) if j[0] in ORDER else 50,
            -j[1] if longest and j[0] == "vm" else 0,
        ),
    )
    free = {b: [0.0] * n for b, n in slots.items()}
    fm = next((j[1] for j in jobs if j[0] == "files_modified"), 0.1)
    prog = [j for j in jobs if j[0] == "programs"]
    end = fm
    prog_done = fm
    agree_after = 0.0

    def place(j, ready):
        boxes = [j[2]] if j[2] in free else list(free)
        b, i = min(
            ((b, i) for b in boxes for i in range(len(free[b]))),
            key=lambda x: max(free[x[0]][x[1]], ready),
        )
        s = max(free[b][i], ready)
        free[b][i] = s + j[1]
        return s + j[1]

    for j in prog:
        prog_done = max(prog_done, place(j, fm))
    end = max(end, prog_done)
    for j in jobs:
        if j[0] in ("files_modified", "programs", "ci_ok", "agree"):
            continue
        ready = prog_done if j[0] in AFTER_PROGRAMS else fm
        e = place(j, ready)
        end = max(end, e)
        if j[0] == "systems":
            agree_after = max(agree_after, e)
    return end + (0.05 if agree_after else 0)


def split(jobs, cap=5.0, setup=1.5):
    out = []
    for k, m, b, arg in jobs:
        if k == "vm" and m > cap and arg in ("rollout", "runner-restart"):
            n = math.ceil((m - setup) / (cap - setup))
            out += [(k, (m - setup) / n + setup, b, arg)] * n
        else:
            out.append((k, m, b, arg))
    return out


def main():
    d = sys.argv[1]
    rows = [
        r
        for f in sorted(glob.glob(os.path.join(d, "history-jobs-*.csv")))
        for r in csv.DictReader(open(f))
    ]
    import datetime

    args = sys.argv[2:]
    since = 0
    if "--since" in args:
        i = args.index("--since")
        since = datetime.datetime.fromisoformat(args[i + 1]).timestamp()
        args = args[:i] + args[i + 2 :]
    scores = {}
    for f in args:
        s = json.load(open(f))
        scores[s["chooser"]] = s["chosen"]
    runs = collections.defaultdict(list)
    for r in rows:
        runs[r["run"]].append(r)
    # finished pull request runs of the one workflow's era with every job's minutes
    done = {
        k: v
        for k, v in runs.items()
        if v[0]["event"].startswith("pull_request")
        and min(int(r["start"]) for r in v) >= since
        and not any(r["class"] in ("superseded", "stopped") for r in v)
    }
    real = lambda r: r["class"] == "real"  # noqa: E731

    def jobs_of(v, drop):
        return [
            (
                r["kind"],
                float(r["min"]),
                r["box"] if r["kind"] in ("systems", "installer") else None,
                r["arg"],
            )
            for r in v
            if not drop(r)
        ]

    def chooser_drop(name):
        ch = scores.get(name, {})

        def drop(r):
            c = ch.get(r["run"])
            return (
                bool(c)
                and "error" not in c
                and c["keep"] is not None
                and r["kind"] in ("vm", "browser")
                and r["arg"] not in c["keep"]
            )

        return drop

    determined = lambda r: r["class"] in ("noop", "determined")  # noqa: E731
    sys_pr = lambda r: r["kind"] in ("systems", "agree", "installer")  # noqa: E731
    levers = [
        ("today", lambda r: False, False, False),
        (
            "exact derivations: skip a job whose derivations all passed before",
            determined,
            False,
            False,
        ),
        ("systems, agree and installer on main's full run only", sys_pr, False, False),
        ("longest VM test first", lambda r: False, True, False),
        ("rollout and runner-restart split under 5 min", lambda r: False, False, True),
    ]
    if "covers" in scores:
        levers.append(
            (
                "today's chooser (covers.json) where runs predate it",
                chooser_drop("covers"),
                False,
                False,
            )
        )
    if "interfaces" in scores:
        levers.append(("interface chooser", chooser_drop("interfaces"), False, False))
        both = chooser_drop("interfaces")
        levers.append(
            (
                "all together",
                lambda r: determined(r) or sys_pr(r) or both(r),
                True,
                True,
            )
        )
    out = {"runs": len(done), "levers": []}
    for slots in (
        {"node1": 2, "node2": 1},
        {"node1": 2, "node2": 2},
        {"node1": 4, "node2": 1},
        {"node1": 4, "node2": 2},
    ):
        base = None
        for name, drop, longest, sp in levers:
            walls, saved, missed = [], 0.0, 0
            for v in done.values():
                js = jobs_of(v, drop)
                if sp:
                    js = split(js)
                walls.append(simulate([(k, m, b) for k, m, b, _ in js], slots, longest))
                saved += sum(float(r["min"]) for r in v if drop(r))
                missed += sum(1 for r in v if drop(r) and real(r))
            mean = statistics.mean(walls)
            base = base if base is not None else mean
            out["levers"].append(
                {
                    "slots": "+".join(f"{b}:{n}" for b, n in slots.items()),
                    "lever": name,
                    "job_minutes_saved": round(saved),
                    "mean_wall": round(mean, 1),
                    "median_wall": round(statistics.median(walls), 1),
                    "wall_saved_per_run": round(base - mean, 1),
                    "real_failures_missed": missed,
                }
            )
    json.dump(out, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
