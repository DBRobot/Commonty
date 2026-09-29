# The thanos test: every box keeps its metrics; the observe box sees them all over the bucket.

start_all()
for m in (a, b):
    m.wait_for_unit("garage.service")
    m.wait_for_open_port(3901)
a_id = a.succeed(f"set -a; . {nix['rpc']}; garage node id -q").strip().split("@")[0]
b.succeed(f"set -a; . {nix['rpc']}; garage node connect {a_id}@a:3901")
for m in (a, b):
    m.succeed("systemctl restart garage-setup.service")
# both boxes hold the applied layout before the buckets and keys are made
for m in (a, b):
    m.wait_until_succeeds(f"set -a; . {nix['rpc']}; garage layout show > /tmp/l && grep -q 'Current cluster layout version: [1-9]' /tmp/l")
for m in (a, b):
    m.succeed("systemctl restart garage-setup.service")

# every box: prometheus with the box label, a sidecar that can reach
# the bucket (it checks at start and fails if it cannot)
for m in (a, b):
    m.succeed("systemctl start dd-facts.service")
    m.wait_for_unit("prometheus.service")
    m.systemctl("restart thanos-sidecar.service")
    m.wait_for_unit("thanos-sidecar.service")
    m.wait_for_open_port(10901)

# the observe box: store over the bucket, query over store and sidecars
a.wait_for_unit("thanos-store.service")
a.wait_for_unit("thanos-compact.service")
a.wait_for_unit("thanos-query.service")
a.wait_for_open_port(10903)
a.wait_until_succeeds("curl -sf 'http://127.0.0.1:10903/api/v1/query?query=dd_box_cpu_cores' -o /tmp/q && grep -q '\"box\":\"a\"' /tmp/q && grep -q '\"box\":\"b\"' /tmp/q")

# The Boxes and Backups pages: the gate on a reads both boxes' facts from
# thanos beside it. Neither box's prometheus answers anyone but itself.
import json
env = "DD_KEYRING_FILE=/root/keys.json"
dirs = "--directory http://127.0.0.1:4181/_dd/directory"
a.wait_for_open_port(4181)
a.succeed(f"{env} {nix['dd']} identity new --name sarah {dirs}")
a.succeed("mkdir -p /root/fleet/fleet")
a.succeed(f"{env} {nix['dd']} member add sarah --repo /root/fleet {dirs}")
a.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")
a.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    "\"$(jq -c . /root/fleet/fleet/members.json)\" "
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
a.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
a.wait_for_open_port(4181)
token = a.succeed(f"{env} {nix['dd']} token").strip()
b.fail("curl -sf --max-time 3 http://192.168.1.1:9090/api/v1/query?query=up")
seen = json.loads(a.succeed(f"curl -sf -H 'authorization: Bearer {token}' http://127.0.0.1:4181/_dd/fleet.json"))
by = {x["name"]: x for x in seen}
assert set(by) == {"a", "b"}, seen
for n in ("a", "b"):
    assert by[n]["up"] and by[n]["cores"], (n, seen)

# b goes away: a still answers, with what it has
b.shutdown()
a.wait_until_succeeds("curl -sf 'http://127.0.0.1:10903/api/v1/query?query=dd_box_cpu_cores' -o /tmp/q && grep -q '\"box\":\"a\"' /tmp/q")
