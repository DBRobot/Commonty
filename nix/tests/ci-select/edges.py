#!/usr/bin/env python3
# The selector (plan.py) on made-up changes against this tree's
# tests-this.json, and the sets themselves held to the tree: every
# registered check has a set, every path a set names is in the tree, every
# job main-only.json and a component's own checks name is one. The
# ci-select check runs it; `edges.py ROOT` prints what each case chose.
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import ciselect  # noqa: E402
import interfaces  # noqa: E402
import plan  # noqa: E402


def tree(root):
    out = []
    for d, ds, fs in os.walk(root):
        ds[:] = [x for x in ds if x not in (".git", "target", "node_modules", "result")]
        for f in fs:
            out.append(os.path.relpath(os.path.join(d, f), root))
    return sorted(out)


def run(root):
    def read(f):
        p = os.path.join(root, f)
        return open(p, errors="replace").read() if os.path.isfile(p) else ""

    files = tree(root)
    raw = json.loads(read(plan.SETS))
    sets = plan.Sets(raw)
    reg_text = read(plan.REGISTRY)
    reg = ciselect.registry(reg_text)
    main_only = plan.load_main_only(read(plan.MAIN_ONLY))
    crates = ciselect.crates(read)
    hosts = ciselect.hosts(read, sets)
    docs_named = ciselect.named_docs(
        lambda: ((f, read(f)) for f in files if f.endswith((".nix", ".rs")))
    )
    problems = []

    # ---- the sets, held to the tree
    registered = set(reg.get("vm", [])) | set(reg.get("eval", [])) | set(plan.GROUPED)
    for n in sorted(registered):
        if not sets.has(n):
            problems.append(f"{n} is registered but has no set in tests-this.json")
    for n in sorted(set(raw.get("_vm", [])) ^ set(reg.get("vm", []))):
        problems.append(f"{n}: tests-this.json's _vm and the registry's vm differ")
    for path, name in crates.items():
        if not sets.has(f"rust:{path}"):
            problems.append(f"crate {name} ({path}) has no rust:{path} set")
    for k in [
        plan.RUST,
        plan.APP,
        plan.APP_EVAL,
        plan.FLAKE,
        plan.KEEP,
        plan.INSTALLER,
    ]:
        if not sets.has(k):
            problems.append(f"the {k} job has no set")
    for h, k in hosts.items():
        if not sets.has(k):
            problems.append(f"{h}'s system ({k}) has no set")
    for k in main_only:
        if not sets.has(k):
            problems.append(f"main-only.json names {k}, which is no job")
    for n, c in interfaces.COMPONENTS.items():
        for k in c["own"]:
            if not sets.has(k):
                problems.append(f"{n}'s own check {k} has no set")
    shared = raw.get("_shared_by_family", {})
    for k, v in raw.items():
        if k.startswith("_"):
            continue
        for g in list(v["paths"]) + list(shared.get(v.get("family", ""), [])):
            r = plan.rx([g])
            if not any(r.match(f) for f in files):
                problems.append(f"{k}: {g} is in no file of the tree")

    # ---- the cases
    def choose(changes, faces=None, registry_base=None, mo=None):
        return plan.plan(
            changes,
            sets,
            reg,
            main_only=main_only if mo is None else mo,
            faces=faces,
            docs_named=docs_named,
            registry_base=registry_base,
            crates=crates,
            hosts=hosts,
        )

    # the VM tests a pull request can run: main-only.json's are main's
    vm = set(reg.get("vm", [])) - main_only
    gate_own = set(interfaces.COMPONENTS["gate"]["own"])
    control_own = set(interfaces.COMPONENTS["control"]["own"])
    gate_readers = {t for t in vm if sets.hits(t, ["servers/gate/src/lib.rs"])}
    added = reg_text.replace(
        "  vm = [\n", '  vm = [\n    # a new one\n    "newtest"\n', 1
    )
    reshaped = reg_text.replace("  eval = [", "  later = [ ];\n  eval = [", 1)
    any_vm = "acme"
    cases = [
        (
            "docs alone",
            [("M", "docs/dev/ci.md"), ("A", "docs/guide/new.md")],
            {},
            lambda r: r["docs_only"] and not r["why"] and not r["flake"],
        ),
        (
            "docs with code",
            [("M", "docs/dev/ci.md"), ("M", f"nix/tests/{any_vm}.py")],
            {},
            lambda r: not r["docs_only"] and any_vm in r["vm_tests"],
        ),
        (
            "the terms (a check reads them by name)",
            [("M", "docs/policies/terms.md")],
            {},
            lambda r: "terms-version" in r["why"] and not r["docs_only"],
        ),
        (
            "a test's own script",
            [("M", "nix/tests/acme.py")],
            {},
            lambda r: r["vm_tests"] == ["acme"] and r["programs"] and not r["rust"],
        ),
        (
            "the VM tests' shared helpers",
            [("M", "nix/tests/lib/waits.py")],
            {},
            lambda r: set(r["vm_tests"]) == vm and not r["page_tests"],
        ),
        (
            "flake.lock bumped: every job a pull request runs",
            [("M", "flake.lock")],
            {},
            lambda r: (
                r["global"]
                and set(r["vm_tests"]) == vm
                and r["rust_crates"] == sorted(crates.values())
                and all(k in r["main_only"] for k in main_only)
                and r["systems_eval"]
                and all(r[g] == ts for g, ts in plan.GROUPS.items())
                and all(k not in r["why"] for k in main_only)
                and not r["installer"]
            ),
        ),
        (
            "a gate-internal change with its face held: its own checks",
            [("M", "servers/gate/src/directory.rs")],
            {"faces": {"gate": []}},
            lambda r: (
                r["kept"] == ["gate"]
                and set(r["why"]) == gate_own
                and not r["vm_tests"]
                and set(r["rust_crates"]) == {"verify", "dd"}
            ),
        ),
        (
            "a gate change whose face moved: what tests the gate",
            [("M", "servers/gate/src/directory.rs")],
            {"faces": {"gate": ["rust-routes servers/gate/src: the face changed"]}},
            lambda r: (
                gate_readers <= set(r["vm_tests"]) and "directory" in r["vm_tests"]
            ),
        ),
        (
            "a control-internal change with its face held",
            [("M", "servers/system/control/state/machine.go")],
            {"faces": {"control": []}},
            lambda r: set(r["why"]) == control_own and not r["vm_tests"],
        ),
        (
            "a control change whose face moved",
            [("M", "servers/system/control/state/machine.go")],
            {"faces": {"control": ["apidiff state: changed"]}},
            lambda r: control_own <= set(r["why"]) and r["vm_tests"],
        ),
        (
            "a test registered, with its files",
            [
                ("M", plan.REGISTRY),
                ("A", "nix/tests/newtest.nix"),
                ("A", "nix/tests/newtest.py"),
            ],
            {"registry_base": (reg_text, added)},
            lambda r: (
                r["registered"] == ["newtest"]
                and "newtest" in r["why"]
                and not r["global"]
                and "console" not in r["vm_tests"]
            ),
        ),
        (
            "the registry changed beyond its names: what tests it, not everything",
            [("M", plan.REGISTRY)],
            {"registry_base": (reg_text, reshaped)},
            lambda r: (
                not r["global"]
                and "ci" in r["why"]
                and r["systems_eval"]
                and not r["vm_tests"]
            ),
        ),
        (
            "one crate's source: that crate's lint and tests, and fmt",
            [("M", "client/davbridge/src/gate.rs")],
            {},
            lambda r: (
                "davbridge" in r["rust_crates"]
                and "verify" not in r["rust_crates"]
                and r["rust"]
                and not r["vm_tests"]
            ),
        ),
        (
            "the app: main's full run alone",
            [("M", "app/src/lib.rs")],
            {},
            lambda r: (
                not r["app"] and ("app" in r["main_only"] or "app" not in main_only)
            ),
        ),
        (
            "the workflow: the checks that read it, not everything",
            [("M", ".forgejo/workflows/entry_point.yml")],
            {},
            lambda r: "ci" in r["why"] and not r["global"] and not r["vm_tests"],
        ),
        (
            "the selector itself: its own check",
            [("M", "nix/tests/ci-select/plan.py")],
            {},
            lambda r: "ci-select" in r["why"] and not r["global"] and not r["vm_tests"],
        ),
        (
            "a path no job tests: style and lint, and it is named",
            [("A", "tools/a-new-thing.sh")],
            {},
            lambda r: r["unclaimed"] == ["tools/a-new-thing.sh"] and not r["why"],
        ),
        (
            "the installer's own module: main's full run alone",
            [("M", "nix/installer/image.nix")],
            {},
            lambda r: not r["installer"] and "installer" in r["main_only"],
        ),
        (
            "a box page's script: the site checks, not the VM tests that never look",
            [("M", "servers/web-shared/home.css")],
            {},
            lambda r: "stylesheet" in r["why"] and r["site_checks"],
        ),
        (
            "the home controller's module: its system, built once",
            [("M", "nix/controller/default.nix")],
            {},
            lambda r: "home-controller" in r["hosts"] and "node1" not in r["hosts"],
        ),
        (
            "a job main-only.json names does not run",
            [("M", "nix/tests/acme.py")],
            {"mo": {"acme"}},
            lambda r: "acme" not in r["vm_tests"] and "acme" in r["main_only"],
        ),
    ]
    rows = []
    for title, changes, kw, ok in cases:
        r = choose(changes, **kw)
        good = bool(ok(r))
        if not good:
            problems.append(f"case failed: {title}")
        rows.append((title, changes, r, good))
    return rows, problems


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    rows, problems = run(root)
    for title, changes, r, good in rows:
        print(
            f"{'ok  ' if good else 'FAIL'} {title}: "
            + " ".join(f"{s} {p}" for s, p in changes)
        )
        if r["docs_only"]:
            print("     -> docs only")
        else:
            print(
                f"     -> {len(r['why'])} jobs: {' '.join(sorted(r['why']))[:300]}"
                + (
                    f"; main's alone: {' '.join(sorted(r['main_only']))}"
                    if r["main_only"]
                    else ""
                )
            )
    for p in problems:
        print(f"FAIL {p}")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
