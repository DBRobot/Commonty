# Staging before a release is signed (docs/dev/rollout.md): node2 joins the
# fleet signing with a key in its (simulated) TPM, is wave 1, and takes the
# writer over from node1 by the planned handover while a member's probe of
# it never fails; then node1 switches with nothing on it.

from rollout_fleet import Fleet

start_all()
f = Fleet(relay, {"node1": node1, "node2": node2}, nix)
f.up()

# joined: its TPM-signed heartbeats are agreed like any box's
node1.wait_until_succeeds(
    "curl -sf http://127.0.0.1:4195/state | jq -e '.boxes.node2.seq > 0'",
    timeout=120,
)

f.probe_start()
f.probe_wait("node1")
f.publish(2, {"node1": "good", "node2": "good"})
f.wait_rollout(node2, ".counter == 2", timeout=300)
assert f.record(node2)["waves"] == [["node2"], ["node1"]], f.record(node2)
f.wait_status(node1, 2, "done")
assert f.agent_said(node1, "handed over web"), f.agent_log(node1)
f.wait_holder("node2", timeout=120)
f.probe_wait("node2")
f.probe_stop()
assert f.running(node1) == f.paths["node1"]["good"]
assert f.running(node2) == f.paths["node2"]["good"]
