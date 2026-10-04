#!/usr/bin/env python3
# The linters, on what a pull request changes. A real fault blocks the merge
# (a script that cannot run, an undefined name, a secret); everything else -
# style, spelling, a likely slip - is a warning: listed here, and counted in
# a yellow lint/warnings status on the commit that blocks nothing.
#
# Environment: BASE, HEAD (commits); and, to post the warnings status,
# TOKEN, API (the repository's api), SHA, RUN (this run's page).
import json
import os
import subprocess
import sys
import urllib.request

HERE = os.path.dirname(os.path.abspath(__file__))
faults, warnings = [], []


def run(args, env=None):
    r = subprocess.run(
        args, capture_output=True, text=True, env={**os.environ, **(env or {})}
    )
    return r.returncode, r.stdout, r.stderr


def lines(out):
    return [ln for ln in out.splitlines() if ln.strip()]


def changed(base, head, *suffixes):
    out = subprocess.run(
        ["git", "diff", "--name-only", "--diff-filter=ACMR", f"{base}...{head}"],
        check=True,
        capture_output=True,
        text=True,
    ).stdout
    return [
        p
        for p in out.split("\n")
        if p and os.path.isfile(p) and (not suffixes or p.endswith(suffixes))
    ]


def actionlint():
    # the workflows, whole: they are small, and a fault anywhere stops every run.
    # Forgejo takes an action by its url, which actionlint does not know.
    common = [
        "actionlint",
        "-oneline",
        "-config-file",
        f"{HERE}/actionlint.yaml",
        "-ignore",
        "specifying action .* in invalid format",
    ]
    flows = [
        os.path.join(".forgejo/workflows", f)
        for f in sorted(os.listdir(".forgejo/workflows"))
        if f.endswith((".yml", ".yaml"))
    ]
    _, out, _ = run(common + flows, {"SHELLCHECK_OPTS": "--severity=error"})
    blocking = set(lines(out))
    faults.extend(f"actionlint: {ln}" for ln in blocking)
    _, out, _ = run(common + flows, {"SHELLCHECK_OPTS": "--severity=style"})
    warnings.extend(f"actionlint: {ln}" for ln in lines(out) if ln not in blocking)


def shellcheck(paths):
    if not paths:
        return
    _, out, _ = run(["shellcheck", "-f", "gcc", "--severity=style", *paths])
    for ln in lines(out):
        (faults if ": error:" in ln else warnings).append(f"shellcheck: {ln}")


def ruff(paths):
    if not paths:
        return
    # a NixOS test script is handed start_all, the machines and the rest by
    # the test driver, so its names are defined where ruff cannot see
    base = [
        "ruff",
        "check",
        "--quiet",
        "--output-format",
        "concise",
        "--extend-per-file-ignores",
        "nix/tests/*.py:F821",
    ]
    _, out, _ = run(base + ["--select", "E9,F63,F7,F82", *paths])
    blocking = set(lines(out))
    faults.extend(f"ruff: {ln}" for ln in blocking)
    _, out, _ = run(base + paths)
    warnings.extend(
        f"ruff: {ln}"
        for ln in lines(out)
        if ln not in blocking and not ln.startswith("Found ")
    )


def biome(paths):
    if not paths:
        return
    _, out, err = run(
        ["biome", "lint", "--reporter=json", "--max-diagnostics=500", *paths]
    )
    try:
        report = json.loads(out)
    except ValueError:
        faults.append(f"biome: could not read its report: {(err or out)[-300:]}")
        return
    for d in report.get("diagnostics", []):
        where = d.get("location", {}).get("path", {}).get("file", "?")
        line = f"biome: {where}: {d.get('category', '?')}: {d.get('description', '')}"
        (faults if d.get("severity") in ("error", "fatal") else warnings).append(line)


def secrets(base, head):
    # only what this pull request adds: the tree holds public keys and test
    # fixtures that look like secrets and are not (.forgejo/checks/gitleaks.toml)
    code, out, err = run(
        [
            "gitleaks",
            "git",
            "--no-banner",
            "--redact",
            "--report-format",
            "json",
            "--report-path",
            "/dev/stdout",
            "--config",
            f"{HERE}/gitleaks.toml",
            "--log-opts",
            f"{base}..{head}",
        ]
    )
    if code not in (0, 1):
        faults.append(f"gitleaks: did not run: {err[-300:]}")
        return
    try:
        found = json.loads(out or "[]")
    except ValueError:
        found = []
    for f in found:
        faults.append(
            f"secret: {f.get('File')}:{f.get('StartLine')}: {f.get('RuleID')} in commit {f.get('Commit', '')[:9]} (value hidden)"
        )


def spelling(paths):
    if not paths:
        return
    _, out, _ = run(
        ["typos", "--format", "brief", "--config", f"{HERE}/typos.toml", *paths]
    )
    warnings.extend(f"spelling: {ln}" for ln in lines(out))


def post(state, description):
    token, api, sha = (
        os.environ.get("TOKEN"),
        os.environ.get("API"),
        os.environ.get("SHA"),
    )
    if not (token and api and sha):
        return
    body = json.dumps(
        {
            "state": state,
            "context": "lint/warnings",
            "description": description,
            "target_url": os.environ.get("RUN", ""),
        }
    ).encode()
    req = urllib.request.Request(
        f"{api}/statuses/{sha}",
        data=body,
        method="POST",
        headers={"authorization": f"token {token}", "content-type": "application/json"},
    )
    try:
        urllib.request.urlopen(req, timeout=30).read()
    except OSError as e:
        print(f"(the warnings status was not posted: {e})")


def main():
    base, head = os.environ["BASE"], os.environ["HEAD"]
    files = changed(base, head)
    actionlint()
    shellcheck([p for p in files if p.endswith(".sh")])
    ruff([p for p in files if p.endswith(".py")])
    biome(
        [
            p
            for p in files
            if p.endswith((".js", ".mjs", ".css", ".json"))
            and not p.startswith("app/web/_dd/")
        ]
    )
    secrets(base, head)
    spelling(files)
    for w in warnings:
        print(f"warning  {w}")
    for f in faults:
        print(f"FAULT    {f}")
    print(
        f"\n{len(faults)} faults (these block the merge), {len(warnings)} warnings (these do not)"
    )
    post(
        "warning" if warnings else "success",
        f"{len(warnings)} warnings" if warnings else "no warnings",
    )
    sys.exit(1 if faults else 0)


if __name__ == "__main__":
    main()
