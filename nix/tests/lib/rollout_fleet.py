"""The staged-rollout fleet (nix/tests/rollout-fleet.nix): a relay running
the control plane's Worker and witness under wrangler, serving releases,
and two boxes running the real dd-control and dd-agent. Shared by the
rollout test and the staging run before a release."""

import json

from waits import wait_fetch_has, wait_unit_active

WRANGLER_TOML = """name = "rollout"
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

# a box's signing key made in its TPM under PCR 7, as dd-unlock-enrol keys
# makes it (box/disk-unlock-enrol.sh); prints the key as fleet/boxes.json has it
TPM_ENROL = """set -eu
export TPM2TOOLS_TCTI=device:/dev/tpmrm0
d=/var/lib/rollout/tpm
mkdir -p $d
w=$(mktemp -d)
tpm2_createprimary -Q -C o -G ecc256 -c $w/srk.ctx
tpm2_startauthsession -Q -S $w/s.ctx
tpm2_policypcr -Q -S $w/s.ctx -l sha256:7 -L $w/pcr.pol
tpm2_flushcontext $w/s.ctx
tpm2_create -Q -C $w/srk.ctx -G ecc256:ecdsa-sha256 -L $w/pcr.pol \\
  -a 'fixedtpm|fixedparent|sensitivedataorigin|sign|noda' -u $d/sig.pub -r $d/sig.priv
tpm2_load -Q -C $w/srk.ctx -u $d/sig.pub -r $d/sig.priv -c $w/sig.ctx
tpm2_readpublic -Q -c $w/sig.ctx -f der -o $w/spki.der
tpm2_flushcontext -t >/dev/null 2>&1 || true
base64 -w0 $w/spki.der
"""

# a member following web's name, as DNS follows the lease (ten seconds
# behind): every answer is the page, or a hold (503 with Retry-After);
# anything else is a failure the member saw
PROBE = r"""
import json, time, urllib.request, http.client
lag, seen, fails, hold, longest = [], None, 0, None, 0.0
def lease():
    for b in ("node1", "node2"):
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
            c = http.client.HTTPConnection(target, 80, timeout=5)
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
    time.sleep(0.5)  # sleep-ok: the member asks twice a second
"""


class Fleet:
    def __init__(self, relay, boxes, nix):
        self.relay = relay
        self.boxes = boxes
        self.nix = nix
        self.paths = nix["paths"]
        self.env = "DD_KEYRING_FILE=/root/keys.json"

    def up(self):
        relay, nix = self.relay, self.nix
        relay.wait_for_unit("nginx.service")
        for m in self.boxes.values():
            m.wait_for_unit("dd-verify.service")
            m.wait_for_open_port(4181)
            m.wait_for_unit("sshd.service")
            m.wait_for_unit("nginx.service")
            # nothing runs the agent until the control plane is up
            m.succeed("systemctl stop dd-agent.timer dd-agent-soak.timer")

        # the release key, as the laptop holds it; the boxes get its public half
        relay.succeed("git init -q /root/repo && mkdir -p /root/repo/fleet")
        self.pub = relay.succeed(
            f"{self.env} {nix['dd']} release init --repo /root/repo | tail -1"
        ).strip()

        # each box's key (in its TPM, or a file standing in for one) and
        # its validator key
        spki, votes = {}, {}
        for name, m in self.boxes.items():
            m.succeed("mkdir -p /var/lib/rollout /var/lib/dd-control")
            if name in nix["tpm"]:
                spki[name] = m.succeed(TPM_ENROL).strip()
            else:
                relay.succeed(
                    "openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 "
                    f"-outform DER -out /root/{name}.p8"
                )
                p8 = relay.succeed(f"base64 -w0 /root/{name}.p8").strip()
                spki[name] = relay.succeed(
                    f"openssl pkey -inform DER -in /root/{name}.p8 -pubout -outform DER | base64 -w0"
                ).strip()
                m.succeed(f"echo {p8} | base64 -d > /var/lib/rollout/box.p8")
            m.succeed(f"echo '{self.pub}' > /root/release.pub")
            votes[name] = m.succeed(
                f"{nix['control']} -print-validator-key -state /var/lib/dd-control"
            ).strip()
        # the witness, at the relay as at Cloudflare
        relay.succeed(
            "openssl genpkey -algorithm ed25519 -outform DER -out /root/w.der"
        )
        wpriv = relay.succeed("tail -c 32 /root/w.der | base64 -w0").strip()
        wpub = relay.succeed(
            "openssl pkey -inform DER -in /root/w.der -pubout -outform DER | tail -c 32 | base64 -w0"
        ).strip()

        config = {
            "boxes": {
                n: {"id": 100 + i, "house": "h1", "key": spki[n]}
                for i, n in enumerate(self.boxes, start=1)
            },
            "validators": [
                {"id": i, "name": n, "key": votes[n]}
                for i, n in enumerate(self.boxes, start=1)
            ]
            + [{"id": len(self.boxes) + 1, "name": "cloudflare", "key": wpub}],
            "rules": {
                "leaseTtlMs": 15000,
                "goneAfterMs": 20000,
                "leases": {"svc/web": {"eligible": list(self.boxes)}},
                # a minute's soak for wave 1 and none after; deadlines long
                # enough for a slow vm
                "rollout": {
                    "soakMs": [60000, 0],
                    "tookMs": 600000,
                    "graceMs": 600000,
                },
            },
            "identity": {
                "release": self.pub,
                "members": {"members": [], "revoked": []},
            },
        }
        blob = json.dumps(config)
        for m in self.boxes.values():
            m.succeed(f"cat > /var/lib/rollout/config.json <<'EOF'\n{blob}\nEOF")
        relay.succeed(
            f"mkdir -p /root/w && cp {nix['worker']}/* /root/w/ && chmod u+w /root/w/*"
        )
        relay.succeed(f"cat > /root/w/wrangler.toml <<'EOF'\n{WRANGLER_TOML}EOF")
        relay.succeed(f"echo 'export default {blob};' > /root/w/config.js")
        relay.succeed(f"echo 'WITNESS_KEY={wpriv}' > /root/w/.dev.vars")
        self.relay_up()
        for m in self.boxes.values():
            m.succeed("systemctl start dd-control.service")

        # agreement, and web placed on node1 (first by name) under its lease
        for m in self.boxes.values():
            m.wait_until_succeeds(
                "curl -sf http://127.0.0.1:4195/state | "
                f"jq -e '.boxes | length == {len(self.boxes)}'",
                timeout=300,
            )
        self.wait_holder("node1")
        node1 = self.boxes["node1"]
        wait_unit_active(node1, "web.service")
        wait_fetch_has(
            node1,
            "http://127.0.0.1/",
            "web on node1",
            curl_args="-sf -H 'Host: web.test.invalid'",
        )
        # from here the agents' own timers run them
        for m in self.boxes.values():
            m.succeed("systemctl start dd-agent.timer dd-agent-soak.timer")

    def relay_up(self):
        self.relay.succeed(
            "systemd-run --unit=relay-worker --working-directory=/root/w "
            "--setenv=HOME=/root --setenv=WRANGLER_SEND_METRICS=false --setenv=CI=1 "
            "--setenv=CLOUDFLARE_CF_FETCH_ENABLED=false "
            "wrangler dev --port 8789 --ip 0.0.0.0"
        )
        self.relay.wait_until_succeeds(
            "curl -sf http://127.0.0.1:8789/health", timeout=300
        )

    def relay_down(self):
        self.relay.succeed("systemctl stop relay-worker")
        self.relay.succeed("systemctl reset-failed relay-worker || true")

    def state(self, m):
        return json.loads(m.succeed("curl -sf http://127.0.0.1:4195/state"))

    def record(self, m):
        return json.loads(m.succeed("curl -sf http://127.0.0.1:4195/rollout"))[
            "rollout"
        ]

    def holder(self, m):
        return (self.state(m)["leases"].get("svc/web") or {}).get("holder")

    def wait_holder(self, name, timeout=300):
        self.boxes[name].wait_until_succeeds(
            "curl -sf http://127.0.0.1:4195/state | "
            f'jq -e \'.leases["svc/web"].holder == "{name}"\'',
            timeout=timeout,
        )

    def running(self, m):
        return m.succeed("readlink /run/current-system").strip()

    def wait_running(self, name, which, timeout=600):
        self.boxes[name].wait_until_succeeds(
            f"[ $(readlink /run/current-system) = {self.paths[name][which]} ]",
            timeout=timeout,
        )

    def wait_rollout(self, m, jq, timeout=900):
        m.wait_until_succeeds(
            f"curl -sf http://127.0.0.1:4195/rollout | jq -e '.rollout | {jq}'",
            timeout=timeout,
        )

    def wait_status(self, m, counter, status, timeout=900):
        self.wait_rollout(
            m, f'.counter == {counter} and .status == "{status}"', timeout
        )

    def nar_hash(self, path):
        m = next(iter(self.boxes.values()))
        info = json.loads(
            m.succeed(
                f"nix --extra-experimental-features nix-command path-info --json {path}"
            )
        )
        entry = info.get(path) or next(e for e in info if e["path"] == path)
        return entry["narHash"]

    def publish(self, counter, which):
        payload = json.dumps(
            {
                "counter": counter,
                "rev": "0123456789abcdef",
                "issued": 1,
                "boxes": {
                    n: {
                        "path": self.paths[n][w],
                        "nar_hash": self.nar_hash(self.paths[n][w]),
                    }
                    for n, w in which.items()
                },
            }
        )
        relay = self.relay
        relay.succeed(f"cat > /root/payload.json <<'EOF'\n{payload}\nEOF")
        relay.succeed(
            f"{self.env} {self.nix['dd']} release sign /root/payload.json --out /root/current.json"
        )
        relay.succeed(
            "mkdir -p /srv/release && install -m 644 /root/current.json /srv/release/current.json"
        )
        for m in self.boxes.values():
            m.succeed("systemctl start --no-block dd-agent.service")

    def agent_said(self, m, text):
        return (
            m.succeed(
                f"journalctl -u dd-agent -o cat | grep -c -F '{text}' || true"
            ).strip()
            != "0"
        )

    def wait_agent_said(self, m, text, timeout=300):
        m.wait_until_succeeds(
            f"journalctl -u dd-agent -o cat | grep -c -F '{text}'", timeout=timeout
        )

    def agent_log(self, m):
        return m.succeed("journalctl -u dd-agent -o cat | tail -60")

    def probe_start(self):
        relay = self.relay
        relay.succeed(f"cat > /root/probe.py <<'EOF'\n{PROBE}\nEOF")
        relay.succeed("systemd-run --unit=member-probe python3 /root/probe.py")

    def probe_wait(self, holder, timeout=120):
        self.relay.wait_until_succeeds(
            f"grep -c '^tick {holder} ' /root/probe.log", timeout=timeout
        )

    def probe_stop(self):
        """stops the member's probe: it never failed, and no hold was over a
        minute; returns its last line"""
        relay = self.relay
        relay.succeed("systemctl stop member-probe")
        last = relay.succeed("grep '^tick' /root/probe.log | tail -1").strip()
        fails = relay.succeed("grep -c '^fail' /root/probe.log || true").strip()
        assert fails == "0", relay.succeed("grep '^fail' /root/probe.log | head -20")
        longest = float(last.split("longest-hold=")[1])
        assert longest <= 60, last
        print(f"member probe: {last}")
        return last
