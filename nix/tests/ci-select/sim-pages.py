#!/usr/bin/env python3
# For sim.py: is a skipped check's moved drvPath the pages' and docs' doing
# alone? The base's tree with only the head's page and docs files laid on
# it is evaluated; if the check comes out as at the head, nothing but pages
# and docs moved it.
#   sim-pages.py sim.json > pages.json   (from a checkout holding the commits)
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ciselect as cs  # noqa: E402

# what dd-pages is made of (flake.nix pages), and the docs
PAGES = re.compile(
    r"^(servers/([^/]+/)?web(/|$)|servers/web-shared/|docs/|data/the-joke/)|^[^/]+\.md$"
)


def main():
    sim = json.load(open(sys.argv[1]))
    by_pr = {}
    for base, head, check, _, want in sim["needs_roots"]:
        by_pr.setdefault((base, head), []).append((check, want))
    out = {}
    for (base, head), checks in by_pr.items():
        tmp = tempfile.mkdtemp(prefix="ciselect-pages.")
        try:
            a = subprocess.Popen(["git", "archive", base], stdout=subprocess.PIPE)
            subprocess.run(["tar", "-x", "-C", tmp], stdin=a.stdout, check=True)
            a.wait()
            moved = []
            for st, p in cs.name_status(base, head):
                if not PAGES.match(p):
                    continue
                moved.append(p)
                dst = os.path.join(tmp, p)
                if st == "D":
                    os.remove(dst)
                    continue
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                with open(dst, "wb") as f:
                    f.write(
                        subprocess.run(
                            ["git", "show", f"{head}:{p}"],
                            capture_output=True,
                            check=True,
                        ).stdout
                    )
            src = subprocess.run(
                ["nix", "store", "add", "--name", "source", tmp],
                capture_output=True,
                text=True,
                check=True,
            ).stdout.strip()
        finally:
            shutil.rmtree(tmp)
        for check, want in checks:
            got = subprocess.run(
                [
                    "nix",
                    "eval",
                    "--raw",
                    f"path:{src}#checks.x86_64-linux.{check}.drvPath",
                ],
                capture_output=True,
                text=True,
            ).stdout.strip()
            key = f"{base[:9]}..{head[:9]}:{check}"
            out[key] = {"pages_only": got == want, "page_files": len(moved)}
            print(key, got == want, len(moved), file=sys.stderr, flush=True)
    json.dump(out, sys.stdout, indent=1)


if __name__ == "__main__":
    main()
