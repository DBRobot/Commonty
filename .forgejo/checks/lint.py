#!/usr/bin/env python3
# The linters, on what a pull request changes. A real fault blocks the merge
# (a script that cannot run, an undefined name, a secret); everything else -
# style, spelling, a likely slip - is a warning: listed here, and counted in
# a yellow lint/warnings status on the commit that blocks nothing.
#
# Environment: BASE, HEAD (commits), TITLE (the pull request's); and, to
# post the warnings status, TOKEN, API (the repository's api), SHA, RUN.
import json
import os
import re
import subprocess
import sys
import time
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


# every workflow: the forge's ci, and the app's release build on GitHub
FLOW_DIRS = (".forgejo/workflows", ".github/workflows")


def workflows():
    return [
        os.path.join(d, f)
        for d in FLOW_DIRS
        if os.path.isdir(d)
        for f in sorted(os.listdir(d))
        if f.endswith((".yml", ".yaml"))
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
    flows = workflows()
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
        loc = d.get("location") or {}
        path = loc.get("path")
        where = path if isinstance(path, str) else (path or {}).get("file", "?")
        if isinstance(loc.get("start"), dict) and "line" in loc["start"]:
            where += f":{loc['start']['line']}"
        what = d.get("message") or d.get("description", "")
        line = f"biome: {where}: {d.get('category', '?')}: {what}"
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


def pinned():
    # an action by a full commit hash or from this repository, never by a
    # tag or a branch, which whoever holds the action's repository can move
    files = workflows()
    for root, _, names in os.walk(".forgejo/actions"):
        files += [os.path.join(root, n) for n in names if n.endswith((".yml", ".yaml"))]
    for f in sorted(files):
        for n, line in enumerate(open(f), 1):
            m = re.match(r"\s*(?:-\s*)?uses:\s*([^\s#]+)", line)
            if not m or m.group(1).startswith("./"):
                continue
            if not re.search(r"@[0-9a-f]{40}$", m.group(1)):
                faults.append(
                    f"pinned: {f}:{n}: {m.group(1)} is not pinned to a full commit hash"
                )


def advisories(base, head):
    # RustSec's advisories against Cargo.lock: a crate this pull request
    # brings in or moves to a version with one blocks; one already there
    # is a warning, for a deps: pull request to take up
    if not os.path.isfile("Cargo.lock"):
        return
    added = set()
    diff = subprocess.run(
        ["git", "diff", f"{base}...{head}", "--", "Cargo.lock"],
        capture_output=True,
        text=True,
    ).stdout
    name = None
    for line in diff.splitlines():
        m = re.match(r'^[ +]name = "(.+)"', line)
        if m:
            name = m.group(1)
        m = re.match(r'^\+version = "(.+)"', line)
        if m and name:
            added.add((name, m.group(1)))
    db = os.path.join(os.environ.get("HOME", "/tmp"), ".cache", "advisory-db")
    code, out, err = run(
        ["cargo-audit", "audit", "--db", db, "--file", "Cargo.lock", "--json"]
    )
    try:
        report = json.loads(out)
    except ValueError:
        warnings.append(
            f"advisories: the advisory database was not read: {err.strip()[-200:]}"
        )
        return
    for v in report.get("vulnerabilities", {}).get("list", []):
        pkg, adv = v["package"], v["advisory"]
        line = f"advisory: {pkg['name']} {pkg['version']}: {adv['id']} {adv.get('title', '')}"
        (faults if (pkg["name"], pkg["version"]) in added else warnings).append(line)
    for kind, items in (report.get("warnings") or {}).items():
        for w in items:
            pkg = w.get("package", {})
            adv = w.get("advisory") or {}
            warnings.append(
                f"advisory ({kind}): {pkg.get('name')} {pkg.get('version')}: {adv.get('id', '')} {adv.get('title', '')}".rstrip()
            )


def stale_inputs(days=90):
    # the flake's inputs, by how long since each was last moved
    try:
        nodes = json.load(open("flake.lock"))["nodes"]
    except (OSError, ValueError, KeyError):
        return
    root = nodes.get("root", {}).get("inputs", {})
    for name, ref in sorted(root.items()):
        node = nodes.get(ref if isinstance(ref, str) else "", {})
        when = node.get("locked", {}).get("lastModified")
        if when and time.time() - when > days * 86400:
            age = int((time.time() - when) // 86400)
            warnings.append(f"flake.lock: {name} was last moved {age} days ago")


# a title's kind against what it touches: a likely mislabel, never a block
DOCS_ONLY = re.compile(r"^(docs/|.*\.md$|LICENSE$)")
LOCKS = re.compile(
    r"^(flake\.lock|Cargo\.(lock|nix)|crate-hashes\.json|.*go\.(sum|mod))$"
)
CODE = re.compile(r"^(box|client|app)/.*\.(rs|go|js|py|sh)$")


# Tests under nix/tests (the VM tests' scripts, and the browser flows and
# fakes in its folders): the waits that made tests fail now and then.
# Only lines a pull request adds are held to it, so a test is moved over to
# nix/tests/lib/waits.py when it is next touched (docs/dev/ci.md, "Writing
# VM tests that don't flake").
SLEEP_OK = re.compile(r"#\s*sleep-ok:\s*\S+(\s+\S+){2,}")
START = re.compile(r"systemctl\s+(?:re)?start\s+(?:--\S+\s+)*([\w@.:-]+)")
JOURNAL_UNIT = re.compile(r"journalctl\b[^\n]*?-u\s*([\w@.:-]+)")


def unit_name(u):
    return u if "." in u else u + ".service"


def vm_test_waits(base, head):
    paths = [p for p in changed(base, head, ".py") if p.startswith("nix/tests/")]
    for p in paths:
        diff = subprocess.run(
            ["git", "diff", "-U0", f"{base}...{head}", "--", p],
            check=True,
            capture_output=True,
            text=True,
        ).stdout
        added = set()
        for line in diff.splitlines():
            m = re.match(r"@@ -\S+ \+(\d+)(?:,(\d+))? @@", line)
            if m:
                first, count = int(m.group(1)), int(m.group(2) or 1)
                added.update(range(first, first + count))
        text = open(p, encoding="utf-8").read().splitlines()
        for n in sorted(added):
            if n > len(text):
                continue
            ln = text[n - 1]
            # a comment line says what not to do as often as it does it
            code = "" if ln.lstrip().startswith("#") else ln
            where = f"{p}:{n}"
            if re.search(r"\|\s*grep\s+(-\w*q|--quiet)", code):
                faults.append(
                    f"vm test: {where}: `| grep -q` - grep leaves at the first match and the "
                    "writer dies of SIGPIPE (curl: exit 23); write to a file and grep -c it, "
                    "or fetch_has / journal_has from nix/tests/lib/waits.py"
                )
            if "time.sleep(" in code and not SLEEP_OK.search(ln):
                faults.append(
                    f"vm test: {where}: time.sleep - wait for the thing itself "
                    "(wait_until_succeeds, wait_unit_active, wait_socket; in a browser flow, "
                    "wait_for_selector or wait_for_function); a sleep that must stay says why "
                    "on its line: # sleep-ok: <reason>"
                )
            # a browser flow's fixed pause (Playwright) is the same guess
            if "wait_for_timeout(" in code and not SLEEP_OK.search(ln):
                faults.append(
                    f"vm test: {where}: wait_for_timeout - wait for what the page will show "
                    "(wait_for_selector, wait_for_function, expect); one that must stay says "
                    "why on its line: # sleep-ok: <reason>"
                )
            if "journalctl" in code and "_SYSTEMD_INVOCATION_ID" not in code:
                j = JOURNAL_UNIT.search(code)
                # the ten lines before, and this one up to the journal
                before = "\n".join(
                    text[max(0, n - 11) : n - 1] + [code[: code.index("journalctl")]]
                )
                started = {unit_name(u) for u in START.findall(before)}
                if j and unit_name(j.group(1)) in started:
                    faults.append(
                        f"vm test: {where}: the journal of {j.group(1)} read after starting it "
                        "holds every run's lines, an earlier run's too; start_fresh() and "
                        "journal_of() read the one run"
                    )
                elif (
                    "succeed(" in code
                    and "grep" in code
                    and START.search(
                        "\n".join(
                            text[max(0, n - 4) : n - 1]
                            + [code[: code.index("journalctl")]]
                        )
                    )
                ):
                    faults.append(
                        f"vm test: {where}: a journal grep held to succeed() right after a "
                        "start - the lines may not be written yet, or be an earlier run's; "
                        "start_fresh() + journal_has(), or wait_until_succeeds"
                    )


def kind_matches(files):
    m = re.match(r"^(?:WIP: )?(?:release: )?([a-z]+): ", os.environ.get("TITLE", ""))
    if not m or not files:
        return
    kind = m.group(1)
    if kind == "docs" and not all(DOCS_ONLY.match(p) for p in files):
        warnings.append("title: a docs: pull request that changes more than docs")
    if kind == "deps" and not any(LOCKS.match(p) for p in files):
        warnings.append("title: a deps: pull request that changes no lockfile")
    if kind == "chore" and any(CODE.match(p) for p in files):
        warnings.append(
            "title: a chore: pull request that changes a program; a fix: or a feature:?"
        )


def slug(text):
    # as the docs page and the bar's search make it (servers/web-shared/shell.js)
    text = re.sub(r"[^\w\s-]", "", text.lower())
    return re.sub(r"\s+", "-", text.strip())


def docs_links():
    # every link in docs/ goes somewhere: a page and its heading, or a file
    # in the repository. And every page is reachable from docs/README.md's
    # list, when there is one - else nobody finds it.
    if not os.path.isdir("docs"):
        return
    pages = {}
    for root, _, names in os.walk("docs"):
        for n in names:
            if n.endswith(".md"):
                p = os.path.join(root, n)
                pages[p] = open(p, encoding="utf-8").read()
    anchors = {}
    for p, text in pages.items():
        found, fenced = set(), False
        for line in text.splitlines():
            if line.lstrip().startswith(("```", "~~~")):
                fenced = not fenced
            elif not fenced and (h := re.match(r"^#{1,6}\s+(.+?)\s*#*$", line)):
                found.add(
                    slug(re.sub(r"[`*_]|\[([^\]]*)\]\([^)]*\)", r"\1", h.group(1)))
                )
        anchors[p] = found
    linked = set()
    for p, text in pages.items():
        here = os.path.dirname(p)
        text = re.sub(r"```.*?```", "", text, flags=re.S)
        for n, line in enumerate(text.splitlines(), 1):
            for target in re.findall(r"\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)", line):
                if re.match(r"^[a-z][a-z0-9+.-]*:", target) or target.startswith("//"):
                    continue
                path, _, anchor = target.partition("#")
                dest = os.path.normpath(os.path.join(here, path)) if path else p
                if path and not os.path.exists(dest):
                    faults.append(f"docs: {p}:{n}: {target} goes nowhere")
                    continue
                if dest in pages:
                    linked.add(dest)
                    if anchor and anchor not in anchors[dest]:
                        faults.append(
                            f"docs: {p}:{n}: {target} names a heading {dest} does not have"
                        )
    readme = os.path.join("docs", "README.md")
    if readme in pages:
        for p in sorted(pages):
            if p != readme and p not in linked:
                warnings.append(
                    f"docs: {p} is not linked from any page; the nav will list it under More"
                )


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
    vm_test_waits(base, head)
    pinned()
    advisories(base, head)
    stale_inputs()
    kind_matches(files)
    docs_links()
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
