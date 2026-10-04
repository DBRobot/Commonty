#!/usr/bin/env python3
# The style check: the pull request's title and body, its commits, and the
# files it changes. Every finding blocks the merge; files the pull request
# does not touch are never looked at, so an old file holds nobody up.
#
# Environment: TITLE, BODY (the pull request's), BASE, HEAD (commits),
# AUTHORS (the commit emails allowed, space separated).
import json
import os
import re
import subprocess
import sys
import tomllib

TYPES = [
    "feature",
    "service",
    "fix",
    "security",
    "ci",
    "docs",
    "refactor",
    "deps",
    "chore",
]
TITLE_RE = re.compile(r"^(WIP: )?(release: )?(%s): (\S.*)$" % "|".join(TYPES))
TITLE_MAX = 120
SECTIONS = ["What", "Why", "How it was tested", "Risk and rollback"]
# what each type must say, beyond the title
NEEDS = {t: SECTIONS for t in TYPES}
NEEDS["docs"] = NEEDS["chore"] = ["What"]
NEEDS["service"] = SECTIONS + ["Hardware and ports"]
# David's rule: nothing in a pull request or a commit names a tool as an author
ATTRIBUTION = re.compile(
    r"co-authored-by:|generated (with|by) \[?claude|claude\.ai/(code|chat|share)|🤖",
    re.I,
)
BIG = 1024 * 1024
BIG_ALLOWED = {"Cargo.nix"}  # crate2nix's output, generated and large by nature
LEFTOVER = re.compile(
    r"(^|/)(result(-[^/]*)?|target/|node_modules/|\.direnv/|__pycache__/|\.DS_Store$)|\.(pyc|swp|orig|rej)$"
)
# where trailing spaces mean something: markdown's line break, a patch's context
KEEP_SPACES = re.compile(r"\.(md|patch|diff)$")

problems = []


def say(where, what):
    problems.append(f"{where}: {what}")


def git(*args):
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True
    ).stdout


def title(text):
    m = TITLE_RE.match(text)
    if not m:
        say(
            "title",
            f'"{text}" is not "<type>: <what it does>", type one of: {", ".join(TYPES)}',
        )
        return None
    if len(m.group(4)) > TITLE_MAX:
        say(
            "title", f"{len(m.group(4))} characters after the type; at most {TITLE_MAX}"
        )
    if re.match(r"(fixup|squash|amend)!", m.group(4)):
        say("title", "a fixup title")
    return m.group(3)


def body(text, kind):
    if ATTRIBUTION.search(text):
        say(
            "body",
            "names a tool as an author (Co-Authored-By, Generated with, a session link)",
        )
    text = re.sub(r"<!--.*?-->", "", text, flags=re.S)
    found = {}
    current = None
    for line in text.splitlines():
        h = re.match(r"^##\s+(.+?)\s*$", line)
        if h:
            current = h.group(1)
            found[current] = ""
        elif current:
            found[current] += line.strip()
    for name in NEEDS.get(kind, SECTIONS):
        if name not in found:
            say(
                "body",
                f'no "## {name}" section (the template: .forgejo/pull_request_template.md)',
            )
        elif not found[name]:
            say("body", f'"## {name}" is empty')


def commits(base, head, authors):
    if git("rev-list", "--merges", f"{base}..{head}").strip():
        say("commits", "a merge commit inside the branch: rebase onto main instead")
    for line in git(
        "log", "--format=%H%x00%ae%x00%s%x00%B%x1e", f"{base}..{head}"
    ).split("\x1e"):
        if not line.strip():
            continue
        sha, email, subject, message = line.strip().split("\x00", 3)
        short = sha[:9]
        if authors and email not in authors:
            say(
                f"commit {short}",
                f"authored as {email}; expected {' or '.join(sorted(authors))}",
            )
        if re.match(r"(fixup|squash|amend)!", subject):
            say(f"commit {short}", f'"{subject}" is a fixup: squash it before merging')
        if ATTRIBUTION.search(message):
            say(
                f"commit {short}",
                "names a tool as an author (Co-Authored-By, Generated with, a session link)",
            )


def text_of(path):
    data = open(path, "rb").read()
    if b"\0" in data[:8192]:
        return data, None
    try:
        return data, data.decode()
    except UnicodeDecodeError:
        return data, None


def tool(args, path):
    r = subprocess.run(args + [path], capture_output=True, text=True)
    return r.returncode == 0, (r.stdout + r.stderr).strip()


def files(base, head):
    changed = git(
        "diff", "--name-only", "--diff-filter=ACMR", f"{base}...{head}"
    ).split("\n")
    for path in filter(None, changed):
        if LEFTOVER.search(path):
            say(
                path, "a build or editor leftover; it does not belong in the repository"
            )
        if not os.path.isfile(path):
            continue
        data, text = text_of(path)
        if len(data) > BIG and path not in BIG_ALLOWED:
            say(
                path,
                f"{len(data) // 1024} KiB; files over 1 MiB stay out of git (the history keeps them forever)",
            )
        if text is None:
            continue
        lines = text.split("\n")
        for n, line in enumerate(lines, 1):
            if (
                line.startswith(("<<<<<<< ", ">>>>>>> "))
                or line == "<<<<<<<"
                or line == ">>>>>>>"
            ):
                say(f"{path}:{n}", "a merge conflict marker")
            if line.rstrip("\r") != line.rstrip("\r").rstrip(
                " \t"
            ) and not KEEP_SPACES.search(path):
                say(f"{path}:{n}", "trailing spaces")
        if text and not text.endswith("\n"):
            say(path, "no newline at the end")
        if path.endswith(".json"):
            try:
                json.loads(text)
            except ValueError as e:
                say(path, f"not valid json: {e}")
        elif path.endswith((".yml", ".yaml")):
            ok, out = tool(["yq", "-e", "true"], path)
            if not ok:
                say(path, f"not valid yaml: {out.splitlines()[-1] if out else ''}")
            if path.startswith("secrets/") and ok:
                r = subprocess.run(
                    [
                        "yq",
                        '[del(.sops) | .. | select(tag != "!!map" and tag != "!!seq")] | map(select((. | tostring | test("^ENC\\[")) | not)) | length',
                        path,
                    ],
                    capture_output=True,
                    text=True,
                )
                if "sops:" not in text or r.stdout.strip() != "0":
                    say(
                        path,
                        "not encrypted with sops: a secret in the clear would be published",
                    )
        elif path.endswith(".toml"):
            try:
                tomllib.loads(text)
            except tomllib.TOMLDecodeError as e:
                say(path, f"not valid toml: {e}")
        elif path.endswith(".py"):
            ok, out = tool(["ruff", "format", "--check", "--quiet"], path)
            if not ok:
                say(path, "not formatted: ruff format " + path)
        elif path.endswith(".nix") and path not in BIG_ALLOWED:
            ok, out = tool(["nixfmt", "--check"], path)
            if not ok:
                say(path, "not formatted: nixfmt " + path)


def main():
    base, head = os.environ["BASE"], os.environ["HEAD"]
    kind = title(os.environ.get("TITLE", ""))
    body(os.environ.get("BODY", ""), kind)
    commits(base, head, set(os.environ.get("AUTHORS", "").split()))
    files(base, head)
    for p in problems:
        print(p)
    if problems:
        print(f"\n{len(problems)} to fix before this can merge")
        sys.exit(1)
    print("style: all good")


if __name__ == "__main__":
    main()
