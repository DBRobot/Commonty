# Staged rollout on two boxes (docs/dev/rollout.md): wave 1, its soak, the
# mark, drain before switch, and the control plane going away mid-rollout.

import json

from waits import wait_unit_active

boxes = {"node1": node1, "node2": node2}
paths = nix["paths"]

start_all()
relay.wait_for_unit("nginx.service")
for m in boxes.values():
    m.wait_for_unit("dd-verify.service")
    m.wait_for_open_port(4181)
    m.wait_for_unit("sshd.service")
    m.wait_for_unit("nginx.service")
    # the test starts each run of the agent itself until the control
    # plane is up; then the agents' own timers drive everything
    m.succeed("systemctl stop dd-agent.timer dd-agent-soak.timer")

# the release key, as the laptop holds it; the boxes get its public half
env = "DD_KEYRING_FILE=/root/keys.json"
relay.succeed("git init -q /root/repo && mkdir -p /root/repo/fleet")
pub = relay.succeed(
    f"{env} {nix['dd']} release init --repo /root/repo | tail -1"
).strip()

# each box's key (a TPM's, on a real box) and its validator key
spki = {}
votes = {}
for name, m in boxes.items():
    relay.succeed(
        "openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 "
        f"-outform DER -out /root/{name}.p8"
    )
    p8 = relay.succeed(f"base64 -w0 /root/{name}.p8").strip()
    spki[name] = relay.succeed(
        f"openssl pkey -inform DER -in /root/{name}.p8 -pubout -outform DER | base64 -w0"
    ).strip()
    m.succeed(
        f"mkdir -p /var/lib/rollout /var/lib/dd-control && echo {p8} | base64 -d > /var/lib/rollout/box.p8"
    )
    m.succeed(f"echo '{pub}' > /root/release.pub")
    votes[name] = m.succeed(
        f"{nix['control']} -print-validator-key -state /var/lib/dd-control"
    ).strip()
# the witness, at the relay as at Cloudflare
relay.succeed("openssl genpkey -algorithm ed25519 -outform DER -out /root/w.der")
wpriv = relay.succeed("tail -c 32 /root/w.der | base64 -w0").strip()
wpub = relay.succeed(
    "openssl pkey -inform DER -in /root/w.der -pubout -outform DER | tail -c 32 | base64 -w0"
).strip()

config = {
    "boxes": {
        n: {"id": 100 + i, "house": "h1", "key": spki[n]}
        for i, n in enumerate(boxes, start=1)
    },
    "validators": [
        {"id": 1, "name": "node1", "key": votes["node1"]},
        {"id": 2, "name": "node2", "key": votes["node2"]},
        {"id": 3, "name": "cloudflare", "key": wpub},
    ],
    "rules": {
        "leaseTtlMs": 15000,
        "goneAfterMs": 20000,
        "leases": {"svc/web": {"eligible": ["node1", "node2"]}},
        # a minute's soak for wave 1 and none after; deadlines long enough
        # for a slow vm
        "rollout": {"soakMs": [60000, 0], "tookMs": 600000, "graceMs": 600000},
    },
    "identity": {"release": pub, "members": {"members": [], "revoked": []}},
}
blob = json.dumps(config)
for m in boxes.values():
    m.succeed(f"cat > /var/lib/rollout/config.json <<'EOF'\n{blob}\nEOF")

wrangler_toml = """name = "rollout"
main = "worker.js"
compatibility_date = "2026-05-01"
no_bundle = true
rules = [
  { type = "CompiledWasm", globs = ["**/*.wasm"] },
  { type = "ESModule", globs = ["**/*.js"] },
]
[[durable_objects.bindings]]
name = "RELAY"
class_name = "Relay"
[[migrations]]
tag = "v1"
new_sqlite_classes = ["Relay"]
"""
relay.succeed(
    f"mkdir -p /root/w && cp {nix['worker']}/* /root/w/ && chmod u+w /root/w/*"
)
relay.succeed(f"cat > /root/w/wrangler.toml <<'EOF'\n{wrangler_toml}EOF")
relay.succeed(f"echo 'export default {blob};' > /root/w/config.js")
relay.succeed(f"echo 'WITNESS_KEY={wpriv}' > /root/w/.dev.vars")


def relay_up():
    relay.succeed(
        "systemd-run --unit=relay-worker --working-directory=/root/w "
        "--setenv=HOME=/root --setenv=WRANGLER_SEND_METRICS=false --setenv=CI=1 "
        "--setenv=CLOUDFLARE_CF_FETCH_ENABLED=false "
        "wrangler dev --port 8789 --ip 0.0.0.0"
    )
    relay.wait_until_succeeds("curl -sf http://127.0.0.1:8789/health", timeout=300)


relay_up()
for m in boxes.values():
    m.succeed("systemctl start dd-control.service")


def state(m):
    return json.loads(m.succeed("curl -sf http://127.0.0.1:4195/state"))


def record(m):
    return json.loads(m.succeed("curl -sf http://127.0.0.1:4195/rollout"))["rollout"]


def holder(m):
    lease = state(m)["leases"].get("svc/web") or {}
    return lease.get("holder")


def running(m):
    return m.succeed("readlink /run/current-system").strip()


def rollout_is(m, counter, status, timeout=900):
    m.wait_until_succeeds(
        "curl -sf http://127.0.0.1:4195/rollout | "
        f"jq -e '.rollout.counter == {counter} and .rollout.status == \"{status}\"'",
        timeout=timeout,
    )


# agreement, and web placed on node1 (first by name) under its lease
for m in boxes.values():
    m.wait_until_succeeds(
        "curl -sf http://127.0.0.1:4195/state | jq -e '.boxes | length == 2'",
        timeout=300,
    )
node1.wait_until_succeeds(
    'curl -sf http://127.0.0.1:4195/state | jq -e \'.leases["svc/web"].holder == "node1"\'',
    timeout=300,
)
wait_unit_active(node1, "web.service")
node1.wait_until_succeeds(
    "curl -sf -H 'Host: web.test.invalid' http://127.0.0.1/ | grep -q 'web on node1'"
)


def nar_hash(path):
    info = json.loads(
        node1.succeed(
            f"nix --extra-experimental-features nix-command path-info --json {path}"
        )
    )
    entry = info.get(path) or next(e for e in info if e["path"] == path)
    return entry["narHash"]


def publish(counter, which):
    payload = json.dumps(
        {
            "counter": counter,
            "rev": "0123456789abcdef",
            "issued": 1,
            "boxes": {
                n: {"path": paths[n][w], "nar_hash": nar_hash(paths[n][w])}
                for n, w in which.items()
            },
        }
    )
    relay.succeed(f"cat > /root/payload.json <<'EOF'\n{payload}\nEOF")
    relay.succeed(
        f"{env} {nix['dd']} release sign /root/payload.json --out /root/current.json"
    )
    relay.succeed(
        "mkdir -p /srv/release && install -m 644 /root/current.json /srv/release/current.json"
    )
    for m in boxes.values():
        m.succeed("systemctl start --no-block dd-agent.service")


def agent_said(m, text):
    return (
        m.succeed(
            f"journalctl -u dd-agent -o cat | grep -c -F '{text}' || true"
        ).strip()
        != "0"
    )


# from here the agents' own timers run them
for m in boxes.values():
    m.succeed("systemctl start dd-agent.timer dd-agent-soak.timer")

# --- a good release: wave 1 (node2, holding no writer), its soak, then node1
publish(2, {"node1": "good", "node2": "good"})
node2.wait_until_succeeds(
    "curl -sf http://127.0.0.1:4195/rollout | jq -e '.rollout.counter == 2'",
    timeout=300,
)
assert record(node2)["waves"] == [["node2"], ["node1"]], record(node2)
node2.wait_until_succeeds(
    f"[ $(readlink /run/current-system) = {paths['node2']['good']} ]", timeout=600
)
# node1 waits for the mark, serving
assert running(node1) == paths["node1"]["base"]
node1.wait_until_succeeds(
    "journalctl -u dd-agent -o cat | grep -q 'held: release 2 waits for wave 1 (node2)'",
    timeout=300,
)
rollout_is(node1, 2, "done")
assert running(node1) == paths["node1"]["good"]
assert running(node2) == paths["node2"]["good"]
# node1 handed web over before it switched; it stays where it went
assert agent_said(node1, "handed over web"), node1.succeed(
    "journalctl -u dd-agent -o cat | tail -40"
)
node2.wait_until_succeeds(
    'curl -sf http://127.0.0.1:4195/state | jq -e \'.leases["svc/web"].holder == "node2"\'',
    timeout=120,
)
for m in boxes.values():
    m.wait_until_succeeds(
        "grep -q 'dd_agent_counter{box=\"[a-z0-9]*\"} 2' /var/lib/dd-facts/dd_agent.prom",
        timeout=300,
    )

# --- a release whose soak fails on wave 1 (node1 now holds no writer):
# it stops there, wave 1 rolls back, node2 never moves
publish(3, {"node1": "crashloop", "node2": "crashloop"})
node1.wait_until_succeeds(
    "curl -sf http://127.0.0.1:4195/rollout | jq -e '.rollout.counter == 3'",
    timeout=300,
)
assert record(node1)["waves"] == [["node1"], ["node2"]], record(node1)
rollout_is(node1, 3, "bad")
assert "restarted" in record(node1)["why"], record(node1)
node1.wait_until_succeeds(
    f"[ $(readlink /run/current-system) = {paths['node1']['good']} ]", timeout=300
)
node1.succeed("grep -q '^3 ' /var/lib/dd-agent/refused")
node2.wait_until_succeeds(
    "grep -q 'dd_rollout_bad{counter=\"3\"} 1' /var/lib/dd-facts/dd_rollout.prom",
    timeout=300,
)
assert running(node2) == paths["node2"]["good"]
node2.fail("grep -q '^3 ' /var/lib/dd-agent/refused")
assert not agent_said(node2, "release 3: switched")

# --- drain before switch: node2 holds web; node1 is wave 1 and proves the
# release; node2 hands web to node1, then its own switch fails and rolls
# back. A member's probe of web, following the lease as DNS does (ten
# seconds behind), never fails: a moment's hold page at most
probe = r"""
import json, time, urllib.request, http.client
lag, seen, fails, hold, longest = [], None, 0, None, 0.0
ips = {"node1": "node1", "node2": "node2"}
def lease():
    for b in ips.values():
        try:
            r = urllib.request.Request(f"http://{b}/_test/state", headers={"Host": "web.test.invalid"})
            return json.load(urllib.request.urlopen(r, timeout=2))["leases"]["svc/web"]["holder"]
        except Exception:
            pass
log = open("/root/probe.log", "w", buffering=1)
while True:
    now = time.time()
    h = lease()
    if h:
        lag.append((now, h))
    while lag and lag[0][0] < now - 10:
        seen = lag.pop(0)[1]
    target = seen or (lag[0][1] if lag else None)
    if target:
        try:
            c = http.client.HTTPConnection(ips[target], 80, timeout=5)
            c.request("GET", "/", headers={"Host": "web.test.invalid"})
            r = c.getresponse()
            body = r.read()
            if r.status == 200 and b"web on" in body:
                hold = None
            elif r.status == 503 and r.getheader("Retry-After"):
                hold = hold or now
                longest = max(longest, now - hold)
            else:
                fails += 1
                log.write(f"fail {target} {r.status}\n")
        except Exception as e:
            fails += 1
            log.write(f"fail {target} {e!r}\n")
    log.write(f"tick {target} fails={fails} longest-hold={longest:.1f}\n")
    time.sleep(0.5)
"""
relay.succeed(f"cat > /root/probe.py <<'EOF'\n{probe}\nEOF")
relay.succeed("systemd-run --unit=member-probe python3 /root/probe.py")
relay.wait_until_succeeds("grep -q 'tick node2' /root/probe.log", timeout=60)

publish(4, {"node1": "good2", "node2": "broken"})
node1.wait_until_succeeds(
    "curl -sf http://127.0.0.1:4195/rollout | jq -e '.rollout.counter == 4'",
    timeout=300,
)
assert record(node1)["waves"] == [["node1"], ["node2"]], record(node1)
rollout_is(node1, 4, "bad", timeout=1200)
assert "did not come up healthy" in record(node1)["why"], record(node1)
node2.wait_until_succeeds(
    f"[ $(readlink /run/current-system) = {paths['node2']['good']} ]", timeout=300
)
node2.wait_for_unit("sshd.service")
assert agent_said(node2, "handed over web"), node2.succeed(
    "journalctl -u dd-agent -o cat | tail -40"
)
assert holder(node1) == "node1"
assert running(node1) == paths["node1"]["good2"]
relay.wait_until_succeeds("grep -q 'tick node1' /root/probe.log", timeout=60)
relay.succeed("systemctl stop member-probe")
last = relay.succeed("grep '^tick' /root/probe.log | tail -1").strip()
fails = relay.succeed("grep -c '^fail' /root/probe.log || true").strip()
assert fails == "0", relay.succeed("grep '^fail' /root/probe.log | head -20")
longest = float(last.split("longest-hold=")[1])
assert longest <= 60, last
print(f"member probe: {last}")

# --- the control plane down mid-rollout: web is on node1, so node2 is
# wave 1. Once it has taken the release, the relay goes away for longer
# than a box counts as gone: node1 stays held where it is, node2 keeps its
# release, and once agreement is back the rollout finishes, the outage not
# counted against anyone
publish(5, {"node1": "good3", "node2": "good3"})
node2.wait_until_succeeds(
    "curl -sf http://127.0.0.1:4195/rollout | "
    "jq -e '.rollout.counter == 5 and .rollout.reports.node2.took > 0'",
    timeout=600,
)
assert record(node2)["waves"] == [["node2"], ["node1"]], record(node2)
relay.succeed("systemctl stop relay-worker")
stalled = state(node1)["time"]
# a minute of no agreement: three goneAfters
node1.wait_until_succeeds(
    f"[ $(( $(date +%s%3N) - {stalled} )) -gt 60000 ]", timeout=120
)
assert running(node1) == paths["node1"]["good"]
assert running(node2) == paths["node2"]["good3"]
node1.succeed("systemctl start --no-block dd-agent.service")
node1.wait_until_succeeds(
    "journalctl -u dd-agent -o cat | grep -q 'held: release 5 waits for wave 1'",
    timeout=120,
)
relay.succeed("systemctl reset-failed relay-worker || true")
relay_up()
rollout_is(node2, 5, "done", timeout=900)
r = record(node2)
assert r["paused"] > 0, r
assert running(node1) == paths["node1"]["good3"]
assert running(node2) == paths["node2"]["good3"]
