#!/usr/bin/env python3
# Each pull request run's faces, as files_modified reads them: for every run
# whose change touched the gate's or the control plane's insides
# (interfaces.py), whether that component's face held from the merge base to
# the run's commit. The code lived elsewhere before the tree moved
# (box/verify, box/control); each end is read where it was, under today's
# name, so every run is judged by the same readers.
#
#   faces.py DIR > faces.json   (run from the repository, with apidiff, goface
#                                and go on PATH; Go modules from the module
#                                cache: FACES_GO_MODULES=mod)
import csv
import json
import os
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.dirname(HERE))
import interfaces  # noqa: E402

# where each component's face directory was, newest first
WHERE = {
    "servers/gate/src": ["servers/gate/src", "box/verify/src"],
    "servers/system/control": ["servers/system/control", "box/control"],
}


def git(*a):
    return subprocess.run(["git", *a], capture_output=True, text=True).stdout


def exists(rev, d):
    r = subprocess.run(["git", "cat-file", "-t", f"{rev}:{d}"], capture_output=True)
    return r.returncode == 0


def tree(rev, dirs):
    """dirs as they were at rev, each under today's name"""
    tmp = tempfile.mkdtemp(prefix="faces.")
    for d in dirs:
        was = next((w for w in WHERE.get(d, [d]) if exists(rev, w)), None)
        if was is None:
            continue
        a = subprocess.Popen(["git", "archive", rev, "--", was], stdout=subprocess.PIPE)
        subprocess.run(["tar", "-x", "-C", tmp], stdin=a.stdout, check=True)
        a.wait()
        if was != d:
            os.makedirs(os.path.dirname(os.path.join(tmp, d)), exist_ok=True)
            shutil.move(os.path.join(tmp, was), os.path.join(tmp, d))
    return tmp


def judge(job):
    run, name, base, head = job
    dirs = sorted({f[1] for f in interfaces.COMPONENTS[name]["faces"]})
    b = h = None
    try:
        b, h = tree(base, dirs), tree(head, dirs)
        return run, name, interfaces.differences(name, b, h, None)
    except (RuntimeError, OSError, ValueError, subprocess.CalledProcessError) as e:
        return run, name, [f"its face could not be read: {str(e)[:300]}"]
    finally:
        for t in (b, h):
            if t:
                shutil.rmtree(t, ignore_errors=True)


def main():
    d = sys.argv[1]
    runs = {r["id"]: r for r in csv.DictReader(open(os.path.join(d, "runs.csv")))}
    base = {
        b["id"]: b["base"] for b in csv.DictReader(open(os.path.join(d, "base.csv")))
    }
    changes = json.load(open(os.path.join(d, "changes.json")))["runs"]
    jobs = []
    for run, files in sorted(changes.items(), key=lambda x: int(x[0])):
        touched = sorted({interfaces.inside_of(f) for f in files} - {None})
        if not touched:
            continue
        mb = git("merge-base", base[run], runs[run]["commit_sha"]).strip()
        for name in touched:
            jobs.append((run, name, mb, runs[run]["commit_sha"]))
    out = {}
    with ThreadPoolExecutor(4) as ex:
        for i, (run, name, diff) in enumerate(ex.map(judge, jobs)):
            out.setdefault(run, {})[name] = diff
            print(
                f"{i + 1}/{len(jobs)} run {run} {name}: {'held' if not diff else diff[0][:80]}",
                file=sys.stderr,
            )
    json.dump(out, sys.stdout, indent=1, sort_keys=True)
    print()


if __name__ == "__main__":
    main()
