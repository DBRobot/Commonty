# Staged rollout on two boxes (docs/dev/rollout.md): wave 1, its soak, the
# mark, drain before switch, and the control plane going away mid-rollout.

from rollout_fleet import Fleet

start_all()
f = Fleet(relay, {"node1": node1, "node2": node2}, nix)
f.up()

# --- a good release: wave 1 is node2, which holds no writer; it soaks, and
# then node1 hands web over and follows
f.publish(2, {"node1": "good", "node2": "good"})
f.wait_rollout(node2, ".counter == 2", timeout=300)
assert f.record(node2)["waves"] == [["node2"], ["node1"]], f.record(node2)
f.wait_running("node2", "good")
# node1 waits for the mark, serving
assert f.running(node1) == f.paths["node1"]["base"]
f.wait_agent_said(node1, "held: release 2 waits for wave 1 (node2)")
f.wait_status(node1, 2, "done")
assert f.running(node1) == f.paths["node1"]["good"]
assert f.running(node2) == f.paths["node2"]["good"]
assert f.agent_said(node1, "handed over web"), f.agent_log(node1)
# and web stays where it went
f.wait_holder("node2", timeout=120)
for m in (node1, node2):
    m.wait_until_succeeds(
        "grep -c 'dd_agent_counter{box=\"[a-z0-9]*\"} 2$' /var/lib/dd-facts/dd_agent.prom",
        timeout=300,
    )

# --- a release whose soak fails on wave 1 (node1 now holds no writer): it
# stops there, wave 1 rolls back, node2 never moves
f.publish(3, {"node1": "crashloop", "node2": "crashloop"})
f.wait_rollout(node1, ".counter == 3", timeout=300)
assert f.record(node1)["waves"] == [["node1"], ["node2"]], f.record(node1)
f.wait_status(node1, 3, "bad")
assert "restarted" in f.record(node1)["why"], f.record(node1)
f.wait_running("node1", "good", timeout=300)
node1.succeed("grep -c '^3 ' /var/lib/dd-agent/refused")
node2.wait_until_succeeds(
    "grep -c 'dd_rollout_bad{counter=\"3\"} 1' /var/lib/dd-facts/dd_rollout.prom",
    timeout=300,
)
assert f.running(node2) == f.paths["node2"]["good"]
node2.fail("grep -c '^3 ' /var/lib/dd-agent/refused")
assert not f.agent_said(node2, "release 3: switched"), f.agent_log(node2)

# --- drain before switch: node2 holds web; node1 is wave 1 and proves the
# release; node2 hands web to node1, then its own switch fails and rolls
# back. A member's probe of web never fails: a moment's hold page at most
f.probe_start()
f.probe_wait("node2")
f.publish(4, {"node1": "good2", "node2": "broken"})
f.wait_rollout(node1, ".counter == 4", timeout=300)
assert f.record(node1)["waves"] == [["node1"], ["node2"]], f.record(node1)
f.wait_status(node1, 4, "bad", timeout=1200)
assert "did not come up healthy" in f.record(node1)["why"], f.record(node1)
f.wait_running("node2", "good", timeout=300)
node2.wait_for_unit("sshd.service")
assert f.agent_said(node2, "handed over web"), f.agent_log(node2)
assert f.holder(node1) == "node1"
assert f.running(node1) == f.paths["node1"]["good2"]
f.probe_wait("node1")
f.probe_stop()

# --- the control plane down mid-rollout: web is on node1, so node2 is wave
# 1. Once it has taken the release the relay goes away for three times as
# long as a box counts as gone: node1 stays where it is, node2 keeps its
# release, and once agreement is back the rollout finishes, the outage not
# counted against anyone
f.publish(5, {"node1": "good3", "node2": "good3"})
f.wait_rollout(node2, ".counter == 5 and .reports.node2.took > 0", timeout=600)
assert f.record(node2)["waves"] == [["node2"], ["node1"]], f.record(node2)
f.relay_down()
stalled = f.state(node1)["time"]
node1.wait_until_succeeds(
    f"[ $(( $(date +%s%3N) - {stalled} )) -gt 60000 ]", timeout=120
)
assert f.running(node1) == f.paths["node1"]["good2"]
assert f.running(node2) == f.paths["node2"]["good3"]
node1.succeed("systemctl start --no-block dd-agent.service")
f.wait_agent_said(node1, "held: release 5 waits for wave 1", timeout=120)
f.relay_up()
f.wait_status(node2, 5, "done")
r = f.record(node2)
assert r["paused"] > 0, r
assert f.running(node1) == f.paths["node1"]["good3"]
assert f.running(node2) == f.paths["node2"]["good3"]
