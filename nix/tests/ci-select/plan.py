#!/usr/bin/env python3
# What a pull request's run runs (docs/dev/ci.md, Choosing the tests): one
# answer per job, from the files the change touches. The same function
# decides a run in files_modified (ciselect.py) and every run of the
# history in the replay (history/replay.py), so what was measured is what
# runs.
#
# A job runs when the change touches what it tests: its set in
# tests-this.json (the code its assertions reach, read from the test, not
# its closure). Then:
#   - docs alone run nothing but style and lint;
#   - a component's insides whose face held (interfaces.py) reach that
#     component's own checks and nothing that merely reads it;
#   - names added to the registry run those tests, the registry itself
#     nothing else;
#   - a truly global change (flake.lock: nixpkgs, the Rust and Go
#     toolchains, every input) runs every job a pull request runs;
#   - a job on main-only.json (jobs that have never caught a real failure,
#     history/mainonly.py) and the cross-box builds (every box's system on
#     every builder, systems_agree, the installer pair) run in main's full
#     run alone, which publish requires.
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import interfaces  # noqa: E402

SETS = "nix/tests/ci-select/tests-this.json"
MAIN_ONLY = "nix/tests/ci-select/main-only.json"
REGISTRY = "nix/tests/registry.nix"

# The checks run by a browser or wrangler against fakes, not VMs, by the
# workflow job that runs them (each job's matrix is its list)
GROUPS = {
    # the Cloudflare Workers under wrangler: mail, DNS, sign-in (the id
    # Worker with the gate), the control plane's relay, witness and
    # validators, and its moves between releases
    "worker_tests": [
        "mail-worker",
        "dns-worker",
        "sso",
        "control",
        "control-relay",
        "control-upgrade",
        "control-together",
        "control-signfail",
    ],
    # the Mail, Files and Movies & TV pages as the app serves them, every bar link
    # and home tile staying in its tab, and Settings with a v1 or v2 entry
    "page_tests": ["mail-page", "files-page", "media-page", "same-tab", "entry-pages"],
    # the site as a browser meets it, through node1's nginx and the gate
    "site_checks": [
        "site-headers",
        "site-roles",
        "site-a11y",
        "site-speed",
        "site-links",
    ],
}
GROUPED = [t for ts in GROUPS.values() for t in ts]

# Under everything: flake.lock pins nixpkgs (the Rust and Go toolchains,
# every service), crane and the rust overlay. Nothing else is: a change to
# flake.nix, the workflow or a shared library reaches the jobs whose sets
# name it, and main's full run checks the rest.
GLOBAL = re.compile(r"^flake\.lock$")
# Read by no build: the docs (the gate serves them as they are), the notes
# at the top, the agents' skills, the GitHub workflow (lint reads it). A
# docs file a .nix or .rs names by path is not one.
DOCS = re.compile(r"^(docs/|\.claude/|\.github/|[^/]+\.md$|LICENSE$)")

# The jobs that are not one check each, by their key in tests-this.json
RUST = "rust"  # fmt over the workspace; each crate is rust:<its path>
APP = "app"  # the app's lint and tests (clippy-app, tests-app, net-check)
APP_EVAL = "app-release"  # the four downloads' recipes evaluate
FLAKE = "flake"  # every box's system evaluates
KEEP = "keep"  # inputs_kept
# a box's system, built once (node1, node2); a device with a set of its
# own (systems(home-controller)) by that
SYSTEMS = "systems"
INSTALLER = "installer"


def rx(globs):
    """one pattern for a set's paths: a plain path covers everything under
    it; * stays inside a directory, ** crosses them"""
    pats = []
    for g in globs:
        g = g.rstrip("/")
        if not any(c in g for c in "*?["):
            pats.append(re.escape(g) + r"(/.*)?")
        else:
            s = re.escape(g).replace(r"\*\*/", "(.*/)?").replace(r"\*\*", ".*")
            s = s.replace(r"\*", "[^/]*").replace(r"\?", "[^/]")
            pats.append(s)
    return re.compile("^(" + "|".join(pats) + ")$") if pats else None


class Sets:
    """tests-this.json, compiled: what each job tests"""

    def __init__(self, raw):
        self.raw = raw
        self.vm = list(raw.get("_vm", []))
        shared = raw.get("_shared_by_family", {})
        self.rx = {}
        self.shared = {}
        for k, v in raw.items():
            if k.startswith("_"):
                continue
            fam = v.get("family", "vm" if k in self.vm else "")
            self.rx[k] = rx(list(v["paths"]) + list(v.get("widened", {})))
            self.shared[k] = rx(shared.get(fam, []))

    def has(self, key):
        return key in self.rx

    def hits(self, key, files):
        r, s = self.rx.get(key), self.shared.get(key)
        return [f for f in files if (r and r.match(f)) or (s and s.match(f))]

    def claims(self, path):
        return any(self.hits(k, [path]) for k in self.rx)


def load_sets(text):
    return Sets(json.loads(text))


def load_main_only(text):
    return set(json.loads(text).get("jobs", {})) if text else set()


def registry_lists(text):
    """{list: [names]} of the registry, and the text with its names and
    comments taken out: what is left is the registry's own shape"""
    lists = {
        m.group(1): re.findall(r'"([^"]+)"', m.group(2))
        for m in re.finditer(r"\n\s*(\w+) = \[(.*?)\];", text, re.S)
    }
    shape = "\n".join(
        line
        for line in text.split("\n")
        if not re.fullmatch(r'\s*("[^"]+"|#.*)?\s*', line)
    )
    return lists, shape


def registered(base_text, head_text):
    """the names a change to the registry adds, when adding and removing
    names (and their comments) is all it does; None when it does more"""
    (b, bshape), (h, hshape) = registry_lists(base_text), registry_lists(head_text)
    if bshape != hshape or set(b) != set(h):
        return None
    return sorted({n for k in h for n in h[k]} - {n for k in b for n in b[k]})


def crate_of(key):
    return key[len("rust:") :] if key.startswith("rust:") else None


def plan(
    changes,
    sets,
    registry,
    main_only=(),
    faces=None,
    docs_named=(),
    registry_base=None,
    crates=None,
    hosts=None,
):
    """changes: [(status, path)]. sets: Sets. registry: {list: [names]} at
    the head (vm, files, eval). main_only: job keys that run in main's
    full run alone. faces: {component: [differences]} for the components
    whose insides changed ([] when the face held). registry_base: the
    registry's text at (base, head), to tell a new name from a reshaped
    file. crates: {crate path: package name}. hosts: {box or device: the
    key its system is built by}. Returns a dict: every job
    key chosen with why, those left to main's full run, and the workflow's
    outputs."""
    vm_names = list(registry.get("vm", []))
    eval_names = list(registry.get("eval", []))
    jobs = (
        vm_names
        + GROUPED
        + eval_names
        + [APP_EVAL, FLAKE, KEEP, RUST, APP, INSTALLER]
        + sorted(set((hosts or {}).values()))
        + sorted(k for k in sets.rx if k.startswith("rust:"))
    )
    out = {
        "docs_only": False,
        "global": None,
        "registered": [],
        "kept": [],
        "faces": faces or {},
        "why": {},
        "main_only": {},
        "unclaimed": [],
    }
    paths = [p for _, p in changes]
    code = [p for p in paths if not (DOCS.match(p) and p not in docs_named)]
    if changes and not code:
        out["docs_only"] = True
        return outputs(out, vm_names, eval_names, crates, hosts)
    why = {}

    def choose(key, reason):
        why.setdefault(key, reason)

    # names added to the registry: those tests, as new ones
    if registry_base and REGISTRY in code:
        added = registered(*registry_base)
        if added is not None:
            code = [p for p in code if p != REGISTRY]
            out["registered"] = added
            for n in added:
                choose(n, "registered in this change")
    # a component whose face held: its insides reach its own checks
    kept = sorted(n for n, d in (faces or {}).items() if not d)
    out["kept"] = kept
    inside = [p for p in code if interfaces.inside_of(p) in kept]
    for n in kept:
        for k in interfaces.COMPONENTS[n]["own"]:
            if any(interfaces.inside_of(p) == n for p in inside):
                choose(k, f"{n}'s own check (its face held)")
    reach = [p for p in code if p not in inside]
    g = next((p for p in reach if GLOBAL.match(p)), None)
    if g:
        out["global"] = f"{g}: under everything"
        for k in jobs:
            choose(k, out["global"])
    for k in jobs:
        if not sets.has(k):
            # a registered check with no set yet runs on every change
            # (checks.ci-select fails until it has one)
            choose(k, "no tests-this entry yet")
            continue
        h = sets.hits(k, reach)
        if h:
            choose(k, h[0])
    out["unclaimed"] = [p for p in reach if not sets.claims(p) and not DOCS.match(p)]
    # main's full run alone: the jobs that never caught a real failure,
    # and the installer (built twice on every builder and compared)
    for k in list(why):
        if k in main_only or k == INSTALLER:
            out["main_only"][k] = why.pop(k)
    out["why"] = why
    return outputs(out, vm_names, eval_names, crates, hosts)


def outputs(out, vm_names, eval_names, crates, hosts):
    """the workflow's view of a plan"""
    why = out["why"]
    vm = [t for t in vm_names if t in why]
    crate_keys = sorted(k for k in why if k.startswith("rust:"))
    names = crates or {}
    out.update(
        vm_tests=vm,
        # the programs the VM tests carry, built once for them
        programs=bool(vm),
        eval_checks=[t for t in eval_names if t in why]
        + ([APP_EVAL] if APP_EVAL in why else []),
        # every box's system evaluated (a pull request builds none of them
        # but the home controller's, below)
        systems_eval=FLAKE in why,
        keep=KEEP in why,
        rust_fmt=RUST in why or bool(crate_keys),
        rust_crates=sorted({names.get(crate_of(k), crate_of(k)) for k in crate_keys}),
        app=APP in why,
        # each built once, on any runner; every box's on every builder is
        # main's full run's
        hosts=sorted(h for h, k in (hosts or {}).items() if k in why),
        installer=False,
    )
    for g, ts in GROUPS.items():
        out[g] = [t for t in ts if t in why]
    out["flake"] = out["systems_eval"] or bool(out["eval_checks"])
    out["rust"] = out["rust_fmt"]
    return out


def everything(registry, crates):
    """main's full run: every job, every box's system on every builder"""
    vm = list(registry.get("vm", []))
    out = {
        "docs_only": False,
        "global": "not a pull request",
        "registered": [],
        "kept": [],
        "faces": {},
        "main_only": {},
        "unclaimed": [],
        "why": {},
        "vm_tests": vm,
        "programs": True,
        "eval_checks": list(registry.get("eval", [])) + [APP_EVAL],
        "systems_eval": True,
        "keep": True,
        "rust_fmt": True,
        "rust_crates": sorted(crates.values()),
        "app": True,
        "hosts": [],
        "installer": True,
        "flake": True,
        "rust": True,
    }
    for g, ts in GROUPS.items():
        out[g] = list(ts)
    return out


def nothing():
    """a draft, or an edit to a commit that already passed"""
    out = outputs(
        {
            "docs_only": False,
            "global": None,
            "registered": [],
            "kept": [],
            "faces": {},
            "main_only": {},
            "unclaimed": [],
            "why": {},
        },
        [],
        [],
        {},
        {},
    )
    return out


def host_keys(boxes, devices, sets):
    """{box or device: the key its system is built by}: a device with its
    own set (the home controller) by that, every box by the boxes' one"""
    out = {b: SYSTEMS for b in boxes}
    for d in devices:
        out[d] = f"systems({d})" if sets.has(f"systems({d})") else SYSTEMS
    return out
