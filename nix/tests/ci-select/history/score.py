#!/usr/bin/env python3
# A test chooser scored against the history (history.py): for every pull
# request's run whose two commits git still has, which of the checks that
# ran (VM tests, browser and Worker checks) the chooser would have left
# out, the minutes that saves, and the real failures it would have missed.
#
#   score.py DIR [--interfaces] > score.json   (run from the repository)
import csv
import glob
import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import ciselect as cs  # noqa: E402
import interfaces  # noqa: E402

COVERS = cs.load_covers(open(cs.COVERS).read())
MIN = json.load(open(cs.MINUTES))


def choose(base, head, faces):
    changes = cs.name_status(base, head)

    def read():
        for f in cs.git("ls-tree", "-r", "--name-only", head).split("\n"):
            if f.endswith((".nix", ".rs")):
                yield f, cs.show(head, f)

    reg = cs.show(head, cs.REGISTRY) or cs.show(head, "flake.nix")
    checks = cs.traced_names(reg)
    fz = interfaces.stable(changes, base, head) if faces else None
    return cs.select(
        changes,
        cs.covers_at(head, checks, covers=COVERS),
        cs.covers_at(base, checks, covers=COVERS),
        reg,
        cs.named_docs(read),
        lambda w, d: cs.dir_exists(base if w == "base" else head, d),
        cs.crate_closure(cs.APP_ROOTS, lambda f: cs.show(head, f)),
        fz,
        MIN,
        (cs.show(base, cs.REGISTRY), cs.show(head, cs.REGISTRY))
        if cs.show(base, cs.REGISTRY)
        else None,
    )


def main():
    d = sys.argv[1]
    faces = "--interfaces" in sys.argv
    rows = [
        r
        for f in sorted(glob.glob(os.path.join(d, "history-jobs-*.csv")))
        for r in csv.DictReader(open(f))
    ]
    runs = {}
    for r in rows:
        if r["event"].startswith("pull_request") and r["base"]:
            runs.setdefault((r["run"], r["base"], r["sha"]), []).append(r)

    def one(key):
        run, base, head = key
        try:
            s = choose(base, head, faces)
        except Exception as e:  # noqa: BLE001
            return run, {"error": str(e)[:200]}
        if s["all"]:
            keep = None
        else:
            keep = set(s["vm_tests"]) | {t for g in cs.GROUPS for t in s[g]}
        return run, {
            "all": s["all"],
            "keep": sorted(keep) if keep is not None else None,
            "kept": s.get("kept", []),
        }

    with ThreadPoolExecutor(6) as ex:
        chosen = dict(ex.map(one, runs))
    saved = missed = flaky_skipped = 0.0
    misses = []
    scored = 0
    for (run, _, _), ts in runs.items():
        c = chosen.get(run, {})
        if "error" in c:
            continue
        scored += 1
        for t in ts:
            if (
                t["kind"] not in ("vm", "browser")
                or c["keep"] is None
                or t["arg"] in c["keep"]
            ):
                continue
            saved += float(t["min"])
            if t["class"] == "real":
                missed += 1
                misses.append(f"run {t['run_no']} {t['kind']}({t['arg']}) {t['why']}")
    json.dump(
        {
            "chooser": "interfaces" if faces else "covers",
            "runs_scored": scored,
            "errors": sum(1 for c in chosen.values() if "error" in c),
            "check_minutes_saved": round(saved),
            "real_failures_missed": int(missed),
            "missed": misses,
            "chosen": chosen,
        },
        sys.stdout,
        indent=1,
    )
    print()


if __name__ == "__main__":
    main()
