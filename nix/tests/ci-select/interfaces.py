#!/usr/bin/env python3
# Each component's insides and its outward face (docs/dev/ci.md,
# Interfaces). A pull request that changes only a component's insides, and
# leaves its face byte for byte the same (and every Go package API apidiff
# compares the same), runs that component's own tests and its contract
# tests; the checks that merely read it (the VM tests above it) are left
# out. A face that changed walks out to every job whose set in
# tests-this.json names the component's files.
#
#   interfaces.py compare BASE HEAD    which components kept their face
#   interfaces.py face NAME DIR        one component's face, from a tree
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))

COMPONENTS = {
    # the gate (servers/gate): its HTTP routes with what each handler takes
    # and answers, the environment it reads, the box paths and ports it
    # names (faces/rust_routes.py). Its pages (templates/, web/) are not
    # insides: they are what a browser meets, chosen by the jobs' sets.
    "gate": {
        "inside": r"^servers/gate/src/.+\.rs$",
        "faces": [("rust-routes", "servers/gate/src")],
        # what runs when its insides change and its face holds, by its key
        # in tests-this.json: its crate's tests and lint and the cli's (whose
        # end to end runs a gate in-process), fmt; the real gate behind
        # node1's nginx, asked as every kind of person; the id Worker
        # signing in through it
        "own": [
            "rust",
            "rust:servers/gate",
            "rust:client/cli",
            "site-headers",
            "site-roles",
            "sso",
        ],
    },
    # the control plane (servers/system/control): what crosses the wire and
    # the HTTP routes, flags, files and ports of its commands
    # (faces/goface), and the exported API of the packages whose types are
    # the agreed state and the relay's frames (apidiff). testdata holds the
    # judge's vectors, which the Rust signers are held to: not insides.
    "control": {
        "inside": r"^servers/system/control/(cmd|place|state|validator|wire|third_party)/(?!(.*/)?testdata/).+\.go$",
        "faces": [
            ("goface", "servers/system/control"),
            (
                "apidiff",
                "servers/system/control",
                ["state", "state/computers", "state/ident", "wire"],
            ),
        ],
        # every package's go tests, and the relay, witness and three
        # validators under wrangler; the relay without its witness, and the
        # moves between releases
        "own": [
            "control",
            "control-relay",
            "control-upgrade",
            "control-together",
            "control-signfail",
        ],
    },
}


def inside_of(path):
    for name, c in COMPONENTS.items():
        if re.match(c["inside"], path):
            return name
    return None


def run(cmd, cwd=None, env=None):
    r = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, text=True)
    if r.returncode != 0:
        raise RuntimeError(f"{' '.join(cmd)}: {r.stderr.strip()[-800:]}")
    return r.stdout


def face_text(kind, tree, d):
    src = os.path.join(tree, d)
    if not os.path.isdir(src):
        return ""
    if kind == "rust-routes":
        return run([sys.executable, os.path.join(HERE, "faces", "rust_routes.py"), src])
    if kind == "goface":
        return run(["goface", src])
    raise ValueError(kind)


def go_env(tree, d, vendor):
    # apidiff loads the packages with the go tool: offline, from the
    # vendored modules (the dd-control build's), or from the module cache
    # when no vendor is given (a replay on a laptop)
    env = {**os.environ, "GOTOOLCHAIN": "local", "CGO_ENABLED": "0"}
    env.setdefault("GOCACHE", os.path.join(tree, ".gocache"))
    if vendor:
        dst = os.path.join(tree, d, "vendor")
        if not os.path.exists(dst):
            shutil.copytree(vendor, dst)
        env["GOFLAGS"] = "-mod=vendor"
        env["GOPROXY"] = "off"
    else:
        env["GOFLAGS"] = "-mod=mod"
    return env


def module_path(tree, d):
    for line in open(os.path.join(tree, d, "go.mod")):
        if line.startswith("module "):
            return line.split()[1]
    raise ValueError(f"{d}/go.mod names no module")


def apidiff(base, head, d, pkgs, vendor):
    """what apidiff says changed in each package's exported API, base to
    head; a module whose requirements moved counts as changed"""
    gm = [os.path.join(t, d, f) for t in (base, head) for f in ("go.mod", "go.sum")]
    if any(not os.path.exists(p) for p in gm):
        return ["go.mod or go.sum is missing at one end"]
    if (
        open(gm[0]).read() != open(gm[2]).read()
        or open(gm[1]).read() != open(gm[3]).read()
    ):
        return ["the module's requirements changed (go.mod, go.sum)"]
    out = []
    tmp = tempfile.mkdtemp(prefix="apidiff.")
    try:
        for p in pkgs:
            ends = []
            for name, tree in (("base", base), ("head", head)):
                if not os.path.isdir(os.path.join(tree, d, p)):
                    ends.append(None)
                    continue
                f = os.path.join(tmp, f"{name}-{p.replace('/', '_')}")
                run(
                    ["apidiff", "-w", f, f"{module_path(tree, d)}/{p}"],
                    cwd=os.path.join(tree, d),
                    env=go_env(tree, d, vendor),
                )
                ends.append(f)
            if None in ends:
                if ends != [None, None]:
                    out.append(f"{p}: the package came or went")
                continue
            said = run(["apidiff", *ends]).strip()
            if said:
                out.append(f"{p}: {said}")
    finally:
        shutil.rmtree(tmp)
    return out


def differences(name, base, head, vendor=None):
    """how a component's face differs between two trees ([] when it is the same)"""
    out = []
    for f in COMPONENTS[name]["faces"]:
        kind, d = f[0], f[1]
        if kind == "apidiff":
            out += [f"apidiff {x}" for x in apidiff(base, head, d, f[2], vendor)]
        elif face_text(kind, base, d) != face_text(kind, head, d):
            out.append(f"{kind} {d}: the face changed")
    return out


def tree_at(rev, paths, cwd=None):
    """a temporary directory holding paths as they are at rev"""
    tmp = tempfile.mkdtemp(prefix="face-tree.")
    have = [
        p
        for p in paths
        if subprocess.run(
            ["git", "cat-file", "-e", f"{rev}:{p}"], cwd=cwd, capture_output=True
        ).returncode
        == 0
    ]
    if have:
        a = subprocess.Popen(
            ["git", "archive", rev, "--", *have], stdout=subprocess.PIPE, cwd=cwd
        )
        subprocess.run(["tar", "-x", "-C", tmp], stdin=a.stdout, check=True)
        if a.wait() != 0:
            raise RuntimeError(f"git archive {rev} failed")
    return tmp


def control_vendor():
    # the vendored modules the dd-control build uses, from the store (no
    # network in CI); a replay sets FACES_GO_MODULES=mod to use the cache
    v = os.environ.get("FACES_GO_MODULES")
    if v == "mod":
        return None
    if v:
        return v
    return run(
        [
            "nix",
            "build",
            "--no-link",
            "--print-out-paths",
            ".#packages.x86_64-linux.dd-control.goModules",
        ]
    ).strip()


def stable(changes, base, head, cwd=None):
    """{component: []} for each component whose insides the change touches
    and whose face it leaves as it was, and {component: [why]} for the
    others; a failure to read a face counts as a change"""
    touched = sorted({inside_of(p) for _, p in changes} - {None})
    out = {}
    for name in touched:
        dirs = sorted({f[1] for f in COMPONENTS[name]["faces"]})
        b = h = None
        try:
            b, h = tree_at(base, dirs, cwd), tree_at(head, dirs, cwd)
            vendor = (
                control_vendor()
                if any(f[0] == "apidiff" for f in COMPONENTS[name]["faces"])
                else None
            )
            out[name] = differences(name, b, h, vendor)
        except (RuntimeError, OSError, ValueError) as e:
            out[name] = [f"its face could not be read: {e}"]
        finally:
            for t in (b, h):
                if t:
                    shutil.rmtree(t, ignore_errors=True)
    return out


def main():
    if len(sys.argv) >= 4 and sys.argv[1] == "compare":
        sys.path.insert(0, HERE)
        import ciselect

        changes = ciselect.name_status(sys.argv[2], sys.argv[3])
        r = stable(changes, sys.argv[2], sys.argv[3])
        for name, why in r.items():
            print(f"{name}: {'face unchanged' if not why else '; '.join(why)}")
        if "--json" in sys.argv:
            print(json.dumps(r))
        return 0
    if len(sys.argv) == 4 and sys.argv[1] == "face":
        for f in COMPONENTS[sys.argv[2]]["faces"]:
            if f[0] != "apidiff":
                sys.stdout.write(face_text(f[0], sys.argv[3], f[1]))
        return 0
    print(__doc__ or "compare BASE HEAD | face NAME DIR", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
