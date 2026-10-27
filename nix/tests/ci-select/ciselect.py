#!/usr/bin/env python3
# Which jobs a pull request's run runs (docs/dev/ci.md, Choosing the
# tests): the files it changes, read from git at both ends, handed to
# plan.py, and its answer written for the workflow.
#
#   select BASE HEAD   files_modified: what this pull request runs
#   select --everything | --nothing
#                      main's full run; a draft or a commit already passed
#   matches FILE       the registry's VM tests (ci.nix)
#   groups             the browser and Worker jobs (ci.nix)
import json
import os
import re
import subprocess
import sys
import tomllib

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import interfaces  # noqa: E402
import plan  # noqa: E402

# What each test usually takes, in minutes, from the forge's history
# (minutes.json): the longest are handed out first, so one long test never
# starts last and holds the run open alone. A test not there counts as long.
MINUTES = "nix/tests/ci-select/minutes.json"


def longest_first(tests, minutes):
    return sorted(tests, key=lambda t: (-minutes.get(t, 99), t))


def git(*args, cwd=None):
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True, cwd=cwd
    ).stdout


def show(rev, path, cwd=None):
    r = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True, cwd=cwd)
    return r.stdout.decode(errors="replace") if r.returncode == 0 else ""


def name_status(base, head, cwd=None):
    out = git("diff", "--name-status", "--no-renames", "-z", base, head, cwd=cwd)
    parts = out.split("\0")
    changes = []
    for i in range(0, len(parts) - 1, 2):
        s, p = parts[i][0], parts[i + 1]
        changes.append(("M" if s in "MT" else s, p))
    return changes


def named_docs(read):
    # docs files a build reads by name: a path in a .nix file, an
    # include_str!/include_bytes! in Rust
    named = set()
    for path, text in read():
        if path.endswith(".nix"):
            for m in re.finditer(r"(?:\./|\.\./)+(docs/[\w./-]+)", text):
                named.add(m.group(1))
        elif path.endswith(".rs"):
            for m in re.finditer(
                r"include_(?:str|bytes)!\([^)]*?(docs/[\w./-]+)", text
            ):
                named.add(m.group(1))
    return named


def crates(read):
    """{crate path: package name} of the workspace (Cargo.toml), but the
    app's, whose checks are app_checks'"""
    try:
        members = tomllib.loads(read("Cargo.toml"))["workspace"]["members"]
    except (tomllib.TOMLDecodeError, KeyError, TypeError):
        return {}
    out = {}
    for m in members:
        try:
            name = tomllib.loads(read(f"{m}/Cargo.toml"))["package"]["name"]
        except (tomllib.TOMLDecodeError, KeyError, TypeError):
            continue
        if m != "app":
            out[m] = name
    return out


def hosts(read, sets):
    boxes = json.loads(read("fleet/boxes.json") or "{}")
    devices = json.loads(read("fleet/devices.json") or "{}")
    return plan.host_keys(
        [b for b in boxes if b != "_"], [d for d in devices if d != "_"], sets
    )


def registry(text):
    return plan.registry_lists(text)[0]


def select_revs(base, head, cwd=None):
    changes = name_status(base, head, cwd)

    def read(f):
        return show(head, f, cwd)

    def every():
        for f in git("ls-tree", "-r", "--name-only", head, cwd=cwd).split("\n"):
            if f.endswith((".nix", ".rs")):
                yield f, show(head, f, cwd)

    sets = plan.load_sets(read(plan.SETS))
    reg = read(plan.REGISTRY)
    return plan.plan(
        changes,
        sets,
        registry(reg),
        main_only=plan.load_main_only(read(plan.MAIN_ONLY)),
        faces=interfaces.stable(changes, base, head, cwd),
        docs_named=named_docs(every),
        registry_base=(show(base, plan.REGISTRY, cwd), reg),
        crates=crates(read),
        hosts=hosts(read, sets),
    )


def here(f):
    return open(f).read() if os.path.exists(f) else ""


def cmd_select(args):
    if args[0] == "--nothing":
        r = plan.nothing()
    elif args[0] == "--everything":
        r = plan.everything(registry(here(plan.REGISTRY)), crates(here))
    else:
        r = select_revs(args[0], args[1])
    minutes = json.loads(here(MINUTES) or "{}")
    r["vm_tests"] = longest_first(r["vm_tests"], minutes)
    for n, d in r["faces"].items():
        print(f"{n}: {'face held: its own checks' if not d else '; '.join(d)}")
    if r["docs_only"]:
        print("docs only: style and lint, nothing else")
    if r["global"]:
        print(f"everything: {r['global']}")
    for k, w in sorted(r["why"].items()):
        print(f"  {k}: {w}")
    for k, w in sorted(r["main_only"].items()):
        print(f"  {k}: main's full run alone ({w})")
    for p in r["unclaimed"]:
        print(f"  no job tests {p}")
    print(
        f"flake: {r['flake']} (systems: {r['systems_eval']}, checks: "
        f"{len(r['eval_checks'])})  keep: {r['keep']}  rust: {r['rust']} "
        f"(crates: {len(r['rust_crates'])})  app: {r['app']}  programs: "
        f"{r['programs']}  hosts: {len(r['hosts'])}  installer: {r['installer']}"
        f"  vm tests: {len(r['vm_tests'])}  "
        + "  ".join(f"{g}: {len(r[g])}" for g in plan.GROUPS)
    )
    # every name goes into a nix attribute and a job's name: a name from
    # the change's own registry or Cargo.toml is never more than one word
    names = (
        r["vm_tests"]
        + r["eval_checks"]
        + r["rust_crates"]
        + r["hosts"]
        + [t for g in plan.GROUPS for t in r[g]]
    )
    bad = [n for n in names if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", n)]
    if bad:
        print(f"not a check's name: {bad}", file=sys.stderr)
        return 1
    gh = os.environ.get("GITHUB_OUTPUT")
    if gh:
        with open(gh, "a") as f:
            for k in (
                "flake",
                "systems_eval",
                "keep",
                "rust",
                "app",
                "programs",
                "installer",
            ):
                f.write(f"{k}={'true' if r[k] else 'false'}\n")
            f.write(f"vm={'true' if r['vm_tests'] else 'false'}\n")
            # attributes for nix build, one line
            f.write(
                "eval_checks="
                + " ".join(
                    f".#checks.x86_64-linux.{c}"
                    for c in r["eval_checks"]
                    if c != plan.APP_EVAL
                )
                + (
                    " .#legacyPackages.x86_64-linux.appReleaseEvaluates"
                    if plan.APP_EVAL in r["eval_checks"]
                    else ""
                )
                + "\n"
            )
            f.write(
                "rust_crates="
                + " ".join(
                    f".#legacyPackages.x86_64-linux.rustChecks.{c}"
                    for c in r["rust_crates"]
                )
                + "\n"
            )
            # a matrix is never empty: the forge creates no job at all for
            # an empty one, and ci_ok, which needs it, then waits forever
            # (run 860, 2026-10-24). "none" stands for nothing chosen, and
            # each job's if skips on it
            f.write(f"vm_tests={json.dumps(r['vm_tests'] or ['none'])}\n")
            f.write(f"pr_hosts={json.dumps(r['hosts'] or ['none'])}\n")
            for g in plan.GROUPS:
                f.write(f"{g}={json.dumps(r[g] or ['none'])}\n")
    return 0


def main():
    if len(sys.argv) < 2:
        print(__doc__ or "select BASE HEAD | matches FILE | groups", file=sys.stderr)
        return 2
    cmd, args = sys.argv[1], sys.argv[2:]
    if cmd == "select":
        return cmd_select(args)
    if cmd == "matches":
        print(json.dumps(sorted(registry(open(args[0]).read()).get("vm", []))))
        return 0
    if cmd == "groups":
        print(json.dumps(sorted(plan.GROUPS)))
        return 0
    print(f"unknown command {cmd}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
