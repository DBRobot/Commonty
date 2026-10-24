#!/usr/bin/env python3
# Which job-minutes in the history could have caught anything, and what a
# chooser by relevance would have run instead. Reads history.py's rows,
# realfail.py's real failures, changes.py's changed files, phases.py's VM
# phases, score.py's two choosers, and tests-this.json (each job's code it
# specifically exercises).
#
#   relevance.py DIR tests-this.json > relevance.json
#
# Every chooser is scored on pull request runs only; main's full run after a
# merge runs everything, so a real failure a chooser leaves out is deferred
# there, not missed.
import collections
import csv
import glob
import json
import os
import re
import statistics
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
from levers import simulate  # noqa: E402
from realfail import job_of  # noqa: E402

ALWAYS = {"files_modified", "style", "lint", "ci_ok"}
WASTE = ("superseded", "stopped", "infra", "flake")
# docs alone change nothing a job checks (ciselect.py's docs rule)
DOCS = re.compile(r"^(docs/|\.claude/|\.github/|[^/]+\.md$|AGENTS\.md$)")


def key_of(job):
    """the tests-this entry a job is judged by"""
    m = re.match(r"(\w+)(?:\((.*)\))?$", job)
    k, a = m.group(1), m.group(2) or ""
    if k in ("vm", "browser"):
        return a or "*vm"
    if k == "programs":
        # build_client of the first weeks built dd and ran its tests
        return f"programs({a})" if a else "programs(build_client)"
    if k in ("systems", "agree"):
        return (
            "systems(home-controller)" if a.startswith("home-controller") else "systems"
        )
    if k == "installer":
        return "installer"
    return k


def rx(globs):
    pats = []
    for g in globs:
        g = g.rstrip("/")
        if not any(c in g for c in "*?["):
            pats.append(re.escape(g) + r"(/.*)?")
        else:
            s = re.escape(g).replace(r"\*\*/", "(.*/)?").replace(r"\*\*", ".*")
            s = s.replace(r"\*", "[^/]*").replace(r"\?", "[^/]")
            pats.append(s)
    return re.compile("^(" + "|".join(pats) + ")$") if pats else None


def load_sets(path):
    raw = json.load(open(path))
    sets = {k: v for k, v in raw.items() if not k.startswith("_")}
    shared = raw.get("_shared", {}).get("paths", [])
    shared_by = raw.get("_shared_by_family", {})
    out = {}
    for k, v in sets.items():
        fam = v.get("family", "vm" if k in raw.get("_vm", []) else "")
        g = list(v["paths"]) + list(v.get("widened", {}).keys())
        out[k] = {
            "rx": rx(g),
            "rx_read": rx(v["paths"]),
            "shared": rx(shared_by.get(fam, shared if fam == "vm" else [])),
        }
    return raw, out


def hits(s, files, read_only=False):
    r = s["rx_read"] if read_only else s["rx"]
    return [
        f for f in files if (r and r.match(f)) or (s["shared"] and s["shared"].match(f))
    ]


def component(p):
    s = p.split("/")
    if s[0] == "nix" and len(s) > 3 and s[1] in ("modules", "tests"):
        return "/".join(s[:3])
    if s[0] == "nix" and len(s) > 2:
        return "/".join(s[:3]) if len(s) > 3 else p
    if s[0] in ("servers", "client", "app") and len(s) > 2:
        if s[1] == "system" and len(s) > 3:
            return "/".join(s[:3])
        return "/".join(s[:2])
    return p if len(s) <= 2 else "/".join(s[:2])


def main():
    d, sets_path = sys.argv[1], sys.argv[2]
    rows = [
        r
        for f in sorted(glob.glob(os.path.join(d, "history-jobs-*.csv")))
        for r in csv.DictReader(open(f))
    ]
    runs = {r["id"]: r for r in csv.DictReader(open(os.path.join(d, "runs.csv")))}
    rf = json.load(open(os.path.join(d, "realfail.json")))
    cj = json.load(open(os.path.join(d, "changes.json")))
    changes, ren = cj["runs"], cj["rename_map"]
    phases = json.load(open(os.path.join(d, "phases.json")))
    scores = {}
    for name in ("covers", "faces"):
        p = os.path.join(d, f"score-{name}.json")
        if os.path.exists(p):
            scores[name] = json.load(open(p))["chosen"]
    raw, sets = load_sets(sets_path)
    vm_keys = [k for k in raw.get("_vm", [])]

    for r in rows:
        r["job"] = job_of(r)
        r["key"] = key_of(r["job"])
        r["m"] = float(r["min"])
        r["created"] = int(runs[r["run"]]["created"])
        r["pr_run"] = r["event"].startswith("pull_request")
    fails = rf["failures"]
    caught_tasks = {f["task"] for f in fails}
    incidents = [f for f in fails if not f["repeat"]]
    inc_tasks = collections.defaultdict(list)  # incident -> its failing tasks
    for f in fails:
        inc_tasks[(f["lane"], f["job"], f["fixed_by"])].append(f)

    # ---- 1/2: per job
    per = collections.defaultdict(lambda: collections.Counter())
    for r in rows:
        p = per[r["job"]]
        p["tasks"] += 1
        p["minutes"] += r["m"]
        p["runs_pr"] += r["pr_run"]
        if int(r["task"]) in caught_tasks:
            p["caught_minutes"] += r["m"]
            p["failed_runs"] += 1
        if r["class"] in WASTE:
            p["waste_" + r["class"]] += r["m"]
    for f in incidents:
        per[f["job"]]["incidents"] += 1
        per[f["job"]]["incidents_rebase"] += f["fixed_by_rebase"]
    table = []
    for j, p in per.items():
        if j.split("(")[0] in ALWAYS or j == "agree":
            continue
        table.append(
            {
                "job": j,
                "tasks": p["tasks"],
                "minutes": round(p["minutes"]),
                "real_failures": p["incidents"],
                "failing_runs": p["failed_runs"],
                "of_them_fixed_by_rebase": p["incidents_rebase"],
                "wasted_minutes": round(p["minutes"] - p["caught_minutes"]),
                "minutes_per_real_failure": round(p["minutes"] / p["incidents"])
                if p["incidents"]
                else None,
            }
        )
    table.sort(key=lambda t: -t["wasted_minutes"])

    # ---- selectors over pull request runs
    pr_rows = [r for r in rows if r["pr_run"]]
    by_run = collections.defaultdict(list)
    for r in pr_rows:
        by_run[r["run"]].append(r)
    order = sorted(by_run, key=lambda x: int(runs[x]["created"]))

    def files_of(run):
        return changes.get(run)

    def touches(key, files, read_only=False):
        if key == "*vm":
            return any(hits(sets[k], files, read_only) for k in vm_keys if k in sets)
        s = sets.get(key)
        if s is None:
            return True  # a job with no set runs (new or unknown)
        return bool(hits(s, files, read_only))

    def tests_this(read_only=False, strip=()):
        def choose(run, rs):
            files = files_of(run)
            if files is None:
                return {r["task"] for r in rs}
            files = [f for f in files if f not in strip]
            # docs count only for a job whose set names them (the site
            # crawls); ciselect.py's docs rule otherwise
            keep = set()
            any_vm = False
            for r in rs:
                if r["job"].split("(")[0] in ALWAYS:
                    keep.add(r["task"])
                elif r["kind"] in ("programs",):
                    continue
                elif touches(r["key"], files, read_only):
                    keep.add(r["task"])
                    any_vm |= r["kind"] == "vm"
            for r in rs:
                if r["kind"] == "programs" and (
                    any_vm or touches(r["key"], files, read_only)
                ):
                    keep.add(r["task"])
                if r["kind"] == "agree" and any(
                    x["task"] in keep for x in rs if x["kind"] == "systems"
                ):
                    keep.add(r["task"])
            return keep

        return choose

    def learned(min_files=30, hindsight=False):
        """runs a job when the change touches its own test files, or a
        component a change that earlier really broke it touched; learns
        from each real failure at the moment it failed"""
        know = collections.defaultdict(set)
        events = sorted(
            ((f["when"], f) for f in fails if f["lane"] == "pr"), key=lambda e: e[0]
        )
        state = {"i": 0}

        def declared(key):
            if key.startswith(
                ("programs", "systems", "installer", "flake", "rust", "app", "keep")
            ):
                base = {
                    "rust": [
                        "servers/**/*.rs",
                        "client/**",
                        "Cargo.*",
                        "servers/*/Cargo.toml",
                    ],
                    "app": ["app/**"],
                    "installer": ["nix/installer/**"],
                    "flake": ["flake.nix", "flake.lock", "nix/tests/registry.nix"],
                    "keep": ["nix/tests/ci-keep.nix"],
                    "systems": ["flake.lock"],
                    "systems(home-controller)": ["nix/controller/**"],
                }.get(key, [])
                return rx(base)
            return rx(
                [f"nix/tests/{key}.nix", f"nix/tests/{key}.py", f"nix/tests/{key}/**"]
            )

        decl = {}

        def choose(run, rs):
            t = 1 << 62 if hindsight else int(runs[run]["created"])
            while state["i"] < len(events) and events[state["i"]][0] < t:
                f = events[state["i"]][1]
                ch = changes.get(str(f["run"])) or []
                ch = [x for x in ch if not DOCS.match(x)]
                src = (
                    ch
                    if len(ch) <= min_files
                    else [x for x in ch if x in set(f["fix_changed"])] or ch
                )
                know[key_of(f["job"])] |= {component(x) for x in src}
                state["i"] += 1
            files = files_of(run)
            if files is None:
                return {r["task"] for r in rs}
            files = [f for f in files if not DOCS.match(f)]
            comps = {component(f) for f in files}
            keep, any_vm = set(), False
            for r in rs:
                k = r["key"]
                if r["job"].split("(")[0] in ALWAYS:
                    keep.add(r["task"])
                    continue
                if r["kind"] == "programs":
                    continue
                if k not in decl:
                    decl[k] = declared(k)
                dk = decl[k]
                lk = know[k] | (
                    set().union(*(know[v] for v in vm_keys)) if k == "*vm" else set()
                )
                if (dk and any(dk.match(f) for f in files)) or comps & lk:
                    keep.add(r["task"])
                    any_vm |= r["kind"] == "vm"
            for r in rs:
                if r["kind"] == "programs" and any_vm:
                    keep.add(r["task"])
                if r["kind"] == "agree" and any(
                    x["task"] in keep for x in rs if x["kind"] == "systems"
                ):
                    keep.add(r["task"])
            return keep

        return choose

    def from_score(name):
        ch = scores.get(name, {})

        def choose(run, rs):
            c = ch.get(run)
            keep = set()
            for r in rs:
                if (
                    c
                    and "error" not in c
                    and c["keep"] is not None
                    and r["kind"] in ("vm", "browser")
                    and r["arg"] not in c["keep"]
                ):
                    continue
                keep.add(r["task"])
            return keep

        return choose

    def score(choose, scope=None):
        saved = 0.0
        kept_all = {}
        deferred_tasks = []
        for run in order:
            rs = by_run[run]
            keep = choose(run, rs)
            kept_all[run] = keep
            for r in rs:
                if scope and r["kind"] not in scope:
                    continue
                if r["task"] not in keep:
                    saved += r["m"]
                    if int(r["task"]) in caught_tasks:
                        deferred_tasks.append(int(r["task"]))
        dset = set(deferred_tasks)
        deferred = []
        for key, fs in inc_tasks.items():
            if key[0] != "pr":
                continue
            if scope and fs[0]["kind"] not in scope:
                continue
            if all(f["task"] in dset for f in fs):
                f0 = fs[0]
                deferred.append(
                    f"run {f0['run_no']} {f0['job']} pr {f0['pr']}"
                    + (" (fixed by rebase)" if f0["fixed_by_rebase"] else "")
                )
        total = sum(r["m"] for r in pr_rows if not scope or r["kind"] in scope)
        return {
            "pr_job_minutes": round(total),
            "minutes_saved": round(saved),
            "share_saved": round(saved / total, 3),
            "failing_runs_deferred": len(dset),
            "real_failures_deferred": len(deferred),
            "deferred": sorted(deferred),
        }, kept_all

    HUBS = ("flake.nix", "Cargo.lock", "Cargo.nix", "Cargo.toml", "crate-hashes.json")
    sel = {}
    kept = {}
    for name, ch in (
        ("tests-this (as read)", tests_this(True)),
        ("tests-this (widened by the failures)", tests_this(False)),
        (
            "tests-this, hub files (flake.nix, Cargo.*) left to main's run",
            tests_this(False, HUBS),
        ),
        (
            "learned, time-ordered (components of earlier real breaks + own files)",
            learned(),
        ),
    ):
        sel[name], kept[name] = score(ch)
    vb = ("vm", "browser")
    cmp_ = {}
    for name, ch in (
        ("today's chooser (covers.json)", from_score("covers")),
        ("interface chooser", from_score("faces")),
        ("tests-this (as read)", tests_this(True)),
        ("tests-this (widened by the failures)", tests_this(False)),
        ("tests-this, hub files left to main's run", tests_this(False, HUBS)),
        ("learned, time-ordered", learned()),
    ):
        cmp_[name], _ = score(ch, vb)

    # ---- validation: every real failure's change must touch its job's set
    validate = []
    for f in incidents:
        if f["lane"] != "pr":
            continue
        k = key_of(f["job"])
        files = changes.get(str(f["run"])) or []
        hit_read = touches(k, files, True)
        hit = touches(k, files, False)
        moved = [ren.get(x, x) for x in f.get("main_moved", []) if not DOCS.match(x)]
        from_main = bool(moved) and touches(k, moved, False)
        validate.append(
            {
                "run_no": f["run_no"],
                "job": f["job"],
                "set": k,
                "pr": f["pr"],
                "touched_as_read": hit_read,
                "touched_widened": hit,
                "fixed_by_rebase": f["fixed_by_rebase"],
                "main_moved_its_set": from_main,
                "files": len(files),
                "fix_changed": f["fix_changed"][:12],
            }
        )

    # what keeps the chosen jobs running: per job family, minutes kept, and
    # the changed files that most often put a job in
    kept_by = collections.Counter()
    trig = collections.defaultdict(collections.Counter)
    W0 = "tests-this (widened by the failures)"
    for run in order:
        files = files_of(run) or []
        for r in by_run[run]:
            if r["task"] in kept[W0][run] and r["job"].split("(")[0] not in ALWAYS:
                fam = r["kind"]
                kept_by[fam] += r["m"]
                k = r["key"]
                if k in sets:
                    for f in hits(sets[k], files):
                        trig[fam][f] += 1
    kept_breakdown = {
        fam: {
            "minutes_kept": round(m),
            "top_triggers": trig[fam].most_common(8),
        }
        for fam, m in kept_by.most_common()
    }

    # ---- 4: the true needed figure
    W = "tests-this (widened by the failures)"
    k_w = kept[W]
    split = collections.Counter()
    for r in rows:
        if not r["pr_run"]:
            split["main's full run (the backstop)"] += r["m"]
        elif r["class"] in WASTE:
            split[f"waste: {r['class']}"] += r["m"]
        elif int(r["task"]) in caught_tasks:
            split["caught a real failure in that run"] += r["m"]
        elif r["job"].split("(")[0] in ALWAYS or r["kind"] == "agree":
            split["overhead (files_modified, style, lint, ci_ok, agree)"] += r["m"]
        elif r["task"] in k_w.get(r["run"], set()):
            split["relevant: change touched what it tests, passed"] += r["m"]
        else:
            split[
                "could not have caught anything: change touched nothing it tests"
            ] += r["m"]
    tot = sum(split.values())
    needed = {
        k: {"minutes": round(v), "share": round(v / tot, 3)}
        for k, v in split.most_common()
    }
    # the same split by the empirical measure alone: a job is relevant to a
    # change that touches its own test files or a component some change that
    # really broke it ever touched (all of history, in hindsight)
    _, k_e = score(learned(hindsight=True))
    split_e = collections.Counter()
    for r in rows:
        if not r["pr_run"] or r["class"] in WASTE or int(r["task"]) in caught_tasks:
            continue
        if r["job"].split("(")[0] in ALWAYS or r["kind"] == "agree":
            continue
        if r["task"] in k_e.get(r["run"], set()):
            split_e["relevant by the empirical measure, passed"] += r["m"]
        else:
            split_e["could not have caught anything by the empirical measure"] += r["m"]
    needed_empirical = {
        k: {"minutes": round(v), "share": round(v / tot, 3)}
        for k, v in split_e.most_common()
    }

    # ---- per PR replay: jobs and simulated wall minutes, today and chosen
    slots = {"node1": 2, "node2": 1}

    def wall(rs, keep=None):
        js = [
            (
                r["kind"],
                r["m"],
                r["box"] if r["kind"] in ("systems", "installer") else None,
            )
            for r in rs
            if keep is None or r["task"] in keep
        ]
        return simulate(js, slots) if js else 0.0

    per_pr = collections.defaultdict(lambda: collections.Counter())
    run_rows = []
    for run in order:
        rs = by_run[run]
        if any(r["class"] in ("superseded",) for r in rs):
            continue
        keep = k_w[run]
        heavy = [
            r
            for r in rs
            if r["job"].split("(")[0] not in ALWAYS and r["kind"] != "agree"
        ]
        hk = [r for r in heavy if r["task"] in keep]
        w0, w1 = wall(rs), wall(rs, keep)
        m0 = sum(r["m"] for r in heavy)
        m1 = sum(r["m"] for r in hk)
        run_rows.append(
            {
                "run_no": int(rs[0]["run_no"]),
                "pr": int(rs[0]["pr"]),
                "jobs_today": len(heavy),
                "jobs_chosen": len(hk),
                "minutes_today": round(m0, 1),
                "minutes_chosen": round(m1, 1),
                "wall_today": round(w0, 1),
                "wall_chosen": round(w1, 1),
            }
        )
        p = per_pr[rs[0]["pr"]]
        p["runs"] += 1
        p["jobs_today"] += len(heavy)
        p["jobs_chosen"] += len(hk)
        p["min_today"] += m0
        p["min_chosen"] += m1
        p["wall_today"] += w0
        p["wall_chosen"] += w1

    def stats(xs):
        return {
            "mean": round(statistics.mean(xs), 1),
            "median": round(statistics.median(xs), 1),
        }

    rr = [x for x in run_rows if x["jobs_today"]]
    vmshare = []
    for run in order:
        rs = [r for r in by_run[run] if r["kind"] in ("vm", "browser")]
        if any(r["class"] == "superseded" for r in by_run[run]) or not rs:
            continue
        t0 = sum(r["m"] for r in rs)
        t1 = sum(r["m"] for r in rs if r["task"] in k_w[run])
        vmshare.append(t1 / t0 if t0 else 0)
    skip_most_runs = sum(
        1 for x in rr if x["minutes_chosen"] <= 0.5 * x["minutes_today"]
    )
    skip_all_runs = sum(1 for x in rr if x["jobs_chosen"] == 0)
    prs = [p for p in per_pr.values() if p["jobs_today"]]
    skip_most_prs = sum(1 for p in prs if p["min_chosen"] <= 0.5 * p["min_today"])
    with open(os.path.join(d, "replay-runs.csv"), "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(run_rows[0]), lineterminator="\n")
        w.writeheader()
        w.writerows(run_rows)
    with open(os.path.join(d, "replay-prs.csv"), "w", newline="") as f:
        w = csv.writer(f, lineterminator="\n")
        w.writerow(
            [
                "pr",
                "runs",
                "jobs_today",
                "jobs_chosen",
                "job_minutes_today",
                "job_minutes_chosen",
                "wall_today",
                "wall_chosen",
            ]
        )
        for pr, p in sorted(per_pr.items(), key=lambda x: int(x[0])):
            w.writerow(
                [
                    pr,
                    p["runs"],
                    p["jobs_today"],
                    p["jobs_chosen"],
                    round(p["min_today"], 1),
                    round(p["min_chosen"], 1),
                    round(p["wall_today"], 1),
                    round(p["wall_chosen"], 1),
                ]
            )
    replay = {
        "slots": "node1:2+node2:1",
        "runs": len(rr),
        "prs": len(prs),
        "per_run": {
            "jobs_today": stats([x["jobs_today"] for x in rr]),
            "jobs_chosen": stats([x["jobs_chosen"] for x in rr]),
            "job_minutes_today": stats([x["minutes_today"] for x in rr]),
            "job_minutes_chosen": stats([x["minutes_chosen"] for x in rr]),
            "wall_today": stats([x["wall_today"] for x in rr]),
            "wall_chosen": stats([x["wall_chosen"] for x in rr]),
        },
        "per_pr": {
            "jobs_today": stats([p["jobs_today"] for p in prs]),
            "jobs_chosen": stats([p["jobs_chosen"] for p in prs]),
            "wall_today": stats([p["wall_today"] for p in prs]),
            "wall_chosen": stats([p["wall_chosen"] for p in prs]),
        },
        "runs_skipping_half_or_more_of_job_minutes": round(skip_most_runs / len(rr), 3),
        "runs_running_no_heavy_job": round(skip_all_runs / len(rr), 3),
        "prs_skipping_half_or_more_of_job_minutes": round(skip_most_prs / len(prs), 3),
        "runs_with_vm_or_browser_checks": len(vmshare),
        "of_them_skipping_half_or_more_of_vm_and_browser_minutes": round(
            sum(1 for x in vmshare if x <= 0.5) / len(vmshare), 3
        ),
        "of_them_skipping_90pct_or_more_of_vm_and_browser_minutes": round(
            sum(1 for x in vmshare if x <= 0.1) / len(vmshare), 3
        ),
        "distribution_share_of_minutes_kept": {
            b: sum(
                1
                for x in rr
                if lo
                <= (
                    x["minutes_chosen"] / x["minutes_today"]
                    if x["minutes_today"]
                    else 0
                )
                < hi
            )
            for b, lo, hi in (
                ("0", 0, 1e-9),
                ("0-10%", 1e-9, 0.1),
                ("10-25%", 0.1, 0.25),
                ("25-50%", 0.25, 0.5),
                ("50-75%", 0.5, 0.75),
                ("75-100%", 0.75, 1.01),
            )
        },
    }

    # ---- 5: structural options, each on its own, over PR runs
    def option(drop, name):
        saved = sum(r["m"] for r in pr_rows if drop(r))
        dt = {
            int(r["task"])
            for r in pr_rows
            if drop(r) and int(r["task"]) in caught_tasks
        }
        dinc = [
            f"run {fs[0]['run_no']} {fs[0]['job']}"
            for key, fs in inc_tasks.items()
            if key[0] == "pr" and all(f["task"] in dt for f in fs)
        ]
        return {
            "option": name,
            "minutes_saved": round(saved),
            "real_failures_deferred": len(dinc),
            "deferred": dinc,
        }

    inst = sets.get("installer")
    opts = []

    def run_files(r):
        return [f for f in (changes.get(r["run"]) or []) if not DOCS.match(f)]

    if inst:
        opts.append(
            option(
                lambda r: (
                    r["kind"] == "installer"
                    and changes.get(r["run"]) is not None
                    and not hits(inst, run_files(r))
                ),
                "installer jobs only when nix/installer/ or what the drive embeds changed",
            )
        )
    opts.append(
        option(
            lambda r: r["kind"] in ("systems", "agree"),
            "build_systems and systems_agree on main's full run only",
        )
    )
    opts.append(
        option(
            lambda r: r["kind"] == "installer", "installer jobs on main's full run only"
        )
    )
    # VM tests sharing one prefix: in a run, every VM job but the slowest pays
    # its evaluation and system builds again
    vm_prefix_saved = 0.0
    for run, rs in by_run.items():
        pre = []
        for r in rs:
            if r["kind"] != "vm" or not r["arg"]:
                continue
            ph = phases.get(r["task"])
            if ph and ph.get("built_test"):
                pre.append(ph["prefix"])
            elif r["status"] == "success":
                pre.append(r["m"])  # built nothing of its own: all prefix
        if len(pre) > 1:
            vm_prefix_saved += sum(pre) - max(pre)
    opts.append(
        {
            "option": "VM tests share one prefix (one evaluation and one build of the boxes, then each test's run)",
            "minutes_saved": round(vm_prefix_saved),
            "real_failures_deferred": 0,
            "deferred": [],
            "note": "upper bound: assumes the prefix (job start to the vm-test-run build) is fully shared",
        }
    )
    # expensive checks main-only: every job that caught nothing on a PR
    never = {
        t["job"]
        for t in table
        if t["real_failures"] == 0
        and t["minutes"] >= 60
        and not t["job"].startswith(("rust", "flake"))
    }
    opts.append(
        option(
            lambda r: r["job"] in never,
            "jobs that never caught a real failure (>= 60 min) on main's full run only",
        )
    )
    worst = {
        t["job"]
        for t in table
        if (t["minutes_per_real_failure"] or 1e9) >= 300 and t["minutes"] >= 100
    }
    opts.append(
        option(
            lambda r: r["job"] in worst,
            "jobs costing >= 300 min per real failure caught (or none) on main's full run only",
        )
    )
    opts.append(
        option(
            lambda r: (
                r["kind"] == "programs"
                and not any(
                    x["kind"] == "vm" and x["task"] in k_w.get(r["run"], set())
                    for x in by_run[r["run"]]
                )
                and r["task"] not in k_w.get(r["run"], set())
            ),
            "build_programs only when a chosen VM test needs it",
        )
    )
    opts.append(
        option(
            lambda r: (
                r["kind"] == "systems"
                and r["arg"].endswith(",node2")
                and r["arg"].split(",")[0] != "node2"
            ),
            "each box's system built once on a PR (on its own box), not on every runner",
        )
    )
    opts.append(
        option(
            lambda r: r["task"] not in k_w.get(r["run"], set()),
            "tests-this chooser (all jobs)",
        )
    )
    opts.append(
        option(
            lambda r: (
                r["task"] not in k_w.get(r["run"], set())
                or r["kind"] in ("systems", "agree", "installer")
            ),
            "tests-this chooser, with systems, agree and installer on main only",
        )
    )
    opts.sort(key=lambda o: -o["minutes_saved"])

    json.dump(
        {
            "totals": {
                "runs": len(runs),
                "jobs": len(rows),
                "job_minutes": round(sum(r["m"] for r in rows)),
                "pr_job_minutes": round(sum(r["m"] for r in pr_rows)),
                "real_failures": len(fails),
                "real_failure_incidents": len(incidents),
            },
            "per_job": table,
            "true_needed": needed,
            "true_needed_empirical": needed_empirical,
            "selectors_all_jobs": sel,
            "selectors_vm_and_browser": cmp_,
            "validation": validate,
            "kept_by_family": kept_breakdown,
            "replay": replay,
            "structural": opts,
        },
        sys.stdout,
        indent=1,
    )
    print()


if __name__ == "__main__":
    main()
