#!/usr/bin/env python3
# The chooser on made-up changes, against this tree's covers.json: a new
# directory, a rename, the test helpers, a lockfile bump, a test's own
# file, docs alone, docs with code, a page, a path nothing claims. The
# ci-select check runs it; `edges.py ROOT --table` prints what it chose.
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ciselect as cs  # noqa: E402


def run(root):
    covers = cs.load_covers(open(os.path.join(root, cs.COVERS)).read())
    flake = open(os.path.join(root, "flake.nix")).read()
    vms = cs.vm_tests(flake)
    every = set(vms + cs.MAIL_WORKER)

    def named():
        for d, _, fs in os.walk(root):
            for f in fs:
                if f.endswith((".nix", ".rs")):
                    p = os.path.join(d, f)
                    yield os.path.relpath(p, root), open(p, errors="replace").read()

    docs_named = cs.named_docs(named)

    def stat(c):
        return cs.add_static(
            c,
            sorted(every),
            lambda f: (
                open(os.path.join(root, f), errors="replace").read()
                if os.path.isfile(os.path.join(root, f))
                else None
            ),
            lambda d: os.path.isdir(os.path.join(root, d)),
        )

    def choose(changes, new_dirs=(), gone_dirs=(), base_covers=None):
        def exists(which, d):
            if which == "base" and d in new_dirs:
                return False
            if which == "head" and d in gone_dirs:
                return False
            return d == "" or os.path.isdir(os.path.join(root, d)) or d in new_dirs

        return cs.select(
            changes,
            stat(covers),
            stat(base_covers or covers),
            flake,
            docs_named,
            exists,
            cs.crate_closure(
                cs.APP_ROOTS, lambda f: open(os.path.join(root, f)).read()
            ),
        )

    def lacking(t):
        # covers.json without one check's entries: as if a pull request
        # had added them
        c = cs.load_covers(open(os.path.join(root, cs.COVERS)).read())
        for k in ("dirs", "files", "listed"):
            for p in c[k]:
                c[k][p].discard(t)
        return c

    pages = {
        t
        for t in every
        if any(
            t in ts
            for d, ts in stat(covers)["dirs"].items()
            if d.startswith("servers/web-shared")
        )
    }
    cases = [
        (
            "a test's own file",
            [("M", "nix/tests/acme.py")],
            lambda r: r["chosen"] == ["acme"],
        ),
        (
            "a new test helper file",
            [("A", "nix/tests/lib/later.py")],
            lambda r: (
                0 < len(r["chosen"]) < len(every) and "console" not in r["chosen"]
            ),
        ),
        (
            "a test helper changed",
            [("M", "nix/tests/lib/waits.py")],
            lambda r: (
                0 < len(r["chosen"]) < len(every) and "console" not in r["chosen"]
            ),
        ),
        (
            "a renamed module",
            [
                ("D", "nix/modules/forge/live.nix"),
                ("A", "nix/modules/forge/live-pages.nix"),
            ],
            lambda r: "forge" in r["chosen"] and "console" not in r["chosen"],
        ),
        (
            "a new directory in a crate",
            [("A", "servers/games/src/catalogue/mod.rs")],
            lambda r: "games" in r["chosen"] and "console" not in r["chosen"],
            ["servers/games/src/catalogue"],
        ),
        (
            "a new crate",
            [("A", "servers/newsvc/Cargo.toml"), ("A", "servers/newsvc/src/main.rs")],
            lambda r: len(r["chosen"]) >= len(every) // 2,
            ["servers/newsvc", "servers/newsvc/src"],
        ),
        (
            "a new top-level directory",
            [("A", "tools/run.sh")],
            lambda r: r["all"],
            ["tools"],
        ),
        ("a path nothing claims", [("M", "scripts/sync-app-web")], lambda r: r["all"]),
        (
            "a crate the app builds on",
            [("M", "servers/system/release/src/bin/dd-attest.rs")],
            lambda r: r["app"] and r["rust"] and not r["all"],
        ),
        ("Cargo.lock bumped", [("M", "Cargo.lock")], lambda r: r["all"]),
        ("nix/lib changed", [("M", "nix/lib/script.nix")], lambda r: r["all"]),
        (
            "the workflow changed",
            [("M", ".forgejo/workflows/entry_point.yml")],
            lambda r: r["all"],
        ),
        (
            "docs alone",
            [("M", "docs/dev/ci.md"), ("A", "docs/guide/new.md"), ("M", "README.md")],
            lambda r: r["docs_only"] and not r["nix"],
        ),
        (
            "the terms (a check reads them by name)",
            [("M", "docs/policies/terms.md")],
            lambda r: not r["docs_only"] and r["nix"] and not r["all"],
        ),
        (
            "docs with code",
            [("M", "docs/dev/games.md"), ("M", "servers/games/src/main.rs")],
            lambda r: (
                "games" in r["chosen"] and "console" not in r["chosen"] and r["rust"]
            ),
        ),
        (
            "a page alone",
            [("M", "servers/web-shared/home.css")],
            lambda r: (
                r["chosen"]
                and set(r["chosen"]) <= pages
                and r["nix"]
                and "console" not in r["chosen"]
            ),
        ),
        (
            "a page removed",
            [("D", "servers/web-shared/home.css")],
            lambda r: r["chosen"] and set(r["chosen"]) <= pages,
        ),
        (
            "a check new to covers.json",
            [("M", "nix/tests/acme.py")],
            lambda r: "acme" in r["chosen"] and "forge" in r["chosen"],
            [],
            [],
            "forge",
        ),
    ]
    rows, bad = [], []
    for case in cases:
        title, changes, ok = case[:3]
        new_dirs = case[3] if len(case) > 3 else []
        gone = case[4] if len(case) > 4 else []
        base = lacking(case[5]) if len(case) > 5 else None
        r = choose(changes, set(new_dirs), set(gone), base)
        good = bool(ok(r))
        if not good:
            bad.append(title)
        kind = (
            "docs only"
            if r["docs_only"]
            else (
                "everything (" + r["reason"] + ")"
                if r["all"]
                else f"{len(r['chosen'])} of {len(every)}"
            )
        )
        rows.append(
            (
                title,
                " ".join(f"{s} {p}" for s, p in changes),
                kind,
                r["chosen"] if not r["all"] else [],
                good,
                r,
            )
        )
    return rows, bad


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    rows, bad = run(root)
    for title, changes, kind, chosen, good, r in rows:
        flags = " ".join(k for k in ("nix", "rust", "app") if r.get(k))
        print(
            f"{'ok  ' if good else 'FAIL'} {title}: {changes}\n     -> {kind}; jobs: {flags or 'style, lint'}"
        )
        if chosen and not r["all"]:
            print(f"        {' '.join(chosen)}")
    if "--json" in sys.argv:
        json.dump(
            [
                {"case": t, "changes": c, "chose": k, "tests": ch, "ok": g}
                for t, c, k, ch, g, _ in rows
            ],
            open(sys.argv[sys.argv.index("--json") + 1], "w"),
            indent=1,
        )
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
