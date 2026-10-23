#!/usr/bin/env python3
# The selector replayed against main's history (docs/dev/ci.md, Choosing
# the tests): for each merged pull request, what ciselect.py picks, held to
# what actually changed - every check's drvPath at the base and at the head.
#
#   sim.py --revs revs.txt --truth DIR --covers covers-G0.json --times times.json
#          [--roots roots.json] [--trace-jobs N] [--out DIR]
#
# revs.txt: main's commits, newest first (each one pull request; main moves
# only fast-forward). DIR/<sha>.json: {check: drvPath} for that commit (one
# `nix eval --json` of checks.x86_64-linux mapped to drvPath). covers-G0:
# covers.json traced at the oldest commit. The replay keeps covers.json as
# CI would: a check CI runs is traced at the head, and where it read
# something covers.json lacked, that pull request would have failed until
# its author traced the check again (a "gap", counted), which the replay
# then does. roots.json, when given: for a skipped check whose drvPath
# moved, the derivations that changed first (sim-roots.sh on the box that
# holds the .drv files), to tell a pages-only move from a real one.
import json
import os
import re
import sys
import time
from concurrent.futures import ThreadPoolExecutor

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ciselect as cs  # noqa: E402

RUST_LINT = {"fmt", "clippy"}
TEST = {"tests"}
APP_LINT = {"clippy-app", "tests-app", "net-check"}


def job_of(check, vms):
    if check in vms:
        return "vm_tests"
    for g, ts in cs.GROUPS.items():
        if check in ts:
            return g
    if check in RUST_LINT:
        return "rust_lint"
    if check in TEST:
        return "test"
    if check in APP_LINT:
        return "app_checks"
    return "flake_check"


def old_select(changes):
    # files_modified before this change: four switches from path regexes

    paths = [p for _, p in changes]

    def any_(rx):
        return any(re.match(rx, p) for p in paths)

    nix = any_(
        r"^(flake\.nix|flake\.lock|nix/|fleet/|secrets/|data/|docs/|servers/|client/|Cargo\.(toml|lock|nix)|crate-hashes\.json|\.forgejo/|\.github/)"
    )
    rust = any_(
        r"^(servers/|client/|Cargo\.(toml|lock|nix)|crate-hashes\.json|flake\.(nix|lock)|\.forgejo/)"
    )
    vm = any_(
        r"^(flake\.nix|flake\.lock|nix/|fleet/|data/|servers/|client/(identity|auth)/|Cargo\.(toml|lock|nix)|crate-hashes\.json|\.forgejo/)"
    )
    app = any_(
        r"^(app/|client/|Cargo\.(toml|lock|nix)|crate-hashes\.json|flake\.(nix|lock)|nix/app-release/|\.forgejo/)"
    )
    return {"nix": nix, "rust": rust, "vm": nix and vm, "app": app}


def main():
    a = sys.argv[1:]
    opt = {}
    while a:
        k = a.pop(0).lstrip("-")
        opt[k] = a.pop(0)
    revs = [r.strip() for r in open(opt["revs"]) if r.strip()]
    loaded = {}

    def truth(r):
        # waits for a commit's drvPaths still being evaluated elsewhere
        f = os.path.join(opt["truth"], r + ".json")
        while r not in loaded:
            if os.path.exists(f):
                loaded[r] = json.load(open(f))
            else:
                time.sleep(20)  # sleep-ok: a file another box is writing
        return loaded[r]

    times = json.load(open(opt["times"]))
    roots = json.load(open(opt["roots"])) if "roots" in opt else {}
    out = opt.get("out", ".")
    jobs = int(opt.get("trace-jobs", "8"))
    covers = cs.load_covers(open(opt["covers"]).read())
    # --narrow yes: only CI itself and the chooser count as under
    # everything; flake.nix, the lockfile, nix/lib and the shared crates go
    # by what each check reads, like any other path
    if opt.get("narrow") == "yes":
        cs.FOUNDATION = re.compile(r"^(\.forgejo/|nix/tests/ci-select/[^/]*\.py$)")
    # --covers-end: covers.json traced at the newest commit too; the replay
    # then uses the two together throughout and traces nothing (fast, and
    # what it would miss shows up as a false negative against the truth)
    # --trace no: covers.json as traced at the oldest commit, never again,
    # with what each commit's .nix files name (as CI has it)
    fixed = "covers-end" in opt or opt.get("trace") == "no"
    if "covers-end" in opt:
        end = cs.load_covers(open(opt["covers-end"]).read())
        for k in ("dirs", "files", "listed"):
            for p, ts in end[k].items():
                covers[k].setdefault(p, set()).update(ts)
        covers["checks"] |= end["checks"]
    rows, fns, pages_skips, gaps, needs_roots = [], [], [], [], []
    for i in range(len(revs) - 2, -1, -1):
        base, head = revs[i + 1], revs[i]
        subject = cs.git("log", "-1", "--format=%s", head).strip()
        changes = cs.name_status(base, head)
        flake = cs.show(head, "flake.nix")
        vms = cs.vm_tests(flake) or []

        def read():
            for f in cs.git("ls-tree", "-r", "--name-only", head).split("\n"):
                if f.endswith((".nix", ".rs")):
                    yield f, cs.show(head, f)

        r = cs.select(
            changes,
            cs.covers_at(head, cs.traced_names(flake), covers=covers),
            cs.covers_at(base, cs.traced_names(flake), covers=covers),
            flake,
            cs.named_docs(read),
            lambda which, d: cs.dir_exists(base if which == "base" else head, d),
            cs.crate_closure(cs.APP_ROOTS, lambda f: cs.show(head, f)),
        )
        old = old_select(changes)
        tb, th = truth(base), truth(head)
        moved = {c for c in th if th.get(c) != tb.get(c)}
        ran_new = {
            "vm_tests": set(r.get("vm_tests", [])),
            **{g: set(r.get(g, [])) for g in cs.GROUPS},
            "flake_check": r["nix"],
            "rust_lint": r["rust"],
            "test": r["rust"],
            "app_checks": r["app"],
        }
        browser = set().union(*(ran_new[g] for g in cs.GROUPS))
        ran_old = {
            "vm_tests": old["vm"],
            **{g: old["nix"] for g in cs.GROUPS},
            "flake_check": old["nix"],
            "rust_lint": old["rust"],
            "test": old["rust"],
            "app_checks": old["app"],
        }
        old_missed = sorted(
            c for c in moved if th.get(c) is not None and not ran_old[job_of(c, vms)]
        )
        for c in sorted(moved):
            if th.get(c) is None:
                continue
            j = job_of(c, vms)
            ran = c in ran_new[j] if isinstance(ran_new[j], set) else ran_new[j]
            if ran:
                continue
            key = f"{base[:9]}..{head[:9]}:{c}"
            kind = roots.get(key)
            row = {
                "pr": head[:9],
                "subject": subject[:70],
                "check": c,
                "job": j,
                "roots": kind,
            }
            if kind is None:
                needs_roots.append((base, head, c, tb.get(c), th.get(c)))
            if kind and all(x in ("dd-pages",) for x in kind):
                pages_skips.append(row)
            else:
                fns.append(row)
        # CI traces every check it runs, at the head
        shape = [
            (st, p)
            for st, p in changes
            if st != "M"
            or p.endswith((".nix", ".json", ".toml", ".lock", ".yaml", ".yml"))
        ]
        reach = set()
        for st, p in shape:
            reach |= cs.touching(
                covers,
                st,
                p,
                lambda d: cs.dir_exists(base, d),
                lambda d: cs.dir_exists(head, d),
            )
        # Only a change to what decides which files an evaluation reads (a
        # file added or removed, a .nix, .json, .toml, .lock or .yaml) can
        # change a check's covers, and only for a check that read it; the
        # replay traces those. CI traces every check it runs regardless.
        traced_now = [
            t
            for t in sorted(ran_new["vm_tests"] | browser)
            if not fixed
            and th.get(t) is not None
            and (t in reach or t not in covers["checks"])
        ]
        src = cs.store_copy(head) if traced_now else None
        with ThreadPoolExecutor(jobs) as ex:
            trs = dict(
                zip(traced_now, ex.map(lambda t: cs.trace_one(src, t), traced_now))
            )
        for t, tr in trs.items():
            if not tr["ok"]:
                gaps.append(
                    {"pr": head[:9], "check": t, "missing": ["evaluation failed"]}
                )
                continue
            m = cs.missing(covers, t, tr) if t in covers["checks"] else ["(new check)"]
            if m:
                gaps.append(
                    {"pr": head[:9], "check": t, "missing": m[:10], "n": len(m)}
                )
                for k in ("dirs", "files", "listed"):
                    for p in list(covers[k]):
                        covers[k][p].discard(t)
                cs.add_trace(covers, t, tr)

        # what each job would have cost, by the timing study's medians
        def cost(job, check=None, built=True):
            if job == "vm_tests":
                return times["vm"].get(check, {"built": 3.0, "cached": 0.25})[
                    "built" if built else "cached"
                ]
            return times["job"][job]

        traced = vms + cs.BROWSER
        old_min = new_min = 0.0
        old_vm = vms if old["vm"] else []
        for t in vms:
            if t in old_vm:
                old_min += cost("vm_tests", t, t in moved)
            if t in ran_new["vm_tests"]:
                new_min += cost("vm_tests", t, t in moved)
        for job, o, n in [
            ("flake_check", old["nix"], r["nix"]),
            ("build_all", old["nix"], r["nix"]),
            # the browser and Worker jobs, timed together as mail_worker
            # before they were split
            ("mail_worker", old["nix"], bool(browser)),
            ("rust_lint", old["rust"], r["rust"]),
            ("test", old["rust"], r["rust"]),
            ("app_lint", old["app"], r["app"]),
        ]:
            old_min += cost(job) if o else 0
            new_min += cost(job) if n else 0
        if browser and old["nix"]:
            # they build only their chosen checks: by their share
            share = len(browser) / len(cs.BROWSER)
            new_min -= cost("mail_worker") * (1 - share)
        rows.append(
            {
                "pr": head[:9],
                "subject": subject[:70],
                "files": len(changes),
                "class": "docs only"
                if r["docs_only"]
                else ("everything: " + r["reason"] if r["all"] else "chosen"),
                "old_vm": len(old_vm),
                "new_vm": len(ran_new["vm_tests"]),
                "moved_vm": len([t for t in vms if t in moved]),
                "new_mail": len(browser),
                "moved_mail": len([t for t in cs.BROWSER if t in moved]),
                "old_min": round(old_min, 1),
                "new_min": round(new_min, 1),
                "traced": len(traced_now),
                "old_missed": old_missed,
                "of": len(traced),
            }
        )
        print(
            f"{head[:9]} {rows[-1]['class'][:40]:40} vm {rows[-1]['old_vm']}->{rows[-1]['new_vm']} (moved {rows[-1]['moved_vm']}) min {old_min:.1f}->{new_min:.1f}",
            flush=True,
        )
    os.makedirs(out, exist_ok=True)
    json.dump(
        {
            "rows": rows,
            "false_negatives": fns,
            "pages_only_skips": pages_skips,
            "gaps": gaps,
            "needs_roots": needs_roots,
        },
        open(os.path.join(out, "sim.json"), "w"),
        indent=1,
    )
    print(
        f"\nfalse negatives: {len(fns)}  pages-only skips: {len(pages_skips)}  gaps: {len(gaps)}  unclassified: {len(needs_roots)}"
    )
    tot_old = sum(r["old_min"] for r in rows)
    tot_new = sum(r["new_min"] for r in rows)
    print(
        f"job-minutes: {tot_old:.0f} -> {tot_new:.0f} ({100 * (1 - tot_new / tot_old):.0f}% saved)"
        if tot_old
        else ""
    )
    return 1 if fns else 0


if __name__ == "__main__":
    sys.exit(main())
