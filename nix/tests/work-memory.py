from waits import wait_unit_active

GIB = 1024 * 1024 * 1024


def show(unit, prop):
    return box.succeed(f"systemctl show -p {prop} --value {unit}").strip()


def oom_kills():
    log = box.succeed("journalctl -k -o cat --no-pager")
    return [ln for ln in log.splitlines() if ln.startswith("oom-kill:")]


box.wait_for_unit("multi-user.target")
for unit in ("ours", "holder", "runner", "build"):
    box.succeed(f"systemctl start {unit}")
    wait_unit_active(box, unit)

# the slice is half of a 4G box, and the units are in it; ours is not
limit = int(box.succeed("cat /sys/fs/cgroup/work.slice/memory.max").strip())
assert limit == 2 * GIB, f"work.slice memory.max {limit}, not 2G"
for unit in ("runner", "build", "holder"):
    cg = show(unit, "ControlGroup")
    assert cg == f"/work.slice/{unit}.service", f"{unit} is in {cg}"
assert show("ours", "ControlGroup") == "/system.slice/ours.service"
assert show("runner", "OOMPolicy") == "continue"
assert show("build", "OOMPolicy") == "stop"

# 1. a job in the runner eats memory until it is killed
runner_pid = show("runner", "MainPID")
other = box.succeed("cat /run/runner-other").strip()
box.succeed("touch /run/runner-hog")
box.wait_until_succeeds("test -s /run/runner-hog-pid", timeout=60)
hog = box.succeed("cat /run/runner-hog-pid").strip()
box.wait_until_fails(f"kill -0 {hog}", timeout=300)
box.wait_until_succeeds(
    "journalctl -u runner -o cat --no-pager | grep -c 'OOM killer killed some processes'",
    timeout=60,
)
kills = oom_kills()
print("\n".join(kills))
assert len(kills) == 1, f"one kill expected, the kernel made {len(kills)}"
# the slice's limit did it, not the box running out, and it took the hog
assert "constraint=CONSTRAINT_MEMCG" in kills[0], kills[0]
assert "oom_memcg=/work.slice," in kills[0], kills[0]
assert f"pid={hog}," in kills[0], kills[0]
# the runner carries on, with its other job; the build's share and the
# box's own service are untouched
assert show("runner", "ActiveState") == "active", show("runner", "ActiveState")
assert show("runner", "MainPID") == runner_pid, "the runner was restarted"
box.succeed(f"kill -0 {other}")
for unit in ("holder", "build", "ours"):
    assert show(unit, "ActiveState") == "active", f"{unit}: {show(unit, 'ActiveState')}"

# 2. the same in a unit left at systemd's default: the whole unit stops,
# its other job with it (the runner on node2, 2026-10-20)
other = box.succeed("cat /run/build-other").strip()
box.succeed("touch /run/build-hog")
box.wait_until_succeeds("test -s /run/build-hog-pid", timeout=60)
box.wait_until_succeeds(
    "systemctl show -p Result --value build | grep -c oom-kill", timeout=300
)
box.wait_until_fails(f"kill -0 {other}", timeout=60)
assert len(oom_kills()) == 2, oom_kills()
for unit in ("runner", "holder", "ours"):
    assert show(unit, "ActiveState") == "active", f"{unit}: {show(unit, 'ActiveState')}"
