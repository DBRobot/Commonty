#!/usr/bin/env python3
# Which checks a change can reach (docs/dev/ci.md, Choosing the tests).
#
# A check's derivation is a function of the files its evaluation reads, the
# directories it lists and the paths it asks after. covers.json holds that
# for every VM test, every browser and Worker check (worker_tests,
# page_tests, site_checks) and the box installer's drive, traced from a
# real evaluation (strace of `nix eval`, from a store copy of the tree, in
# a fresh cache so nothing is skipped as already copied). To it the chooser
# adds, at both ends of the change, every file the check's .nix files name
# (static_refs), so an import a pull request adds counts before covers.json
# is traced again. A change that touches none of a check's paths leaves its
# derivation as it was, and the check is skipped.
#
#   select   files_modified: the jobs and tests a pull request needs
#   trace    write covers.json again (all of it, or --only some checks);
#            about a minute a check, so run it on a box
#   matches  does this script read the flake's test list right (ci.nix)
#   groups   the browser and Worker jobs and their checks (ci.nix)
import json
import os
import posixpath
import re
import shutil
import subprocess
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
COVERS = "nix/tests/ci-select/covers.json"
SYSTEM = "x86_64-linux"

# The checks run by a browser or wrangler against fakes, not VMs, by the
# workflow job that runs them (each job's matrix is its list, chosen check
# by check like the VM tests)
GROUPS = {
    # the Cloudflare Workers under wrangler: mail, DNS, sign-in (the id
    # Worker with the gate), the control plane's relay and validators
    "worker_tests": ["mail-worker", "dns-worker", "sso", "control"],
    # the Mail, Files and Movies & TV pages as the app serves them, and every bar link
    # and home tile staying in its tab
    "page_tests": ["mail-page", "files-page", "media-page", "same-tab"],
    # the site as a browser meets it, through node1's nginx and the gate
    "site_checks": [
        "site-headers",
        "site-roles",
        "site-a11y",
        "site-speed",
        "site-links",
    ],
}
BROWSER = [t for ts in GROUPS.values() for t in ts]
# Traced like a check but not one: name -> (the attribute evaluated, the
# .nix file its static references start from). The box installer's drive,
# for build_installer and installer_agrees.
TARGETS = {
    "installer": (f"installers.{SYSTEM}.box", "nix/installer/image.nix"),
}

# Under everything: a change here can move any check, so all of them run.
FOUNDATION = re.compile(
    r"^(flake\.(nix|lock)|nix/lib/|\.forgejo/|Cargo\.(toml|lock|nix)"
    r"|crate-hashes\.json|client/(identity|auth)/|nix/tests/ci-select/[^/]*\.py$)"
)
# Read by no build: the docs (the gate serves them as they are, and no test
# opens one), the notes at the top, the agents' skills, the GitHub workflow
# (lint reads it). A docs file a .nix or .rs names by path is not one.
DOCS = re.compile(r"^(docs/|\.claude/|\.github/|[^/]+\.md$|LICENSE$)")
# The coarse switches, as before, for the jobs that are not chosen test by
# test: what each job's builds can read. docs/ left nix's list: the docs
# reach a box only in its pages, which the release builds.
NIX = re.compile(
    r"^(flake\.nix|flake\.lock|nix/|fleet/|secrets/|data/|servers/|client/"
    r"|Cargo\.(toml|lock|nix)|crate-hashes\.json|\.forgejo/)"
)
RUST = re.compile(
    r"^(servers/|client/|Cargo\.(toml|lock|nix)|crate-hashes\.json|flake\.(nix|lock)|\.forgejo/)"
)
APP = re.compile(
    r"^(app/|client/|Cargo\.(toml|lock|nix)|crate-hashes\.json|flake\.(nix|lock)|nix/app-release/|\.forgejo/)"
)
# Directories whose files are each their own thing (a test, a box module, a
# fleet list): a read there covers that file alone, not its neighbours.
# Anywhere else a read or a listing covers the whole directory, so a new
# file beside the ones a check reads is already covered.
FILE_LEVEL = {"", "nix", "nix/tests", "nix/modules/box", "nix/roles", "fleet"}


def git(*args, cwd=None):
    return subprocess.run(
        ["git", *args], check=True, capture_output=True, text=True, cwd=cwd
    ).stdout


def vm_tests(flake_text):
    m = re.search(r"\n\s*vmTests = \[(.*?)\];", flake_text, re.S)
    return re.findall(r'"([^"]+)"', m.group(1)) if m else None


def traced_names(flake_text):
    # every name chosen one by one: the VM tests, the browser and Worker
    # checks, and the targets that are not checks
    return (vm_tests(flake_text) or []) + BROWSER + list(TARGETS)


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


# ---- covers.json


def load_covers(text):
    c = json.loads(text) if text else {}
    for k in ("dirs", "files", "listed"):
        c[k] = {p: set(v) for p, v in c.get(k, {}).items()}
    c["checks"] = set(c.get("checks", []))
    return c


def dump_covers(c, rev):
    def inv(m):
        return {p: sorted(m[p]) for p in sorted(m) if m[p]}

    return (
        json.dumps(
            {
                "_": "made by nix/tests/ci-select/ciselect.py trace",
                "rev": rev,
                "checks": sorted(c["checks"]),
                "dirs": inv(c["dirs"]),
                "files": inv(c["files"]),
                "listed": inv(c["listed"]),
            },
            indent=1,
        )
        + "\n"
    )


def of_check(c, t):
    # one check's own entries, for comparing two covers.json
    pick = lambda m: sorted(p for p, ts in m.items() if t in ts)  # noqa: E731
    return (pick(c["dirs"]), pick(c["files"]), pick(c["listed"]))


def add_trace(c, t, tr):
    for p in tr["reads"] | tr["stats"]:
        d = posixpath.dirname(p)
        if d in FILE_LEVEL:
            c["files"].setdefault(p, set()).add(t)
        else:
            c["dirs"].setdefault(d, set()).add(t)
    for d in tr["lists"]:
        if d in FILE_LEVEL:
            c["listed"].setdefault(d, set()).add(t)
        else:
            c["dirs"].setdefault(d, set()).add(t)
    c["checks"].add(t)


def missing(c, t, tr):
    # what a trace read that covers.json does not give the check
    out = []
    for p in sorted(tr["reads"] | tr["stats"]):
        d = posixpath.dirname(p)
        ok = t in c["files"].get(p, ()) or (
            d not in FILE_LEVEL and t in c["dirs"].get(d, ())
        )
        if not ok:
            out.append(p)
    for d in sorted(tr["lists"]):
        m = c["listed"] if d in FILE_LEVEL else c["dirs"]
        if t not in m.get(d, ()):
            out.append(d + "/")
    return out


def touching(c, status, path, exists_before, exists_after):
    # the checks one changed path can reach
    d = posixpath.dirname(path)
    hit = set(c["files"].get(path, ()))
    if d not in FILE_LEVEL:
        hit |= c["dirs"].get(d, set())
    if status != "M":
        # a name coming or going changes its directory's listing, and every
        # directory above it that came or went with it
        exists = exists_before if status == "A" else exists_after
        a = d
        while a and not exists(a):
            a = posixpath.dirname(a)
        for x in {d, a}:
            hit |= c["listed"].get(x, set())
            if x not in FILE_LEVEL:
                hit |= c["dirs"].get(x, set())
    return hit


# ---- what a check's .nix files name, read without evaluating

PATHLIT = re.compile(r"(?<![\w.])((?:\.\.?/)+[A-Za-z0-9_.${}/+-]*)")


def static_refs(entry, read, isdir):
    """Every file the .nix files reachable from `entry` name by a literal
    path, followed through the .nix ones, and the directory of any path
    built at evaluation (`./x/${name}.nix` names all of ./x). Cheap, and it
    sees an import a pull request adds, which covers.json, traced before,
    cannot. Returns (files, dirs)."""
    files, dirs, todo = set(), set(), [entry]
    while todo:
        f = todo.pop()
        if f in files:
            continue
        if not f.endswith(".nix"):
            if not isdir(f):
                files.add(f)
            continue
        text = read(f)
        if text is None:
            continue
        files.add(f)
        here = posixpath.dirname(f)
        for m in PATHLIT.finditer(text):
            lit = m.group(1).rstrip("};,)'\"")
            dyn = "${" in lit
            if dyn:
                lit = lit[: lit.index("${")]
                lit = lit[: lit.rfind("/") + 1] if "/" in lit else lit
            q = posixpath.normpath(posixpath.join(here, lit))
            if q.startswith(".."):
                continue
            if dyn or isdir(q):
                dirs.add("" if q == "." else q)
                if not dyn and read(q + "/default.nix") is not None:
                    todo.append(q + "/default.nix")
            else:
                todo.append(q)
    return files, dirs


def add_static(c, checks, read, isdir):
    # covers.json and what the checks' .nix files name now, together
    c = {
        k: {p: set(v) for p, v in c[k].items()} for k in ("dirs", "files", "listed")
    } | {"checks": set(c["checks"])}
    for t in checks:
        entry = TARGETS[t][1] if t in TARGETS else f"nix/tests/{t}.nix"
        if read(entry) is None:
            entry = f"nix/tests/{t}/default.nix"
        fs, ds = static_refs(entry, read, isdir)
        if read(f"nix/tests/{t}.py") is not None:
            fs.add(f"nix/tests/{t}.py")
        for f in fs:
            d = posixpath.dirname(f)
            (
                c["files"].setdefault(f, set())
                if d in FILE_LEVEL
                else c["dirs"].setdefault(d, set())
            ).add(t)
        for d in ds:
            c["dirs"].setdefault(d, set()).add(t)
            c["listed"].setdefault(d, set()).add(t)
    return c


# ---- select


def crate_closure(roots, read):
    # the crates a build compiles: its roots and their path dependencies,
    # followed to the end (flake.nix crateClosure), from each Cargo.toml
    import tomllib

    seen, todo = [], list(roots)
    while todo:
        d = todo.pop()
        if d in seen:
            continue
        seen.append(d)
        try:
            m = tomllib.loads(read(d + "/Cargo.toml"))
        except (tomllib.TOMLDecodeError, TypeError, OSError):
            continue
        deps = {
            **m.get("dependencies", {}),
            **m.get("build-dependencies", {}),
            **m.get("dev-dependencies", {}),
        }
        for v in deps.values():
            if isinstance(v, dict) and "path" in v:
                todo.append(posixpath.normpath(posixpath.join(d, v["path"])))
    return seen


# the app's crates (flake.nix sources.app): a change to any runs app_checks
APP_ROOTS = ["app", "client/hubwriter"]


def select(
    changes, covers_head, covers_base, flake_text, docs_named, exists, app_dirs=()
):
    """changes: [(status, path)], status A/M/D (renames as a D and an A).
    exists(rev, dir) -> bool, rev "base" or "head". Returns a dict."""
    vms = vm_tests(flake_text) or []
    traced = vms + BROWSER + list(TARGETS)
    why = {}
    out = {"docs_only": False, "all": False}
    code = [(s, p) for s, p in changes if not (DOCS.match(p) and p not in docs_named)]
    if changes and not code:
        out.update(
            docs_only=True,
            nix=False,
            rust=False,
            app=False,
            chosen=[],
            why={},
            vm_tests=[],
            installer=False,
            reason=None,
            **{g: [] for g in GROUPS},
        )
        return out
    allr = None
    for s, p in code:
        if FOUNDATION.match(p):
            allr = f"{p}: under everything"
            break
    c = covers_head
    if allr is None and not vms:
        allr = "flake.nix: no vmTests list to read"
    unknown = []
    chosen = set()
    if allr is None:
        for s, p in code:
            hit = touching(
                c, s, p, lambda d: exists("base", d), lambda d: exists("head", d)
            )
            hit |= touching(
                covers_base,
                s,
                p,
                lambda d: exists("base", d),
                lambda d: exists("head", d),
            )
            hit &= set(traced)
            for t in hit:
                why.setdefault(t, p)
            chosen |= hit
            # claimed by a check that reads it or its directory, or by a
            # job's path list; a listing above it alone does not claim it
            # (a new directory at the top)
            d = posixpath.dirname(p)
            direct = any(
                cv["files"].get(p) or (d not in FILE_LEVEL and cv["dirs"].get(d))
                for cv in (c, covers_base)
            )
            claimed = (
                direct or NIX.match(p) or RUST.match(p) or APP.match(p) or DOCS.match(p)
            )
            if not claimed:
                unknown.append(p)
        if unknown:
            allr = f"{unknown[0]}: no check claims it"
    if allr is None:
        # a check covers.json does not know yet, or whose entries changed
        for t in traced:
            if t not in c["checks"]:
                chosen.add(t)
                why.setdefault(t, "not in covers.json yet")
            elif of_check(c, t) != of_check(covers_base, t):
                chosen.add(t)
                why.setdefault(t, "its covers.json entries changed")
    if allr is not None:
        chosen = set(traced)
        why = {t: allr for t in traced}
    paths = [p for _, p in code]
    nix = (
        any(NIX.match(p) or p in docs_named for p in paths)
        or bool(chosen)
        or allr is not None
    )
    out.update(
        all=allr is not None,
        nix=nix,
        rust=allr is not None or any(RUST.match(p) for p in paths),
        app=allr is not None
        or any(
            APP.match(p) or any(p.startswith(d + "/") for d in app_dirs) for p in paths
        ),
        chosen=sorted(chosen),
        why=why,
        reason=allr,
    )
    out["vm_tests"] = [t for t in vms if t in chosen]
    for g, ts in GROUPS.items():
        out[g] = [t for t in ts if t in chosen]
    out["installer"] = "installer" in chosen
    return out


def name_status(base, head, cwd=None):
    out = git("diff", "--name-status", "--no-renames", "-z", base, head, cwd=cwd)
    parts = out.split("\0")
    changes = []
    for i in range(0, len(parts) - 1, 2):
        s, p = parts[i][0], parts[i + 1]
        changes.append(("M" if s in "MT" else s, p))
    return changes


def dir_exists(rev, d, cwd=None):
    if d == "":
        return True
    r = subprocess.run(
        ["git", "cat-file", "-t", f"{rev}:{d}"], capture_output=True, text=True, cwd=cwd
    )
    return r.returncode == 0 and r.stdout.strip() == "tree"


def show(rev, path, cwd=None):
    r = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True, cwd=cwd)
    return r.stdout.decode(errors="replace") if r.returncode == 0 else ""


def at_rev(rev, cwd=None):
    # read(path) and isdir(path) at a commit, for static_refs
    blobs = set(git("ls-tree", "-r", "--name-only", rev, cwd=cwd).split("\n"))
    trees = set(git("ls-tree", "-r", "-d", "--name-only", rev, cwd=cwd).split("\n"))
    return (lambda f: show(rev, f, cwd) if f in blobs else None), (lambda d: d in trees)


def covers_at(rev, checks, cwd=None, covers=None):
    # covers.json as committed at rev, with what the checks' .nix files name
    read, isdir = at_rev(rev, cwd)
    c = load_covers(show(rev, COVERS, cwd)) if covers is None else covers
    return add_static(c, checks, read, isdir)


def select_revs(base, head, cwd=None):
    changes = name_status(base, head, cwd)

    def read():
        files = git("ls-tree", "-r", "--name-only", head, cwd=cwd).split("\n")
        for f in files:
            if f.endswith((".nix", ".rs")):
                yield f, show(head, f, cwd)

    flake = show(head, "flake.nix", cwd)
    checks = traced_names(flake)
    return select(
        changes,
        covers_at(head, checks, cwd),
        covers_at(base, checks, cwd),
        flake,
        named_docs(read),
        lambda which, d: dir_exists(base if which == "base" else head, d, cwd),
        crate_closure(APP_ROOTS, lambda f: show(head, f, cwd)),
    )


def cmd_select(args):
    if args[0] in ("--nothing", "--everything"):
        # a draft or a commit already passed: nothing; not a pull request:
        # everything, untraced
        on = args[0] == "--everything"
        vms = vm_tests(open("flake.nix").read()) or []
        r = {
            "docs_only": False,
            "all": on,
            "reason": "not a pull request" if on else None,
            "nix": on,
            "rust": on,
            "app": on,
            "chosen": [],
            "why": {},
            "vm_tests": vms if on else [],
            "installer": on,
            **{g: ts if on else [] for g, ts in GROUPS.items()},
        }
    else:
        r = select_revs(args[0], args[1])
    if r["docs_only"]:
        print("docs only: style and lint, nothing else")
    elif r["all"]:
        print(f"everything: {r['reason']}")
    for t in r.get("chosen", []):
        print(f"  {t}: {r['why'][t]}")
    print(
        f"nix: {r['nix']}  rust: {r['rust']}  app: {r['app']}  installer: {r['installer']}  "
        f"vm tests: {len(r.get('vm_tests', []))}  "
        + "  ".join(f"{g}: {len(r.get(g, []))}" for g in GROUPS)
    )
    gh = os.environ.get("GITHUB_OUTPUT")
    if gh:
        with open(gh, "a") as f:
            for k in ("nix", "rust", "app", "installer"):
                f.write(f"{k}={'true' if r[k] else 'false'}\n")
            vm = r.get("vm_tests", [])
            f.write(f"vm={'true' if vm else 'false'}\n")
            f.write(f"vm_tests={json.dumps(vm)}\n")
            for g in GROUPS:
                f.write(f"{g}={json.dumps(r.get(g, []))}\n")
    return 0


# ---- tracing


def store_copy(rev, cwd=None):
    # the tree at rev, as a flake from a store path: nix reads its files
    # with plain opens (a git flake reads them from git's objects, where a
    # trace cannot see which), and hashes nothing it does not use
    tmp = tempfile.mkdtemp(prefix="ciselect-tree.")
    try:
        a = subprocess.Popen(["git", "archive", rev], stdout=subprocess.PIPE, cwd=cwd)
        subprocess.run(["tar", "-x", "-C", tmp], stdin=a.stdout, check=True)
        if a.wait() != 0:
            raise SystemExit(f"git archive {rev} failed")
        return subprocess.run(
            ["nix", "store", "add", "--name", "source", tmp],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    finally:
        shutil.rmtree(tmp)


FD = r"(?:AT_FDCWD|-?\d+)<([^>]*)>"
CALL = re.compile(
    r"^(openat2|openat|newfstatat|statx|faccessat2|faccessat)\("
    + FD
    + r', "((?:[^"\\]|\\.)*)"'
)
PCALL = re.compile(r'^(open|stat|lstat|access|readlink)\("((?:[^"\\]|\\.)*)"')
DENTS = re.compile(r"^getdents64\(\d+<([^>]*)>")


def parse_trace(files, root):
    reads, stats, lists = set(), set(), set()
    pre = root.rstrip("/") + "/"

    def rel(p):
        if p == root:
            return ""
        if p.startswith(pre):
            return posixpath.normpath(p[len(pre) :])
        return None

    for fn in files:
        for line in open(fn, errors="replace"):
            m = DENTS.match(line)
            if m:
                r = rel(m.group(1))
                if r is not None:
                    lists.add("" if r == "." else r)
                continue
            m = CALL.match(line)
            if m:
                call, d, name = m.groups()
                full = name if name.startswith("/") else (d + "/" + name if name else d)
            else:
                m = PCALL.match(line)
                if not m:
                    continue
                call, full = m.groups()
            r = rel(posixpath.normpath(full))
            if r is None or r == ".":
                continue
            ok = re.search(r"\) = (\d+)", line)
            if (
                call.startswith("open")
                and ok
                and "O_DIRECTORY" not in line
                and "O_PATH" not in line
            ):
                reads.add(r)
            else:
                stats.add(r)
    # a directory looked at is in lists or stats; a read file is not a stat
    return {"reads": reads, "stats": stats - reads, "lists": lists}


def trace_one(src, check, build=False, keep=None):
    tmp = tempfile.mkdtemp(prefix="ciselect-trace.")
    try:
        env = {**os.environ, "XDG_CACHE_HOME": os.path.join(tmp, "cache")}
        what = TARGETS[check][0] if check in TARGETS else f"checks.{SYSTEM}.{check}"
        attr = f"path:{src}#{what}"
        cmd = (
            ["nix", "build", "--no-link", "-L", attr]
            if build
            else ["nix", "eval", "--raw", attr + ".drvPath"]
        )
        r = subprocess.run(
            [
                "strace",
                "--seccomp-bpf",
                "-f",
                "-ff",
                "-qq",
                "-y",
                "-o",
                os.path.join(tmp, "t"),
                "-e",
                "trace=openat,openat2,open,newfstatat,statx,stat,lstat,access,faccessat,faccessat2,readlink,getdents64",
                *cmd,
            ],
            env=env,
            capture_output=not build,
            text=True,
        )
        files = [os.path.join(tmp, f) for f in os.listdir(tmp) if f.startswith("t.")]
        tr = parse_trace(files, src)
        tr["ok"] = r.returncode == 0
        tr["out"] = (r.stdout or "").strip() if not build else ""
        tr["err"] = (r.stderr or "")[-2000:] if not build else ""
        return tr
    finally:
        shutil.rmtree(tmp)


def cmd_trace(args):
    rev, only, jobs = "HEAD", None, 4
    out = COVERS
    while args:
        a = args.pop(0)
        if a == "--rev":
            rev = args.pop(0)
        elif a == "--only":
            only = args.pop(0).split(",")
        elif a == "--jobs":
            jobs = int(args.pop(0))
        elif a == "--out":
            out = args.pop(0)
    sha = git("rev-parse", rev).strip()
    checks = only or traced_names(show(sha, "flake.nix"))
    c = load_covers(open(out).read() if only and os.path.exists(out) else "")
    if only:
        for m in ("dirs", "files", "listed"):
            for p in list(c[m]):
                c[m][p] -= set(only)
    src = store_copy(sha)
    print(f"tracing {len(checks)} checks of {sha[:9]} ({src})", file=sys.stderr)

    def one(t):
        tr = trace_one(src, t)
        print(
            f"  {t}: {'ok' if tr['ok'] else 'FAILED'} {len(tr['reads'])} read",
            file=sys.stderr,
        )
        return t, tr

    bad = []
    with ThreadPoolExecutor(jobs) as ex:
        for t, tr in ex.map(one, checks):
            if not tr["ok"] and "does not provide attribute" in tr["err"]:
                continue
            if not tr["ok"]:
                bad.append(t)
                print(tr["err"], file=sys.stderr)
                continue
            add_trace(c, t, tr)
    open(out, "w").write(dump_covers(c, sha))
    return 1 if bad else 0


def main():
    if len(sys.argv) < 2:
        print(
            __doc__ or "select BASE HEAD | run CHECK... | trace [--only a,b] | matches",
            file=sys.stderr,
        )
        return 2
    cmd, args = sys.argv[1], sys.argv[2:]
    if cmd == "select":
        return cmd_select(args)
    if cmd == "trace":
        return cmd_trace(args)
    if cmd == "matches":
        print(json.dumps(sorted(vm_tests(open(args[0]).read()) or [])))
        return 0
    if cmd == "groups":
        print(json.dumps(sorted(GROUPS)))
        return 0
    print(f"unknown command {cmd}", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
