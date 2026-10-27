#!/usr/bin/env python3
# Every pull request run of the history, chosen again by today's selector
# (plan.py, the very function files_modified calls), in the order the runs
# were made: the jobs each would run, its wall time on the runner slots,
# and every real failure it would have left to main's full run.
#
#   replay.py DIR > replay.json     (run from the repository; DIR as
#                                    history.py's, with changes.json,
#                                    realfail.json and faces.json)
#
# Nothing is fitted to the failures. tests-this.json was read from the
# tests (no entry was added for a failure); the faces are read from each
# run's own two commits (faces.py); the jobs left to main's full run are
# decided at each run from the runs before it alone (main_only_at), never
# from what came after. Each job's minutes are what it took in that run;
# a job the run did not have, or one that was cancelled, killed or flaked,
# counts at its usual minutes (the median of its last 30 good runs).
import collections
import csv
import glob
import json
import os
import statistics
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
sys.path.insert(0, os.path.dirname(HERE))
import ciselect  # noqa: E402
import plan  # noqa: E402
from levers import simulate  # noqa: E402
from realfail import job_of  # noqa: E402

GOOD = ("needed", "determined", "noop", "real", "overhead")
OVERHEAD = {"files_modified", "style", "lint", "ci_ok"}
# what the main-only rule may move: a job that is one check (a VM test, a
# browser or Worker check), the app's, the keep check, a system's build.
# The rust job, flake_check and the programs are not one thing each.
MOVABLE_KINDS = ("vm", "browser", "app", "keep", "systems")
# a job's minutes count at most this toward the rule's minutes (a run the
# forge lost for a day counts as a long run, not a day)
CAP = 60.0


def pct(xs, p):
    xs = sorted(xs)
    if not xs:
        return 0.0
    k = (len(xs) - 1) * p
    f = int(k)
    c = min(f + 1, len(xs) - 1)
    return xs[f] + (xs[c] - xs[f]) * (k - f)


def stats(xs):
    return {
        "n": len(xs),
        "mean": round(statistics.mean(xs), 1) if xs else 0,
        "median": round(statistics.median(xs), 1) if xs else 0,
        "p90": round(pct(xs, 0.9), 1),
    }


class History:
    def __init__(self, d, root="."):
        self.rows = [
            r
            for f in sorted(glob.glob(os.path.join(d, "history-jobs-*.csv")))
            for r in csv.DictReader(open(f))
        ]
        self.runs = {
            r["id"]: r for r in csv.DictReader(open(os.path.join(d, "runs.csv")))
        }
        rf = json.load(open(os.path.join(d, "realfail.json")))
        self.failures = rf["failures"]
        self.changes = json.load(open(os.path.join(d, "changes.json")))["runs"]
        fp = os.path.join(d, "faces.json")
        self.faces = json.load(open(fp)) if os.path.exists(fp) else {}
        self.catches = {
            int(k): v
            for k, v in json.load(open(os.path.join(HERE, "catches.json"))).items()
            if not k.startswith("_")
        }

        def read(f):
            p = os.path.join(root, f)
            return open(p).read() if os.path.exists(p) else ""

        self.sets = plan.load_sets(read(plan.SETS))
        self.registry = ciselect.registry(read(plan.REGISTRY))
        self.crates = ciselect.crates(read)
        self.hosts = ciselect.hosts(read, self.sets)
        self.docs_named = ciselect.named_docs(
            lambda: (
                (f, read(f))
                for f in os.popen("git ls-files '*.nix' '*.rs'").read().split()
            )
        )
        for r in self.rows:
            r["job"] = job_of(r)
            r["m"] = float(r["min"])
            r["t"] = int(self.runs[r["run"]]["created"])
            r["pr_run"] = r["event"].startswith("pull_request")
            r["key"], r["cross"] = self.key_of(r)
        self.by_run = collections.defaultdict(list)
        for r in self.rows:
            if r["pr_run"]:
                self.by_run[r["run"]].append(r)
        self.order = sorted(self.by_run, key=lambda x: int(self.runs[x]["created"]))
        good = collections.defaultdict(list)
        for r in sorted(self.rows, key=lambda r: r["t"]):
            if r["class"] in GOOD and r["status"] == "success":
                good[r["key"]].append((r["t"], r["m"]))
                good["kind:" + r["kind"]].append((r["t"], r["m"]))
        self.good = good
        # a test that did not exist yet cannot be added to an old run
        self.first = {}
        for r in self.rows:
            if r["kind"] in ("vm", "browser"):
                self.first[r["key"]] = min(self.first.get(r["key"], r["t"]), r["t"])
        # each real failure, once, with the runs it failed in
        inc = collections.defaultdict(list)
        for f in self.failures:
            if f["lane"] == "pr":
                inc[(f["job"], f["fixed_by"])].append(f)
        self.incidents = [sorted(fs, key=lambda f: f["when"]) for fs in inc.values()]

    def key_of(self, r):
        """the job's key in tests-this.json, and whether it is a box's
        system built on another box (main's full run's alone)"""
        k, a = r["kind"], r["arg"]
        if k == "vm":
            return (a or "*vm"), False
        if k == "browser":
            return (a or "mail-worker"), False
        if k == "systems":
            host, _, on = a.partition(",")
            key = self.hosts.get(host, plan.SYSTEMS)
            return key, (on not in (host, "?", ""))
        return k, False

    def usual(self, key, kind, t):
        """the median of the job's last 30 good runs before t"""
        for k in (key, "kind:" + kind):
            xs = [m for (tt, m) in self.good.get(k, []) if tt < t][-30:]
            if not xs:
                xs = [m for (_, m) in self.good.get(k, [])][:30]
            if xs:
                return statistics.median(xs)
        return 1.0

    def minutes(self, r):
        if r["class"] in GOOD:
            return r["m"]
        return self.usual(r["key"], r["kind"], r["t"])

    def catch_keys(self, f):
        c = self.catches.get(int(f["run_no"]))
        if c:
            return c["catch"]
        k, a = f["kind"], f["job"]
        if k in ("vm", "browser"):
            arg = a[a.index("(") + 1 : -1] if "(" in a else ""
            return [arg or ("*vm" if k == "vm" else "mail-worker")]
        return [k]

    def cause(self, f):
        """the failure's cause when it was not the change's code (catches.json)"""
        c = (self.catches.get(int(f["run_no"])) or {}).get("cause")
        if c:
            return c
        return "main" if f["fixed_by_rebase"] else None

    def is_eval_failure(self, f):
        c = self.catches.get(int(f["run_no"]))
        return bool(c) and c.get("eval", False)


def main_only_at(h, t, min_tasks, min_minutes, evidence=False):
    """the jobs that, by the runs before t alone, have never caught a real
    failure (of any cause) and have run often and long enough to say so"""
    tasks = collections.Counter()
    mins = collections.Counter()
    for r in h.rows:
        if r["t"] >= t or not r["pr_run"] or r["class"] == "superseded":
            continue
        if r["kind"] not in MOVABLE_KINDS or r["cross"]:
            continue
        tasks[r["key"]] += 1
        mins[r["key"]] += min(r["m"], CAP)
    caught = set()
    for fs in h.incidents:
        # a failure of the box, the forge or CI itself, or a test's own
        # race, is not a catch
        if fs[0]["when"] < t and h.cause(fs[0]) not in ("infra", "ci", "test"):
            caught |= set(h.catch_keys(fs[0]))
    keys = {
        k
        for k in tasks
        if tasks[k] >= min_tasks and mins[k] >= min_minutes and k not in caught
    }
    if evidence:
        return {k: {"runs": tasks[k], "minutes": round(mins[k])} for k in sorted(keys)}
    return keys


def choose(h, run, variant, main_only):
    """the plan for one run under a variant"""
    files = h.changes.get(run)
    if files is None:
        return None
    faces = h.faces.get(run, {}) if variant["faces"] else None
    if faces is not None:
        # a component the run touched but whose face was never read counts
        # as changed
        faces = {n: d for n, d in faces.items()}
    p = plan.plan(
        [("M", f) for f in files],
        h.sets,
        h.registry,
        main_only=main_only if variant["main_only"] else (),
        faces=faces,
        docs_named=h.docs_named,
        crates=h.crates,
        hosts=h.hosts,
    )
    return p


def selected(p, key):
    if key == "programs":
        return p["programs"]
    if key == "*vm":
        return bool(p["vm_tests"])
    return key in p["why"]


def jobs_for(h, run, rs, p, variant, eval_ratio):
    """[(kind, minutes, box)] the run would have, and the keys it ran"""
    t = rs[0]["t"]
    out = []
    have = set()
    if p is None:
        # no record of what it changed: what it ran
        for r in rs:
            if variant["structural"] and r["kind"] in ("systems", "agree", "installer"):
                continue
            out.append((r["kind"], h.minutes(r), None))
        return out
    hosts_done = set()
    for r in sorted(rs, key=lambda r: (r["cross"], r["m"])):
        k, kind = r["key"], r["kind"]
        if r["job"].split("(")[0] in OVERHEAD:
            out.append((kind, h.minutes(r), None))
            continue
        if kind in ("agree", "installer"):
            continue
        if kind == "systems":
            host = r["arg"].partition(",")[0]
            if r["cross"] or host in hosts_done or host not in p["hosts"]:
                continue
            hosts_done.add(host)
            have.add("host:" + host)
            out.append((kind, h.minutes(r), None))
            continue
        if kind == "flake":
            if not (
                p["flake"] if variant["narrow_flake"] else (p["flake"] or p["vm_tests"])
            ):
                continue
            m = h.minutes(r)
            if variant["narrow_flake"]:
                m = narrowed(m, p, eval_ratio)
            have.add("flake")
            out.append((kind, m, None))
            continue
        if kind == "rust":
            if not p["rust"]:
                continue
            m = h.minutes(r)
            if not p["rust_crates"]:
                m = min(m, 1.0)  # fmt alone
            have.add("rust")
            out.append((kind, m, None))
            continue
        if kind == "programs":
            if p["programs"]:
                have.add("programs:" + (r["arg"] or "all"))
                out.append((kind, h.minutes(r), None))
            continue
        if kind == "vm" and k == "*vm":
            if p["vm_tests"]:
                have.update(p["vm_tests"])
                out.append((kind, h.minutes(r), None))
            continue
        if selected(p, k):
            have.add(k)
            out.append((kind, h.minutes(r), None))
    # what the plan runs that the run did not have, at its usual minutes
    for k in p["vm_tests"]:
        if k not in have and h.first.get(k, 1 << 62) <= t:
            out.append(("vm", h.usual(k, "vm", t), None))
    for g in plan.GROUPS:
        for k in p[g]:
            if k not in have and h.first.get(k, 1 << 62) <= t:
                out.append(("browser", h.usual(k, "browser", t), None))
    if p["programs"] and not any(x.startswith("programs:") for x in have):
        for part in ("cli", "boxes"):
            out.append(("programs", h.usual(f"programs({part})", "programs", t), None))
    if p["rust"] and "rust" not in have:
        m = h.usual("rust", "rust", t)
        out.append(("rust", m if p["rust_crates"] else min(m, 1.0), None))
    if p["app"] and "app" not in {x[0] for x in out}:
        out.append(("app", h.usual("app", "app", t), None))
    if p["keep"] and "keep" not in {x[0] for x in out}:
        out.append(("keep", h.usual("keep", "keep", t), None))
    if (
        p["flake"] or (not variant["narrow_flake"] and p["vm_tests"])
    ) and "flake" not in have:
        m = h.usual("flake", "flake", t)
        if variant["narrow_flake"]:
            m = narrowed(m, p, eval_ratio)
        out.append(("flake", m, None))
    for host in p["hosts"]:
        if "host:" + host not in have:
            out.append(
                (
                    "systems",
                    h.usual(h.hosts.get(host, plan.SYSTEMS), "systems", t),
                    None,
                )
            )
    return out


def narrowed(m, p, eval_ratio):
    """flake_check on a pull request: every check and every box's system
    evaluated (the share eval_ratio of the full evaluation: no packages,
    no dev shells), and only the chosen evaluation-only checks built
    (about a minute in the history's logs); checkout and setup 0.3"""
    full_eval = max(m - 1.3, 0.0)
    return min(
        m,
        0.3
        + (eval_ratio * full_eval if p["systems_eval"] else 0.0)
        + (1.0 if p["eval_checks"] else 0.0),
    )


def caught_by(h, p, f, variant):
    if p is None:
        return True
    keys = h.catch_keys(f)
    if any(selected(p, k) for k in keys):
        return True
    # flake_check evaluating every check catches what fails to evaluate
    if h.is_eval_failure(f) and (
        p["systems_eval"] if variant["narrow_flake"] else (p["flake"] or p["vm_tests"])
    ):
        return True
    # rust's fmt catches a crate's fmt failure when any crate runs
    return False


VARIANTS = {
    "what ran": None,
    "systems and installer on main (ciinterface)": {
        "structural": True,
        "sets": False,
        "faces": False,
        "main_only": False,
        "narrow_flake": False,
    },
    "+ tests-this sets": {
        "structural": True,
        "sets": True,
        "faces": False,
        "main_only": False,
        "narrow_flake": False,
    },
    "+ interface faces": {
        "structural": True,
        "sets": True,
        "faces": True,
        "main_only": False,
        "narrow_flake": False,
    },
    "+ never-caught jobs on main (time-ordered)": {
        "structural": True,
        "sets": True,
        "faces": True,
        "main_only": True,
        "narrow_flake": False,
    },
    "+ flake_check narrowed (combined)": {
        "structural": True,
        "sets": True,
        "faces": True,
        "main_only": True,
        "narrow_flake": True,
    },
}


def run_replay(h, slots, eval_ratio, min_tasks, min_minutes, static_main_only=None):
    results = {}
    mo_cache = {}
    for name, v in VARIANTS.items():
        per_run = []
        deferred = []
        plans = {}
        for run in h.order:
            rs = h.by_run[run]
            t = rs[0]["t"]
            superseded = any(r["class"] == "superseded" for r in rs)
            if v is None:
                js = [(r["kind"], h.minutes(r), None) for r in rs]
            elif not v["sets"]:
                js = [
                    (r["kind"], h.minutes(r), None)
                    for r in rs
                    if r["kind"] not in ("systems", "agree", "installer")
                ]
            else:
                if v["main_only"]:
                    if static_main_only is not None:
                        mo = static_main_only
                    else:
                        if t not in mo_cache:
                            mo_cache[t] = main_only_at(h, t, min_tasks, min_minutes)
                        mo = mo_cache[t]
                else:
                    mo = set()
                p = choose(h, run, v, mo)
                plans[run] = p
                js = jobs_for(h, run, rs, p, v, eval_ratio)
            wall = simulate(js, slots) if js else 0.0
            heavy = [j for j in js if j[0] not in OVERHEAD]
            per_run.append(
                {
                    "run": int(run),
                    "t": t,
                    "run_no": int(rs[0]["run_no"]),
                    "pr": int(rs[0]["pr"]),
                    "superseded": superseded,
                    "jobs": len(heavy),
                    "job_minutes": round(sum(j[1] for j in heavy), 2),
                    "wall": round(wall, 2),
                }
            )
        if v is not None and v["sets"]:
            for fs in h.incidents:
                runs_failed = [str(f["run"]) for f in fs]
                ok = any(
                    caught_by(h, plans.get(r), f, v)
                    for r, f in zip(runs_failed, fs)
                    if r in plans
                )
                if not ok and any(r in plans for r in runs_failed):
                    f0 = fs[0]
                    deferred.append(
                        {
                            "run_no": f0["run_no"],
                            "pr": f0["pr"],
                            "job": f0["job"],
                            "caught_by": h.catch_keys(f0),
                            "cause": h.cause(f0) or "pr",
                            "files": len(h.changes.get(str(f0["run"])) or []),
                        }
                    )
        elif v is not None:
            # every job but the systems, agree and installer ran as before
            for fs in h.incidents:
                if all(f["kind"] in ("systems", "agree", "installer") for f in fs):
                    f0 = fs[0]
                    deferred.append(
                        {
                            "run_no": f0["run_no"],
                            "pr": f0["pr"],
                            "job": f0["job"],
                            "caught_by": h.catch_keys(f0),
                            "cause": h.cause(f0) or "pr",
                            "files": len(h.changes.get(str(f0["run"])) or []),
                        }
                    )
        results[name] = (per_run, deferred, plans)
    return results


def summarise(per_run, deferred, since=None):
    live = [
        x for x in per_run if not x["superseded"] and (since is None or x["t"] >= since)
    ]
    per_pr = collections.defaultdict(lambda: collections.Counter())
    for x in live:
        p = per_pr[x["pr"]]
        p["wall"] += x["wall"]
        p["job_minutes"] += x["job_minutes"]
        p["runs"] += 1
        p["last"] = x["wall"]  # runs come in the order they were made
    prs = list(per_pr.values())
    return {
        "per_run_wall": stats([x["wall"] for x in live]),
        # what a pull request waits on before it merges: its last run
        "per_pr_last_run_wall": stats([p["last"] for p in prs]),
        "per_run_job_minutes": stats([x["job_minutes"] for x in live]),
        "per_pr_wall": stats([p["wall"] for p in prs]),
        "per_pr_job_minutes": stats([p["job_minutes"] for p in prs]),
        "pr_job_minutes_total": round(
            sum(x["job_minutes"] for x in per_run if since is None or x["t"] >= since)
        ),
        "deferred": sorted(deferred, key=lambda d: d["run_no"]),
        "deferred_pr_caused": len([d for d in deferred if d["cause"] == "pr"]),
        "deferred_other": collections.Counter(
            d["cause"] for d in deferred if d["cause"] != "pr"
        ),
    }


def main():
    d = sys.argv[1]
    args = sys.argv[2:]

    def opt(name, default):
        return type(default)(args[args.index(name) + 1]) if name in args else default

    def slots_of(s):
        return {k: int(v) for k, v in (x.split("=") for x in s.split(","))}

    slots = slots_of(opt("--slots", "node1=4,node2=2"))
    old = slots_of(opt("--old-slots", "node1=2,node2=2"))
    eval_ratio = opt("--eval-ratio", 0.815)
    min_tasks = opt("--min-tasks", 300)
    min_minutes = opt("--min-minutes", 300.0)
    h = History(d)
    if "--main-only" in args:
        # main-only.json: the rule at the end of the history
        last = max(r["t"] for r in h.rows)
        jobs = main_only_at(h, last + 1, min_tasks, min_minutes, evidence=True)
        json.dump(
            {
                "_": (
                    "Jobs that run in main's full run alone: by every pull request "
                    "run in the history, each ran at least "
                    f"{min_tasks} times and {round(min_minutes)} minutes and never "
                    "caught a real failure (a failure of the box, the forge, CI "
                    "itself or a test's own race is not one). Written by "
                    "history/replay.py --main-only; the replay applies the same "
                    "rule at each run from the runs before it alone. A job main's "
                    "full run catches failing comes off this list."
                ),
                "as_of": time.strftime("%Y-%m-%d", time.gmtime(last)),
                "jobs": jobs,
            },
            sys.stdout,
            indent=1,
        )
        print()
        return
    out = {
        "settings": {
            "slots": slots,
            "old_slots": old,
            "systems_eval_share_of_full_eval": eval_ratio,
            "main_only_rule": {"min_tasks": min_tasks, "min_minutes": min_minutes},
        },
        "runs": len(h.order),
        "incidents": len(h.incidents),
    }
    res = run_replay(h, slots, eval_ratio, min_tasks, min_minutes)
    out["variants"] = {n: summarise(pr, de) for n, (pr, de, _) in res.items()}
    base_old = run_replay(h, old, eval_ratio, min_tasks, min_minutes)
    out["what ran, old slots"] = summarise(*base_old["what ran"][:2])
    out["combined, old slots"] = summarise(
        *base_old["+ flake_check narrowed (combined)"][:2]
    )
    # the last two weeks: today's tests, today's sizes
    since = int(time.mktime(time.strptime(opt("--since", "2026-10-13"), "%Y-%m-%d")))
    out["since " + opt("--since", "2026-10-13")] = {
        n: {
            k: v
            for k, v in summarise(pr, [], since).items()
            if k
            in (
                "per_run_wall",
                "per_pr_last_run_wall",
                "per_pr_wall",
                "pr_job_minutes_total",
            )
        }
        for n, (pr, de, _) in res.items()
    }
    final = res["+ flake_check narrowed (combined)"]
    # the list main-only.json holds: the rule at the end of the history
    now = 1 << 62
    out["main_only_now"] = sorted(main_only_at(h, now, min_tasks, min_minutes))
    with open(os.path.join(d, "selector-runs.csv"), "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        names = list(res)
        w.writerow(
            ["run", "run_no", "pr", "superseded", "created"]
            + [f"wall:{n}" for n in names]
        )
        for i, x in enumerate(res["what ran"][0]):
            w.writerow(
                [x["run"], x["run_no"], x["pr"], x["superseded"], x["t"]]
                + [res[n][0][i]["wall"] for n in names]
            )
    plans = final[2]
    out["chosen_jobs_per_run"] = stats(
        [len(p["why"]) for p in plans.values() if p is not None]
    )
    json.dump(out, sys.stdout, indent=1)
    print()


if __name__ == "__main__":
    main()
