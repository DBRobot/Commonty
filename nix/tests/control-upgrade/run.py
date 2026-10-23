# Release 359's control plane switched to this release's while it decides.
# Validators node1, node2 and the witness, and node3, which only reads, all
# start on 359's programs; each switches to this release's in turn, a gap
# between, in two orders. Throughout: decisions keep coming (the longest
# wait is measured and bounded), every box at a height holds the same
# state as every other there, old and new alike, and svc/db stays with its
# holder at its epoch. Release 359's config has no rule for svc/next and
# this one's has: it is granted only once version 2 is agreed, and that is
# only once every validator and every live box runs this release.
import base64, json, os, shutil, subprocess, sys, threading, time, urllib.request

from cryptography.hazmat.primitives.asymmetric import ec, ed25519
from cryptography.hazmat.primitives import serialization as S

T = os.getcwd()
failed = []


def ok(c, m):
    print(("PASS " if c else "FAIL ") + m, flush=True)
    if not c:
        failed.append(m)


def wait(what, cond, timeout=90):
    end = time.time() + timeout
    while time.time() < end:
        if cond():
            return True
        time.sleep(0.5)  # sleep-ok: polling the boxes' /state
    ok(False, f"timed out: {what}")
    return False


# the fleet: keys, and the two releases' configs
cfg = {
    "boxes": {},
    "validators": [],
    "rules": {
        "leaseTtlMs": 15000,
        "goneAfterMs": 10000,
        "leases": {"svc/db": {"eligible": ["node2", "node3"]}},
    },
}
for i, n in enumerate(["node1", "node2", "node3"], start=1):
    k = ec.generate_private_key(ec.SECP256R1())
    open(f"{T}/{n}.p8", "wb").write(
        k.private_bytes(S.Encoding.DER, S.PrivateFormat.PKCS8, S.NoEncryption())
    )
    pub = k.public_key().public_bytes(
        S.Encoding.DER, S.PublicFormat.SubjectPublicKeyInfo
    )
    cfg["boxes"][n] = {
        "id": 100 + i,
        "house": f"h{i}",
        "key": base64.b64encode(pub).decode(),
    }
w = ed25519.Ed25519PrivateKey.generate()
raw = lambda k: base64.b64encode(
    k.public_bytes(S.Encoding.Raw, S.PublicFormat.Raw)
).decode()
validators = {}
for i in (1, 2):
    os.makedirs(f"{T}/keys{i}", exist_ok=True)
    validators[i] = (
        subprocess.check_output(
            [f"{T}/new/dd-control", "-print-validator-key", "-state", f"{T}/keys{i}"]
        )
        .decode()
        .strip()
    )
    cfg["validators"].append({"id": i, "name": f"node{i}", "key": validators[i]})
cfg["validators"].append({"id": 3, "name": "cloudflare", "key": raw(w.public_key())})
old_cfg = cfg
new_cfg = json.loads(json.dumps(cfg))
new_cfg["rules"]["leases"]["svc/next"] = {"eligible": ["node1"]}
new_cfg["before"] = old_cfg
json.dump(old_cfg, open(f"{T}/old/config.json", "w"))
json.dump(new_cfg, open(f"{T}/new/config.json", "w"))
witness_key = base64.b64encode(
    w.private_bytes(S.Encoding.Raw, S.PrivateFormat.Raw, S.NoEncryption())
).decode()
placement = open(os.environ["PLACEMENT"]).read()
open(f"{T}/placement.json", "w").write(placement)
for i in (1, 2, 3):
    json.dump(
        {"db": ["db.service"], "next": ["next.service"]},
        open(f"{T}/carries{i}.json", "w"),
    )
os.makedirs(f"{T}/bin", exist_ok=True)
with open(f"{T}/bin/systemctl", "w") as f:
    f.write('#!/bin/sh\necho "$@" >> "$SYSTEMCTL_LOG"\n')
os.chmod(f"{T}/bin/systemctl", 0o755)


class Fleet:
    def __init__(self, name, port, listen):
        self.name, self.port, self.listen = name, port, listen
        self.dir = f"{T}/{name}"
        os.makedirs(self.dir)
        self.procs, self.which, self.relay = {}, {}, None
        self.samples = []
        self.stop_watch = False
        for i in (1, 2):
            shutil.copytree(f"{T}/keys{i}", f"{self.dir}/st{i}")
        os.makedirs(f"{self.dir}/st3")
        self.w = f"{self.dir}/w"
        os.makedirs(self.w)
        for f in ("worker.js", "wasm_exec.js"):
            shutil.copy(f"{T}/{f}", self.w)
        open(f"{self.w}/placement.js", "w").write(f"export default {placement};\n")
        open(f"{self.w}/.dev.vars", "w").write(f"WITNESS_KEY={witness_key}\n")
        open(f"{self.w}/wrangler.toml", "w").write(
            "\n".join(
                [
                    f'name = "control-upgrade-{name}"',
                    'main = "worker.js"',
                    'compatibility_date = "2026-05-01"',
                    "no_bundle = true",
                    "rules = [",
                    '  { type = "CompiledWasm", globs = ["**/*.wasm"] },',
                    '  { type = "ESModule", globs = ["**/*.js"] },',
                    "]",
                    "[[durable_objects.bindings]]",
                    'name = "RELAY"',
                    'class_name = "Relay"',
                    "[[migrations]]",
                    'tag = "v1"',
                    'new_sqlite_classes = ["Relay"]',
                    "",
                ]
            )
        )

    # the relay with the witness of one release; its saved state is kept
    # across a switch, as a deploy keeps the Durable Object's storage
    def start_relay(self, rel):
        shutil.copy(f"{T}/{rel}/witness.wasm", self.w)
        conf = json.load(open(f"{T}/{rel}/config.json"))
        open(f"{self.w}/config.js", "w").write(f"export default {json.dumps(conf)};\n")
        self.relay = subprocess.Popen(
            ["wrangler", "dev", "--port", str(self.port), "--ip", "127.0.0.1"],
            cwd=self.w,
            stdout=open(f"{self.dir}/wrangler.log", "a"),
            stderr=subprocess.STDOUT,
        )
        self.which["witness"] = rel
        wait(f"{self.name}: relay up", lambda: self.health().get("witness"))

    def stop_relay(self):
        self.relay.terminate()
        self.relay.wait()

    def health(self):
        try:
            return json.load(
                urllib.request.urlopen(
                    f"http://127.0.0.1:{self.port}/health", timeout=2
                )
            )
        except Exception:
            return {}

    def start_box(self, i, rel):
        env = dict(
            os.environ,
            PATH=f"{T}/bin:" + os.environ["PATH"],
            SYSTEMCTL_LOG=f"{self.dir}/systemctl{i}.log",
        )
        self.procs[i] = subprocess.Popen(
            [
                f"{T}/{rel}/dd-control",
                "-config",
                f"{T}/{rel}/config.json",
                "-box",
                f"node{i}",
                "-relay",
                f"ws://127.0.0.1:{self.port}/relay",
                "-state",
                f"{self.dir}/st{i}",
                "-box-key",
                f"{T}/node{i}.p8",
                "-listen",
                f"127.0.0.1:{self.listen + i}",
                "-heartbeat",
                "1s",
                "-placement",
                f"{T}/placement.json",
                "-placed-carries",
                f"{T}/carries{i}.json",
                "-placed-dir",
                f"{self.dir}/placed{i}",
            ],
            env=env,
            stdout=open(f"{self.dir}/n{i}.log", "a"),
            stderr=subprocess.STDOUT,
        )
        self.which[i] = rel

    def stop_box(self, i):
        self.procs[i].terminate()
        self.procs[i].wait()

    def state(self, i):
        try:
            return json.load(
                urllib.request.urlopen(
                    f"http://127.0.0.1:{self.listen + i}/state", timeout=2
                )
            )
        except Exception:
            return {}

    # every half second: each box's state, as (time, box, its release, state)
    def watch(self):
        while not self.stop_watch:
            now = time.time()
            for i in (1, 2, 3):
                s = self.state(i)
                if s.get("height"):
                    self.samples.append((now, i, self.which.get(i), s))
            time.sleep(0.5)  # sleep-ok: the sampling beat

    def switch(self, who):
        if who == "witness":
            self.stop_relay()
            self.start_relay("new")
        else:
            self.stop_box(who)
            self.start_box(who, "new")

    def height(self):
        return max((self.state(i).get("height", 0) for i in (1, 2, 3)), default=0)

    def run(self, order):
        self.start_relay("old")
        for i in (1, 2, 3):
            self.start_box(i, "old")
        db = lambda: self.state(2).get("leases", {}).get("svc/db") or {}
        wait(
            f"{self.name}: all on 359 agree, svc/db held",
            lambda: db().get("holder") in ("node2", "node3"),
        )
        held = (db()["holder"], db()["epoch"])
        self.held_at = self.state(2)["height"]
        print(f"{self.name}: svc/db with {held[0]} at epoch {held[1]}", flush=True)
        t = threading.Thread(target=self.watch)
        t.start()
        phases = []
        h = self.height()
        wait(f"{self.name}: decisions on 359", lambda: self.height() > h + 5)
        for who in order:
            t0 = time.time()
            self.switch(who)
            # the gap before the next: decisions come on, with every box up
            h = self.height()
            wait(
                f"{self.name}: decisions after {who} switched",
                lambda: self.height() > h + 10,
            )
            phases.append((who, t0, time.time()))
            agreed = [self.state(i).get("machine") for i in (1, 2, 3)]
            ok(
                who == order[-1] or not any(agreed),
                f"{self.name}: nothing new agreed with {who} switched and {order[order.index(who) + 1 :]} not",
            )
        wait(
            f"{self.name}: version 2 agreed",
            lambda: all(self.state(i).get("machine") == 2 for i in (1, 2, 3)),
        )
        nxt = lambda: self.state(1).get("leases", {}).get("svc/next") or {}
        wait(
            f"{self.name}: svc/next granted under the new release's rules",
            lambda: nxt().get("holder") == "node1",
        )
        h = self.height()
        wait(f"{self.name}: decisions after", lambda: self.height() > h + 5)
        self.stop_watch = True
        t.join()
        for i in (1, 2, 3):
            self.stop_box(i)
        self.stop_relay()
        self.judge(held, phases)

    def judge(self, held, phases):
        # one state per height, whichever release computed it
        by_height, split = {}, []
        for _, i, rel, s in self.samples:
            key = json.dumps(s, sort_keys=True)
            prev = by_height.setdefault(s["height"], (i, rel, key))
            if prev[2] != key:
                split.append((s["height"], prev[:2], (i, rel)))
        ok(
            not split,
            f"{self.name}: every box holds the same state at each height ({len(by_height)} heights; split {split[:3]})",
        )
        olds = {s["height"] for _, _, rel, s in self.samples if rel == "old"}
        news = {s["height"] for _, _, rel, s in self.samples if rel == "new"}
        ok(
            len(olds & news) > 10,
            f"{self.name}: {len(olds & news)} heights held alike by 359 and this release",
        )
        # svc/db: with its holder at its epoch all through
        lease = lambda s: s.get("leases", {}).get("svc/db") or {}
        dbe = {
            (lease(s).get("holder"), lease(s).get("epoch"))
            for _, _, _, s in self.samples
            if s["height"] >= self.held_at
        }
        lapsed = sorted(
            {
                s["height"]
                for _, _, _, s in self.samples
                if s["height"] >= self.held_at and not lease(s)
            }
        )
        ok(
            dbe == {held},
            f"{self.name}: svc/db held by {held[0]} at epoch {held[1]} from height {self.held_at} on (saw {sorted(dbe, key=str)}, none at {lapsed[:5]})",
        )
        # svc/next: never before version 2
        early = [
            s["height"]
            for _, _, _, s in self.samples
            if s.get("leases", {}).get("svc/next") and s.get("machine") != 2
        ]
        ok(
            not early,
            f"{self.name}: svc/next granted only under version 2 (early at {early[:3]})",
        )
        before2 = [s["height"] for _, _, _, s in self.samples if s.get("machine")]
        first2 = min(before2) if before2 else None
        last_switch = phases[-1][1]
        at2 = min(
            (t for t, _, _, s in self.samples if s.get("machine") == 2), default=None
        )
        ok(
            at2 is not None and at2 >= last_switch,
            f"{self.name}: version 2 agreed at height {first2}, {at2 - last_switch:.1f}s after the last switch"
            if at2
            else f"{self.name}: version 2 never agreed",
        )
        # the longest wait between two decisions, by phase
        times = sorted({(round(t, 1), s["height"]) for t, _, _, s in self.samples})
        for who, t0, t1 in phases:
            best, last_h, last_t = 0.0, None, t0
            for t, h in times:
                if t < t0 or t > t1:
                    continue
                if last_h is None or h > last_h:
                    best = max(best, t - last_t)
                    last_h, last_t = h, t
            limit = 30 if who == "witness" else 10
            ok(
                best <= limit,
                f"{self.name}: {who} switched; longest wait for a decision {best:.1f}s (at most {limit}s)",
            )


Fleet("a", 8791, 4290).run([1, "witness", 2, 3])
Fleet("b", 8792, 4390).run([3, 2, "witness", 1])
if failed:
    print("FAILED:", *failed, sep="\n  ")
    for d in ("a", "b"):
        for f in sorted(os.listdir(f"{T}/{d}")):
            if f.endswith(".log") and not f.startswith("systemctl"):
                print(f"--- {d}/{f}")
                print("".join(open(f"{T}/{d}/{f}").readlines()[-15:]))
    sys.exit(1)
