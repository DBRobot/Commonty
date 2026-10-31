# The control plane of the release the fleet runs (nix/tests/control-upgrade.nix,
# its state machine version 2, called "359" below where it was that first)
# switched to this release's while it decides. Validators node1, node2 and
# the witness, and node3, which only reads, all start on the old programs;
# each switches to this release's in turn, a gap between, in two orders. Throughout: decisions keep coming (the longest
# wait is measured and bounded), every box at a height holds the same
# state as every other there, old and new alike, and svc/db stays with its
# holder at its epoch. Release 359's config has no rule for svc/next and
# this one's has: it is granted only once version 3 is agreed, and that is
# only once every validator and every live box runs this release.
#
# SCENARIO=together is release 360's switch of 2026-10-24 instead: the
# witness in the config but off (a relay with no witness key), node1 and
# node2 on 359 with 359's config, which has no svc/* lease, switched to this
# release together. Decisions come again within ADVANCE s, and the leased
# writers (postgresql, forgejo) run on the box holding their lease within
# the lease's 45 s and a margin of the switch.
import base64, copy, json, os, shutil, subprocess, sys, threading, time, urllib.request

from cryptography.hazmat.primitives.asymmetric import ec, ed25519
from cryptography.hazmat.primitives import serialization as S

T = os.getcwd()
failed = []
# the state machine each release runs (state/machine.go)
OLD, NEW = 2, 3


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
        # longer than the 30s a witness switch may go without a decision
        # (wrangler's restart, 13s under load): a lease expires by the
        # clock at the first decision after, and svc/db must outlast it
        "leaseTtlMs": 45000,
        "goneAfterMs": 90000,
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
# attestation's config, new with version 3: no maker's root in a test
new_cfg["attest"] = {"roots": {}}
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

# the together scenario's: the two boxes, 359's config with no lease for
# the writers, and this release's with them; the witness in both
WRITERS = ("postgresql", "forgejo")
t_old = copy.deepcopy(old_cfg)
del t_old["boxes"]["node3"]
t_old["rules"]["leases"] = {}
t_new = copy.deepcopy(t_old)
t_new["rules"]["leases"] = {
    f"svc/{w}": {"eligible": ["node1", "node2"]} for w in WRITERS
}
t_new["before"] = t_old
json.dump(t_old, open(f"{T}/old/config-together.json", "w"))
json.dump(t_new, open(f"{T}/new/config-together.json", "w"))
t_place = json.loads(placement)
t_place["boxes"] = {
    n: dict(t_place["boxes"]["node2"], site=f"h{i}", can={w: 1 for w in WRITERS})
    for i, n in ((1, "node1"), (2, "node2"))
}
t_place["services"] = {
    w: dict(t_place["services"]["db"], placed=True, lease=True) for w in WRITERS
}
open(f"{T}/placement-together.json", "w").write(json.dumps(t_place))
for i in (1, 2):
    json.dump(
        {w: [f"{w}.service"] for w in WRITERS},
        open(f"{T}/carries-together{i}.json", "w"),
    )
# the signfail scenario's: the together scenario's fleet on this release
# alone, node2 suiting the writers better by score and unable to start
# postgres (SYSTEMCTL_FAILED: no base backup, as on 2026-10-24); forgejo
# needs postgres on its box
s_place = copy.deepcopy(t_place)
s_place["boxes"]["node2"]["can"] = {w: 1.5 for w in WRITERS}
s_place["services"]["forgejo"]["local"] = ["postgresql"]
s_place["buckets"] = {
    "postgres-wal": {"copies": 2, "service": "postgresql"},
    "forge": {"copies": 2, "service": "forgejo"},
}
open(f"{T}/placement-signfail.json", "w").write(json.dumps(s_place))
json.dump(t_new, open(f"{T}/new/config-signfail.json", "w"))
for i, wal in ((1, 156 << 20), (2, 759 << 10)):
    json.dump(
        {w: [f"{w}.service"] for w in WRITERS},
        open(f"{T}/carries-signfail{i}.json", "w"),
    )
    json.dump(
        {
            "store_size_bytes": 1000 << 30,
            "store_free_bytes": 900 << 30,
            "store/postgres-wal/bytes": wal,
            "store/forge/bytes": 2 << 30,
        },
        open(f"{T}/facts-signfail{i}.json", "w"),
    )
# the flap scenario's: the signfail fleet with both boxes signing, node2
# suiting the writers better by score and able to run them
open(f"{T}/placement-flap.json", "w").write(json.dumps(s_place))
json.dump(t_new, open(f"{T}/new/config-flap.json", "w"))
for i in (1, 2):
    shutil.copy(f"{T}/carries-signfail{i}.json", f"{T}/carries-flap{i}.json")
    shutil.copy(f"{T}/facts-signfail{i}.json", f"{T}/facts-flap{i}.json")
os.makedirs(f"{T}/bin", exist_ok=True)
# nothing it starts runs: is-active says not, as of a unit stopped. A unit
# SYSTEMCTL_FAILED names, or the file SYSTEMCTL_FAILED_FILE does, failed;
# the one SYSTEMCTL_SLOW names (unit:seconds) is still coming up that long
# after it was started (a restore), and the rest are up
with open(f"{T}/bin/systemctl", "w") as f:
    f.write(
        """#!/bin/sh
echo "$@" >> "$SYSTEMCTL_LOG"
failed() {
  case " ${SYSTEMCTL_FAILED:-} $(cat "${SYSTEMCTL_FAILED_FILE:-/dev/null}" 2>/dev/null) " in *" $1 "*) return 0 ;; esac
  return 1
}
case "$1" in
  start)
    shift
    for u in "$@"; do date +%s > "$SYSTEMCTL_LOG.start.$u"; done ;;
  is-active) exit 3 ;;
  is-failed)
    for u in "$@"; do failed "$u" && exit 0; done
    exit 1 ;;
  show)
    for u in "$@"; do :; done
    state=active job=
    if failed "$u"; then
      state=failed
    elif [ "${SYSTEMCTL_SLOW%%:*}" = "$u" ] && [ -e "$SYSTEMCTL_LOG.start.$u" ] \\
      && [ $(( $(date +%s) - $(cat "$SYSTEMCTL_LOG.start.$u") )) -lt "${SYSTEMCTL_SLOW#*:}" ]; then
      state=activating job="1 start"
    fi
    printf 'Type=notify\\nActiveState=%s\\nJob=%s\\n' "$state" "$job" ;;
esac
"""
    )
os.chmod(f"{T}/bin/systemctl", 0o755)


class Fleet:
    # boxes: the ones that run; witness: whether the relay has its key;
    # kind: which configs, placement and carries ("" or "-together")
    def __init__(self, name, port, listen, boxes=(1, 2, 3), witness=True, kind=""):
        self.name, self.port, self.listen = name, port, listen
        self.boxes, self.witness, self.kind = boxes, witness, kind
        self.dir = f"{T}/{name}"
        os.makedirs(self.dir)
        self.procs, self.which, self.relay = {}, {}, None
        self.samples, self.plans = [], []
        self.stop_watch = False
        for i in (1, 2):
            shutil.copytree(f"{T}/keys{i}", f"{self.dir}/st{i}")
        os.makedirs(f"{self.dir}/st3")
        self.w = f"{self.dir}/w"
        os.makedirs(self.w)
        for f in ("worker.js", "wasm_exec.js"):
            shutil.copy(f"{T}/{f}", self.w)
        plan = open(f"{T}/placement{kind}.json").read()
        open(f"{self.w}/placement.js", "w").write(f"export default {plan};\n")
        # no key: no witness here, as the relay ran from 2026-10-23
        if witness:
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
        conf = json.load(open(f"{T}/{rel}/config{self.kind}.json"))
        open(f"{self.w}/config.js", "w").write(f"export default {json.dumps(conf)};\n")
        self.relay = subprocess.Popen(
            ["wrangler", "dev", "--port", str(self.port), "--ip", "127.0.0.1"],
            cwd=self.w,
            stdout=open(f"{self.dir}/wrangler.log", "a"),
            stderr=subprocess.STDOUT,
        )
        self.which["witness"] = rel
        wait(
            f"{self.name}: relay up",
            lambda: (
                self.health().get("witness")
                if self.witness
                else "connected" in self.health()
            ),
        )

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
            # node2 cannot start postgres: no base backup (2026-10-24)
            SYSTEMCTL_FAILED="postgresql.service"
            if self.kind == "-signfail" and i == 2
            else "",
            # what the flap scenario fails as it goes, and postgres restoring
            # for a minute wherever it starts
            SYSTEMCTL_FAILED_FILE=f"{self.dir}/failed{i}",
            SYSTEMCTL_SLOW="postgresql.service:60" if self.kind == "-flap" else "",
        )
        self.procs[i] = subprocess.Popen(
            [
                f"{T}/{rel}/dd-control",
                "-config",
                f"{T}/{rel}/config{self.kind}.json",
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
                f"{T}/placement{self.kind}.json",
                "-placed-carries",
                f"{T}/carries{self.kind}{i}.json",
                "-placed-dir",
                f"{self.dir}/placed{i}",
            ]
            + (
                [
                    "-test-facts",
                    f"{T}/facts-signfail{i}.json",
                    "-test-sign-fail",
                    f"{self.dir}/sign-fail{i}",
                ]
                if self.kind == "-signfail"
                else []
            )
            + (["-test-facts", self.facts(i)] if self.kind == "-flap" else []),
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

    # every half second: each box's state, as (time, box, its release, state);
    # in the flap scenario each box's plan too, as (time, box, plan)
    def watch(self):
        while not self.stop_watch:
            now = time.time()
            for i in self.boxes:
                s = self.state(i)
                if s.get("height"):
                    self.samples.append((now, i, self.which.get(i), s))
                if self.kind == "-flap":
                    try:
                        plan = json.load(
                            urllib.request.urlopen(
                                f"http://127.0.0.1:{self.listen + i}/plan", timeout=2
                            )
                        )
                        self.plans.append((now, i, plan))
                    except Exception:
                        pass
            time.sleep(0.5)  # sleep-ok: the sampling beat

    # the flap scenario's facts a box adds to its heartbeat: its own file,
    # changed as the scenario goes (dd-drain's "draining")
    def facts(self, i):
        path = f"{self.dir}/facts{i}.json"
        if not os.path.exists(path):
            shutil.copy(f"{T}/facts-flap{i}.json", path)
        return path

    def drain(self, i, on):
        f = json.load(open(self.facts(i)))
        f["draining"] = 1 if on else 0
        json.dump(f, open(self.facts(i) + ".new", "w"))
        os.replace(self.facts(i) + ".new", self.facts(i))

    def switch(self, who):
        if who == "witness":
            self.stop_relay()
            self.start_relay("new")
        else:
            self.stop_box(who)
            self.start_box(who, "new")

    def height(self):
        return max((self.state(i).get("height", 0) for i in self.boxes), default=0)

    # a writer runs on box i: its flag there holds the lease's epoch, and
    # its unit was started there since the log was `mark` long
    def runs(self, i, svc, epoch, mark):
        try:
            flag = json.load(open(f"{self.dir}/placed{i}/{svc}"))
            log = open(f"{self.dir}/systemctl{i}.log").read()[mark:]
        except Exception:
            return False
        return flag.get("epoch") == epoch and any(
            line.startswith("start") and f"{svc}.service" in line.split()
            for line in log.splitlines()
        )

    # release 360's switch: both boxes on 359 deciding without the witness,
    # then both on this release at once
    def together(self, advance, ttl, margin):
        self.start_relay("new")
        for i in self.boxes:
            self.start_box(i, "old")
        h = self.height()
        wait(
            f"{self.name}: decisions on 359 without the witness",
            lambda: self.height() > h + 10,
            timeout=120,
        )
        s = self.state(1)
        ok(
            s.get("machine") in (None, OLD) and not s.get("leases"),
            f"{self.name}: on the old release nothing new is agreed and no writer's lease granted",
        )
        t = threading.Thread(target=self.watch)
        t.start()
        marks = {}
        h = self.height()
        for i in self.boxes:
            self.stop_box(i)
            try:
                marks[i] = os.path.getsize(f"{self.dir}/systemctl{i}.log")
            except OSError:
                marks[i] = 0
        t0 = time.time()
        for i in self.boxes:
            self.start_box(i, "new")
        if wait(
            f"{self.name}: decisions within {advance}s of the switch",
            lambda: self.height() > h + 5,
            timeout=advance,
        ):
            ok(True, f"{self.name}: decisions {time.time() - t0:.1f}s after the switch")

        def leases():
            got = self.state(1).get("leases") or {}
            return {w: got.get(f"svc/{w}") or {} for w in WRITERS}

        def up():
            return all(
                lease.get("holder") in ("node1", "node2")
                and self.runs(
                    int(lease["holder"][-1]),
                    w,
                    lease["epoch"],
                    marks[int(lease["holder"][-1])],
                )
                for w, lease in leases().items()
            )

        limit = ttl + margin
        if wait(
            f"{self.name}: {', '.join(WRITERS)} run on their lease's box within {limit}s of the switch",
            up,
            timeout=max(1, limit - (time.time() - t0)),
        ):
            held = ", ".join(
                "%s on %s at epoch %s" % (w, lease["holder"], lease["epoch"])
                for w, lease in leases().items()
            )
            ok(True, f"{self.name}: {held}, {time.time() - t0:.1f}s after the switch")
        h = self.height()
        wait(f"{self.name}: decisions after", lambda: self.height() > h + 5)
        self.stop_watch = True
        t.join()
        for i in self.boxes:
            self.stop_box(i)
        self.stop_relay()
        by_height, split = {}, []
        for _, i, rel, s in self.samples:
            key = json.dumps(s, sort_keys=True)
            prev = by_height.setdefault(s["height"], (i, rel, key))
            if prev[2] != key:
                split.append((s["height"], prev[:2], (i, rel)))
        ok(
            not split,
            f"{self.name}: both boxes hold the same state at each height ({len(by_height)} heights; split {split[:3]})",
        )
        early = [
            s["height"]
            for _, _, _, s in self.samples
            if any((s.get("leases") or {}).get(f"svc/{w}") for w in WRITERS)
            and s.get("machine") != NEW
        ]
        ok(
            not early,
            f"{self.name}: a writer's lease granted only under the new version (early at {early[:3]})",
        )

    # node1's TPM stops signing, as on 2026-10-24 23:52: heartbeats and
    # renewals fail. For 20 s nothing may move. For longer than a box
    # counts as alive, node2 may take the writers once node1 is gone, never
    # while node1, which held them, is alive; node2 cannot start postgres
    # and refuses it, and they come back to node1 once it signs again
    def signfail(self, short=20, long=110):
        self.start_relay("new")
        for i in self.boxes:
            self.start_box(i, "new")

        def leases():
            got = self.state(2).get("leases") or {}
            return {w: got.get(f"svc/{w}") or {} for w in WRITERS}

        def on(box, mark=0):
            return all(
                lease.get("holder") == box
                and self.runs(int(box[-1]), w, lease["epoch"], mark)
                for w, lease in leases().items()
            )

        wait(f"{self.name}: the writers run on node1", lambda: on("node1"), timeout=150)
        held = {w: (l["holder"], l["epoch"]) for w, l in leases().items()}
        t = threading.Thread(target=self.watch)
        t.start()
        # a short failure: renewals resume well within the lease
        fail = f"{self.dir}/sign-fail1"
        open(fail, "w").close()
        time.sleep(short)  # sleep-ok: the failure lasts this long
        os.remove(fail)
        time.sleep(10)  # sleep-ok: renewals agreed again
        seen = {
            (
                w,
                (s.get("leases") or {}).get(f"svc/{w}", {}).get("holder"),
                (s.get("leases") or {}).get(f"svc/{w}", {}).get("epoch"),
            )
            for _, _, _, s in self.samples
            for w in WRITERS
        }
        ok(
            seen == {(w, h, e) for w, (h, e) in held.items()},
            f"{self.name}: {short}s of failed signatures moved no lease (saw {sorted(seen, key=str)})",
        )
        ok(
            on("node1"),
            f"{self.name}: the writers still run on node1 after {short}s of failed signatures",
        )
        log = open(f"{self.dir}/n1.log").read()
        ok(
            log.count("tpm: a signature failed") == 1
            and log.count("tpm: signing again") == 1,
            f"{self.name}: one line when signing failed, one when it was back",
        )
        # a long failure: node1 is counted gone after 90 s
        self.samples.clear()
        open(fail, "w").close()
        t0 = time.time()
        time.sleep(long)  # sleep-ok: the failure lasts this long
        marks = {i: os.path.getsize(f"{self.dir}/systemctl{i}.log") for i in self.boxes}
        os.remove(fail)
        t1 = time.time()
        back = wait(
            f"{self.name}: the writers back on node1 once it signs again",
            lambda: on("node1", marks[1]),
            timeout=150,
        )
        if back:
            ok(
                True,
                f"{self.name}: the writers back on node1 {time.time() - t1:.1f}s after it signed again",
            )
        self.stop_watch = True
        t.join()
        # while node1 was alive, by the agreed state, no writer's lease
        # was granted to node2: at the first state with node2 holding it at
        # an epoch, node1 had gone unheard longer than a box counts alive
        first = {}
        for at, _, _, st in sorted(self.samples, key=lambda x: x[3]["height"]):
            for w in WRITERS:
                lease = (st.get("leases") or {}).get(f"svc/{w}") or {}
                if lease.get("holder") == "node2":
                    first.setdefault((w, lease.get("epoch")), (at, st))
        early = sorted(
            round(at - t0)
            for at, st in first.values()
            if st["time"] - (st.get("boxes") or {}).get("node1", {}).get("lastSeen", 0)
            <= 90000
        )
        ok(first != {}, f"{self.name}: node2 took the writers once node1 was gone")
        ok(
            not early,
            f"{self.name}: no writer's lease went to node2 while node1, which held them, was alive (at {early[:5]}s)",
        )
        log2 = open(f"{self.dir}/n2.log").read()
        ok(
            "postgresql failed to start here" in log2,
            f"{self.name}: node2 refused the postgres it could not start",
        )
        for i in self.boxes:
            self.stop_box(i)
        self.stop_relay()

    # 2026-10-25/26: agreement stalled past the lease's time again and
    # again; both boxes stopped their writers, the lapsed leases were gone
    # from the agreed state, and the plan gave the group to whichever box
    # suited it best, a box that took it over refused it seconds into its
    # restore, and two refusals left it nowhere. Here: node1 holds the
    # writers and node2 suits them better by score. A stall longer than the
    # lease gives them back to node1, and nothing moves for minutes after;
    # drained, node1 hands them to node2, whose postgres restores for a
    # minute and is not refused for it; and with node2 drained and node1
    # refusing postgres, the plan never leaves it nowhere
    # a box's log so far; one it has not written yet is empty
    def log(self, name):
        try:
            return open(f"{self.dir}/{name}").read()
        except FileNotFoundError:
            return ""

    def logsize(self, i):
        return len(self.log(f"systemctl{i}.log"))

    def flap(self, stall=60, observe=120):
        self.start_relay("new")
        # node1 first: node2 drained until node1 holds them
        self.drain(2, True)
        for i in self.boxes:
            self.start_box(i, "new")

        def leases():
            got = self.state(1).get("leases") or {}
            return {w: got.get(f"svc/{w}") or {} for w in WRITERS}

        def on(box, mark=0, only=WRITERS):
            return all(
                lease.get("holder") == box
                and self.runs(int(box[-1]), w, lease["epoch"], mark)
                for w, lease in leases().items()
                if w in only
            )

        wait(f"{self.name}: the writers run on node1", lambda: on("node1"), timeout=150)
        self.drain(2, False)
        t = threading.Thread(target=self.watch)
        t.start()
        time.sleep(15)  # sleep-ok: node2 back, and better by score
        held = {w: l.get("epoch") for w, l in leases().items()}
        ok(
            on("node1"),
            f"{self.name}: the holder keeps its writers though node2 suits them better",
        )

        # agreement stalls past the lease's time: the relay gone
        self.stop_relay()
        time.sleep(stall)  # sleep-ok: the stall lasts this long
        mark2 = self.logsize(2)
        self.start_relay("new")
        t1 = time.time()
        back = wait(
            f"{self.name}: the writers back on node1 after a {stall}s stall",
            lambda: (
                on("node1")
                and all(l.get("epoch", 0) > held[w] for w, l in leases().items())
            ),
            timeout=120,
        )
        if back:
            ok(
                True,
                f"{self.name}: the writers back on node1 {time.time() - t1:.1f}s after agreement came back",
            )
        self.samples.clear()
        time.sleep(observe)  # sleep-ok: watched this long for a move
        seen = {
            (w, (s.get("leases") or {}).get(f"svc/{w}", {}).get("holder"))
            for _, _, _, s in self.samples
            for w in WRITERS
        }
        ok(
            all(h == "node1" for _, h in seen),
            f"{self.name}: {observe}s after the stall, every lease with node1 (saw {sorted(seen, key=str)})",
        )
        log2 = self.log(f"systemctl2.log")[mark2:]
        ok(
            not any(line.startswith("start") for line in log2.splitlines()),
            f"{self.name}: node2 started no writer after the stall",
        )

        # node1 drained: node2 takes them over, postgres restoring a minute
        mark2 = self.logsize(2)
        self.drain(1, True)
        wait(
            f"{self.name}: node2 takes the writers over",
            lambda: on("node2", mark2),
            timeout=120,
        )
        epochs = {w: l.get("epoch") for w, l in leases().items()}
        self.samples.clear()
        time.sleep(75)  # sleep-ok: the restore's minute, and a margin
        log = self.log("n2.log")
        ok(
            "failed to start here" not in log,
            f"{self.name}: node2 refused nothing while postgres restored",
        )
        seen = {
            (
                w,
                (s.get("leases") or {}).get(f"svc/{w}", {}).get("holder"),
                (s.get("leases") or {}).get(f"svc/{w}", {}).get("epoch"),
            )
            for _, _, _, s in self.samples
            for w in WRITERS
        }
        ok(
            seen == {(w, "node2", e) for w, e in epochs.items()},
            f"{self.name}: the writers stayed on node2 through its restore (saw {sorted(seen, key=str)})",
        )

        # node2 drained, node1 back, and node1 unable to start postgres:
        # it refuses it, and the plan, with no other box, gives it back
        mark1 = self.logsize(1)
        open(f"{self.dir}/failed1", "w").write("postgresql.service\n")
        self.plans.clear()
        self.drain(1, False)
        self.drain(2, True)
        wait(
            f"{self.name}: node1 refuses postgres it cannot start, and tries it again",
            lambda: "trying it again" in self.log("n1.log"),
            timeout=150,
        )
        wait(
            f"{self.name}: postgres held by node1 at last",
            lambda: on("node1", mark1, only=("postgresql",)),
            timeout=60,
        )
        nowhere = sorted(
            {
                why
                for _, _, plan in self.plans
                for svc, why in (plan.get("unplaced") or {}).items()
                if svc in WRITERS
            }
        )
        ok(
            "failed to start here" in self.log("n1.log") and not nowhere,
            f"{self.name}: refused where no other box could run it, the plan still placed it (unplaced: {nowhere})",
        )
        self.stop_watch = True
        t.join()
        for i in self.boxes:
            self.stop_box(i)
        self.stop_relay()

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
                who == order[-1] or all(m in (None, OLD) for m in agreed),
                f"{self.name}: nothing new agreed with {who} switched and {order[order.index(who) + 1 :]} not",
            )
        wait(
            f"{self.name}: version {NEW} agreed",
            lambda: all(self.state(i).get("machine") == NEW for i in (1, 2, 3)),
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
        # svc/next: never before the new version
        early = [
            s["height"]
            for _, _, _, s in self.samples
            if s.get("leases", {}).get("svc/next") and s.get("machine") != NEW
        ]
        ok(
            not early,
            f"{self.name}: svc/next granted only under version {NEW} (early at {early[:3]})",
        )
        before2 = [
            s["height"] for _, _, _, s in self.samples if s.get("machine") == NEW
        ]
        first2 = min(before2) if before2 else None
        last_switch = phases[-1][1]
        at2 = min(
            (t for t, _, _, s in self.samples if s.get("machine") == NEW), default=None
        )
        ok(
            at2 is not None and at2 >= last_switch,
            f"{self.name}: version {NEW} agreed at height {first2}, {at2 - last_switch:.1f}s after the last switch"
            if at2
            else f"{self.name}: version {NEW} never agreed",
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


if os.environ.get("SCENARIO") == "flap":
    fleets = ["e"]
    e = Fleet("e", 8795, 4690, boxes=(1, 2), witness=False, kind="-flap")
    try:
        e.flap()
    finally:
        # a scenario that broke off leaves nothing running to hold the build
        e.stop_watch = True
        for proc in list(e.procs.values()) + [e.relay]:
            if proc and proc.poll() is None:
                proc.kill()
elif os.environ.get("SCENARIO") == "signfail":
    fleets = ["d"]
    Fleet("d", 8794, 4590, boxes=(1, 2), witness=False, kind="-signfail").signfail()
elif os.environ.get("SCENARIO") == "together":
    fleets = ["c"]
    Fleet("c", 8793, 4490, boxes=(1, 2), witness=False, kind="-together").together(
        advance=int(os.environ.get("ADVANCE", "30")), ttl=45, margin=30
    )
else:
    fleets = ["a", "b"]
    Fleet("a", 8791, 4290).run([1, "witness", 2, 3])
    Fleet("b", 8792, 4390).run([3, 2, "witness", 1])
if failed:
    print("FAILED:", *failed, sep="\n  ")
    for d in fleets:
        for f in sorted(os.listdir(f"{T}/{d}")):
            if f.endswith(".log") and not f.startswith("systemctl"):
                print(f"--- {d}/{f}")
                print("".join(open(f"{T}/{d}/{f}").readlines()[-15:]))
    sys.exit(1)
