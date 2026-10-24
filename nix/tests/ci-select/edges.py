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
    flake = open(os.path.join(root, cs.REGISTRY)).read()
    every = set(cs.traced_names(flake))
    # a check covers.json has not traced yet runs on every change until it
    # is; the cases judge the traced ones, and that it ran
    untraced = every - covers["checks"]
    # the group jobs as the cases judge them: of the traced checks only
    groups = {g: [t for t in ts if t not in untraced] for g, ts in cs.GROUPS.items()}

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

    def choose(
        changes, new_dirs=(), gone_dirs=(), base_covers=None, faces=None, reg=None
    ):
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
            reg[1] if reg else flake,
            docs_named,
            exists,
            cs.crate_closure(
                cs.APP_ROOTS, lambda f: open(os.path.join(root, f)).read()
            ),
            faces,
            None,
            reg,
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
    # the checks that read the gate's code: what a change to its face reaches
    gate_readers = {
        t
        for d, ts in stat(covers)["dirs"].items()
        if d == "servers/gate/src" or d.startswith("servers/gate/src/")
        for t in ts
    }
    gate_own = set(cs.interfaces.COMPONENTS["gate"]["checks"])
    gate_rs = [("M", "servers/gate/src/joke.rs")]
    added = flake.replace("  vm = [\n", '  vm = [\n    # a new one\n    "newtest"\n', 1)
    reshaped = flake.replace("  eval = [", "  later = [ ];\n  eval = [", 1)
    cases = [
        (
            "a test registered, with its files",
            [
                ("M", cs.REGISTRY),
                ("A", "nix/tests/newtest.nix"),
                ("A", "nix/tests/newtest.py"),
            ],
            lambda r: (
                not r["all"]
                and r.get("registered") == ["newtest"]
                and r["vm_tests"] == ["newtest"]
                and "console" not in r["chosen"]
                and r["eval"]
            ),
            [],
            [],
            None,
            None,
            (flake, added),
        ),
        (
            "the registry changed beyond its names",
            [("M", cs.REGISTRY)],
            lambda r: r["all"],
            [],
            [],
            None,
            None,
            (flake, reshaped),
        ),
        (
            "a gate-internal change with an unchanged interface",
            gate_rs,
            lambda r: (
                set(r["chosen"]) == gate_own
                and not r["vm_tests"]
                and r["rust"]
                and not r["eval"]
                and not r["programs"]
                and not r["all"]
                and r["kept"] == ["gate"]
            ),
            [],
            [],
            None,
            {"gate": []},
        ),
        (
            "a gate route-schema change runs what reads the gate",
            gate_rs,
            lambda r: (
                set(r["chosen"]) == gate_readers
                and r["vm_tests"]
                and "console" not in r["chosen"]
                and not r["all"]
                and r["kept"] == []
            ),
            [],
            [],
            None,
            {"gate": ["rust-routes servers/gate/src: the face changed"]},
        ),
        (
            "a control-internal change with an unchanged interface",
            [("M", "servers/system/control/place/place.go")],
            lambda r: (
                r["chosen"] == ["control"]
                and not r["vm_tests"]
                and not r["eval"]
                and not r["rust"]
            ),
            [],
            [],
            None,
            {"control": []},
        ),
        (
            "the gate's face held, a module beside it changed",
            gate_rs + [("M", "nix/modules/forge/forgejo.nix")],
            lambda r: (
                "forge" in r["chosen"]
                and gate_own <= set(r["chosen"])
                and "console" not in r["chosen"]
                and r["eval"]
            ),
            [],
            [],
            None,
            {"gate": []},
        ),
        (
            "the face readers (part of the chooser)",
            [("M", "nix/tests/ci-select/faces/rust_routes.py")],
            lambda r: r["all"],
        ),
        (
            "the interface map (part of the chooser)",
            [("M", "nix/tests/ci-select/interfaces.py")],
            lambda r: r["all"],
            [],
            [],
            None,
            {},
        ),
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
            lambda r: (
                r["all"]
                and r["installer"]
                and all(r[g] == ts for g, ts in groups.items())
            ),
        ),
        (
            "docs alone",
            [("M", "docs/dev/ci.md"), ("A", "docs/guide/new.md"), ("M", "README.md")],
            lambda r: (
                r["docs_only"]
                and not (r["nix"] or r["rust"] or r["app"] or r["installer"])
                and not r["vm_tests"]
                and not any(r[g] for g in cs.GROUPS)
            ),
        ),
        (
            "a page script the app carries",
            [("M", "app/web/_dd/static/calendar.js")],
            lambda r: (
                r["page_tests"] == groups["page_tests"]
                and r["site_checks"] == ["site-links"]
                and not r["worker_tests"]
                and set(r["vm_tests"]) <= {"site-journeys"}
                and not r["installer"]
                and not r["all"]
            ),
        ),
        (
            "a box page's script",
            [("M", "servers/mail/web/mail-search.js")],
            lambda r: (
                set(r["worker_tests"]) <= {"mail-worker", "sso"}
                and not r["page_tests"]
                and r["site_checks"]
                and not r["installer"]
                and not r["all"]
            ),
        ),
        (
            "a Worker's source",
            [("M", "client/mail/worker/worker.js")],
            lambda r: (
                "mail-worker" in r["worker_tests"]
                and "control" not in r["worker_tests"]
                and not r["page_tests"]
                and not r["site_checks"]
                and not r["installer"]
            ),
        ),
        (
            "the installer's own module",
            [("M", "nix/installer/setup.nix")],
            lambda r: (
                r["installer"]
                and r["nix"]
                and not r["all"]
                and not any(r[g] for g in cs.GROUPS)
                and set(r["chosen"]) <= {"installer", "join-install"}
            ),
        ),
        (
            "the installer's screen",
            [("M", "nix/installer/web/setup.js")],
            lambda r: r["installer"] and not r["all"],
        ),
        (
            "the program the installer carries (the agent's crate)",
            [("M", "servers/system/release/src/bin/dd-attest.rs")],
            lambda r: r["installer"] and not r["all"],
        ),
        (
            "a box module the installer does not embed",
            [("M", "nix/modules/forge/forgejo.nix")],
            lambda r: not r["installer"] and not r["all"],
        ),
        (
            "the chooser itself",
            [("M", "nix/tests/ci-select/ciselect.py")],
            lambda r: (
                r["all"]
                and r["installer"]
                and all(r[g] == ts for g, ts in groups.items())
            ),
        ),
        (
            "flake.lock bumped (the installer's nixpkgs)",
            [("M", "flake.lock")],
            lambda r: r["all"] and r["installer"],
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
        base = lacking(case[5]) if len(case) > 5 and case[5] else None
        faces = case[6] if len(case) > 6 else None
        reg = case[7] if len(case) > 7 else None
        r = choose(changes, set(new_dirs), set(gone), base, faces, reg)
        seen = dict(
            r,
            chosen=[t for t in r["chosen"] if t not in untraced],
            vm_tests=[t for t in r["vm_tests"] if t not in untraced],
            **{g: [t for t in r[g] if t not in untraced] for g in cs.GROUPS},
        )
        good = bool(ok(seen)) and (r["docs_only"] or untraced <= set(r["chosen"]))
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
    return rows, bad, sorted(untraced)


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    rows, bad, untraced = run(root)
    if untraced:
        print(
            f"not in covers.json yet, so on every change: {' '.join(untraced)}\n"
            f"  trace it on a box: nix/tests/ci-select/ciselect.py trace --only {','.join(untraced)}"
        )
    for title, changes, kind, chosen, good, r in rows:
        flags = " ".join(k for k in ("nix", "rust", "app", "installer") if r.get(k))
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
