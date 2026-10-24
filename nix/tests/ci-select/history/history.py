#!/usr/bin/env python3
# Every CI run the forge has recorded, job by job: what ran, what it cost,
# and what of it was already decided before it ran. The baseline any test
# chooser is scored against: minutes saved against real failures missed.
#
#   export.sh DIR                  the forge's tables and job logs, read only
#   logfacts.py DIR/logs DIR/tasks.csv > DIR/logfacts.json
#   history.py DIR > history.json  this; history-jobs-NN.csv beside it, a row a job
#
# A job's minutes are, in this order:
#   superseded   its run was cancelled for a newer push to the same pull
#                request (or the same commit's run ran twice)
#   infra        it failed for its box, not its code: killed from outside,
#                out of memory, out of disk, the network
#   flake        it failed, and the very derivation it failed on passed in
#                another run (or, for a job with no derivation, the same job
#                passed on the same commit)
#   noop         it passed and built nothing: the answer was already in that
#                box's store
#   determined   it passed and built its check's derivation again, though
#                that derivation had passed before (on the other box, or
#                before a rebase)
#   overhead     files_modified, style, lint, ci_ok and the agree jobs
#   stopped      cancelled when another job of its run failed (stop-run)
#   needed       the rest: a derivation new to CI, built and judged
# A failure is real when it is none of the above.
import collections
import csv
import json
import os
import re
import sys

KINDS = {
    "vm_tests": "vm",
    "vm_test": "vm",
    "build_host": "systems",
    "build_systems": "systems",
    "same_build": "agree",
    "systems_agree": "agree",
    "build_part": "programs",
    "build_client": "programs",
    "build_programs": "programs",
    "lint": "lint",
    "test": "rust",
    "rust": "rust",
    "rust_lint": "rust",
    "rust_check": "rust",
    "app_lint": "app",
    "app_checks": "app",
    "flake_check": "flake",
    "nix_check": "flake",
    "installer_repro": "installer",
    "build_installer": "installer",
    "same_installer": "agree",
    "installer_agrees": "agree",
    "keep_check": "keep",
    "inputs_kept": "keep",
    "files_modified": "files_modified",
    "ci_ok": "ci_ok",
    "style": "style",
    "mail_worker": "browser",
    "worker_tests": "browser",
    "page_tests": "browser",
    "site_checks": "browser",
}
# a job whose own check is one derivation, judged by building it
CHECKS = {"vm", "browser"}
OVERHEAD = {"files_modified", "style", "lint", "ci_ok", "agree", "keep"}
INFRA = {"killed", "oom", "disk", "network"}


def kind_of(job_id, name):
    base = job_id.split(".")[-1]
    if job_id in ("nix_checks", "rust_checks", ""):
        return None, None
    if job_id == "rust_checks.lint" or job_id == "rust_checks.test":
        base = "rust"
    if job_id == "rust_checks.app_lint":
        base = "app_lint"
    k = KINDS.get(base)
    m = re.search(r"\((.*)\)$", name)
    return k, (m.group(1).replace(" ", "") if m else None)


def box_of(name):
    return "node1" if "node1" in name else "node2" if "node2" in name else name


def target(kind, arg, drvs):
    if kind not in CHECKS or not arg:
        return None
    want = f"vm-test-run-{arg}" if kind == "vm" else arg
    hit = [d for d in drvs if d.split("-", 1)[1] == want or want in d.split("-", 1)[1]]
    return hit[-1] if hit else None


def load(d):
    rd = lambda f: list(csv.DictReader(open(os.path.join(d, f))))  # noqa: E731
    runs = {r["id"]: r for r in rd("runs.csv")}
    base = {b["id"]: b for b in rd("base.csv")}
    runners = {r["id"]: box_of(r["name"]) for r in rd("runners.csv")}
    jobs = {j["id"]: j for j in rd("jobs.csv")}
    facts = json.load(open(os.path.join(d, "logfacts.json")))
    tasks = []
    for t in rd("tasks.csv"):
        j = jobs.get(t["job_id"])
        if not j or j["run_id"] not in runs:
            continue
        k, a = kind_of(j["job_id"], j["name"])
        if k is None:
            continue
        s, e = int(t["started"] or 0), int(t["stopped"] or 0)
        if not s or e <= s:
            continue
        f = facts.get(t["id"], {})
        r = runs[j["run_id"]]
        tasks.append(
            {
                "task": int(t["id"]),
                "run": int(r["id"]),
                "run_no": int(r["index"]),
                "event": r["event"],
                "pr": int(r["pull_request_id"] or 0),
                "sha": r["commit_sha"],
                "base": base.get(r["id"], {}).get("base") or "",
                "kind": k,
                "arg": a or "",
                "box": runners.get(t["runner_id"], "?"),
                "start": s,
                "stop": e,
                "min": round((e - s) / 60, 2),
                "status": {"1": "success", "2": "failure", "3": "cancelled"}.get(
                    t["status"], t["status"]
                ),
                "attempt": int(t["attempt"]),
                "built": f.get("built"),
                "drv": target(k, a, f.get("drvs", [])),
                "why": (f.get("why") or [""])[0],
            }
        )
    return runs, tasks


def superseded_runs(runs):
    """runs cancelled for a newer run of the same pull request, and a
    second run of a commit that overlapped the first"""
    by_pr = collections.defaultdict(list)
    for r in runs.values():
        if r["pull_request_id"] not in ("", "0"):
            by_pr[r["pull_request_id"]].append(r)
    out = set()
    for rs in by_pr.values():
        rs.sort(key=lambda r: int(r["created"]))
        for a, b in zip(rs, rs[1:]):
            stop = int(a["stopped"] or 0)
            if a["status"] == "3" and int(b["created"]) <= (stop or 1 << 62):
                out.add(a["id"])
    by_sha = collections.defaultdict(list)
    for r in runs.values():
        by_sha[r["commit_sha"]].append(r)
    for rs in by_sha.values():
        rs.sort(key=lambda r: int(r["created"]))
        for a, b in zip(rs, rs[1:]):
            if (
                int(b["created"]) < int(a["stopped"] or 0)
                and a["workflow_id"] == b["workflow_id"]
            ):
                out.add(b["id"])
    return out


def classify(runs, tasks):
    sup = superseded_runs(runs)
    passed_drv = collections.defaultdict(list)
    passed_job = collections.defaultdict(list)
    for t in tasks:
        if t["status"] == "success":
            if t["drv"]:
                passed_drv[t["drv"]].append(t["stop"])
            passed_job[(t["sha"], t["kind"], t["arg"])].append(t["stop"])
    for t in tasks:
        failed = t["status"] == "failure"
        if str(t["run"]) in sup:
            c = "superseded"
        elif failed and t["why"] in INFRA:
            c = "infra"
        elif failed and t["kind"] in ("style", "lint"):
            c = "real"
        elif failed and (
            (t["drv"] and passed_drv.get(t["drv"]))
            or (not t["drv"] and passed_job.get((t["sha"], t["kind"], t["arg"])))
        ):
            c = "flake"
        elif failed:
            c = "real"
        elif t["status"] == "cancelled":
            c = "superseded" if str(t["run"]) in sup else "stopped"
        elif t["kind"] in OVERHEAD:
            c = "overhead"
        elif t["built"] == 0 and t["kind"] not in ("flake",):
            c = "noop"
        elif t["drv"] and any(s < t["start"] for s in passed_drv[t["drv"]]):
            c = "determined"
        else:
            c = "needed"
        t["class"] = c
    return sup


def summary(runs, tasks, sup):
    by = collections.Counter()
    n = collections.Counter()
    for t in tasks:
        by[t["class"]] += t["min"]
        n[t["class"]] += 1
    total = sum(by.values())
    kinds = collections.defaultdict(collections.Counter)
    for t in tasks:
        kinds[t["kind"]][t["class"]] += t["min"]
    walls = [
        (int(r["stopped"]) - int(r["started"])) / 60
        for r in runs.values()
        if r["stopped"] not in ("", "0") and r["started"] not in ("", "0")
    ]
    sup_wall = sum(
        (int(runs[r]["stopped"]) - int(runs[r]["started"])) / 60
        for r in sup
        if runs[r]["stopped"] not in ("", "0") and runs[r]["started"] not in ("", "0")
    )
    real = [t for t in tasks if t["class"] == "real"]
    return {
        "runs": len(runs),
        "jobs": len(tasks),
        "job_minutes": round(total),
        "by_class": {
            k: {"minutes": round(v), "jobs": n[k], "share": round(v / total, 3)}
            for k, v in by.most_common()
        },
        "by_kind": {
            k: {c: round(m) for c, m in v.most_common()}
            for k, v in sorted(kinds.items())
        },
        "run_wall_minutes": round(sum(walls)),
        "superseded_run_wall_minutes": round(sup_wall),
        "real_failures": collections.Counter(
            f"{t['kind']}:{t['why']}" for t in real
        ).most_common(),
        "flakes_by_check": collections.Counter(
            f"{t['kind']}({t['arg']})" for t in tasks if t["class"] == "flake"
        ).most_common(15),
        "infra_by_why": collections.Counter(
            f"{t['why']}@{t['box']}" for t in tasks if t["class"] == "infra"
        ).most_common(),
    }


def main():
    d = sys.argv[1]
    runs, tasks = load(d)
    sup = classify(runs, tasks)
    keys = [
        "task",
        "run",
        "run_no",
        "event",
        "pr",
        "sha",
        "base",
        "kind",
        "arg",
        "box",
        "start",
        "stop",
        "min",
        "status",
        "attempt",
        "built",
        "drv",
        "why",
        "class",
    ]
    # in parts under a megabyte each (the style check's limit); commits and
    # derivations by their first twelve characters
    rows = sorted(tasks, key=lambda t: t["task"])
    for t in rows:
        t["sha"], t["base"] = t["sha"][:12], t["base"][:12]
        t["drv"] = t["drv"] and t["drv"][:12] + t["drv"][32:]
    for n in range(0, len(rows), 6000):
        with open(
            os.path.join(d, f"history-jobs-{n // 6000:02}.csv"), "w", newline=""
        ) as f:
            w = csv.DictWriter(f, fieldnames=keys, lineterminator="\n")
            w.writeheader()
            for t in rows[n : n + 6000]:
                w.writerow({k: t[k] for k in keys})
    json.dump(summary(runs, tasks, sup), sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
