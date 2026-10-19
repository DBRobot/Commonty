# The release test: a signed release moves a box; a bad one is rolled back; a tampered one is refused;
# one counter names one system; the guard puts back the system and the profile.

import json

start_all()
forge.wait_for_unit("nginx.service")
box.wait_for_unit("dd-verify.service")
box.wait_for_open_port(4181)
box.wait_for_unit("sshd.service")
# the test runs the agent itself: its own timer, which a switch starts
# again, must not run it between a publish and the test's run (on a slow
# box it did, and the test's run then only said "tried already")
box.succeed(
    "mkdir -p /run/systemd/system/dd-agent.timer.d && "
    "printf '[Timer]\\nOnActiveSec=\\nOnUnitActiveSec=\\nOnActiveSec=1d\\n' "
    "> /run/systemd/system/dd-agent.timer.d/test.conf && systemctl daemon-reload"
)

# the release key lives on the forge machine here, standing in for the
# laptop; the box gets the public half
env = "DD_KEYRING_FILE=/root/keys.json"
forge.succeed("git init -q /root/repo && mkdir -p /root/repo/fleet")
pub = forge.succeed(
    f"{env} {nix['dd']} release init --repo /root/repo | tail -1"
).strip()
box.succeed(f"echo '{pub}' > /root/release.pub")


def nar_hash(path):
    info = json.loads(
        box.succeed(
            f"nix --extra-experimental-features nix-command path-info --json {path}"
        )
    )
    entry = info.get(path) or next(e for e in info if e["path"] == path)
    return entry["narHash"]


def publish(counter, path, tamper=False):
    payload = json.dumps(
        {
            "counter": counter,
            "rev": "0123456789abcdef",
            "issued": 1,
            "boxes": {"box": {"path": path, "nar_hash": nar_hash(path)}},
        }
    )
    forge.succeed(f"cat > /root/payload.json <<'EOF'\n{payload}\nEOF")
    forge.succeed(
        f"{env} {nix['dd']} release sign /root/payload.json --out /root/current.json"
    )
    if tamper:
        forge.succeed(
            'sed -i \'s/"counter": [0-9]*/"counter": 99/\' /root/current.json'
        )
    forge.succeed(
        "mkdir -p /srv/release && install -m 644 /root/current.json /srv/release/current.json"
    )


def agent(expect_ok=True):
    # this run, not the last one: a start only queued leaves the unit as the
    # last run left it. A switch can make systemd re-exec, which cuts
    # systemctl's wait short, and a systemctl that cannot reach systemd then
    # prints nothing: wait for a new invocation that systemd says is done
    before = box.succeed("systemctl show -P InvocationID dd-agent.service").strip()
    box.succeed("systemctl start --no-block dd-agent.service")
    box.wait_until_succeeds(
        "id=$(systemctl show -P InvocationID dd-agent.service) && "
        f'[ -n "$id" ] && [ "$id" != "{before}" ] && ' + gone("dd-agent.service"),
        timeout=300,
    )
    result = box.succeed("systemctl show -P Result dd-agent.service").strip()
    assert (result == "success") == expect_ok, (result, run_log())


def run_log():
    """what the agent's last run said, and only that run"""
    return box.succeed(
        "journalctl -o cat _SYSTEMD_INVOCATION_ID=$(systemctl show -P InvocationID dd-agent.service)"
    )


def said(text):
    # journald may take the last lines of an exited run a moment after
    # systemd has the unit stopped
    box.wait_until_succeeds(
        "journalctl -o cat _SYSTEMD_INVOCATION_ID=$(systemctl show -P InvocationID dd-agent.service)"
        f" | grep -c -F '{text}'",
        timeout=30,
    )


def gone(unit):
    """not running, as systemd says it: `! systemctl is-active` also passes
    when systemctl cannot reach a systemd that is re-executing"""
    return f'case "$(systemctl show -P ActiveState {unit})" in inactive|failed) ;; *) false ;; esac'


# release 1 names what the box already runs: nothing to do, counter kept
publish(1, f"{nix['base']}")
agent()
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 1 ]")

# release 2 moves it, and leaves no guard behind
publish(2, f"{nix['marked']}")
agent()
box.succeed(gone("dd-agent-guard.timer"))
box.succeed(f"[ $(readlink /run/current-system) = {nix['marked']} ]")
box.succeed("grep -q 'release 2' /etc/dd-marker")
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 2 ]")
box.succeed("grep -q 'dd_agent_counter{box=\"box\"} 2' /var/lib/dd-facts/dd_agent.prom")

# an older release is refused, whatever it names
publish(1, f"{nix['base']}")
agent(expect_ok=False)
box.succeed(f"[ $(readlink /run/current-system) = {nix['marked']} ]")
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 2 ]")

# a file the key did not sign is refused
publish(5, f"{nix['base']}", tamper=True)
agent(expect_ok=False)
box.succeed(f"[ $(readlink /run/current-system) = {nix['marked']} ]")
said("release refused")

# a release that leaves the box without ssh comes back by itself
publish(3, f"{nix['broken']}")
agent(expect_ok=False)
box.succeed(f"[ $(readlink /run/current-system) = {nix['marked']} ]")
box.wait_for_unit("sshd.service")
box.succeed("grep -q 'release 2' /etc/dd-marker")
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 2 ]")
box.succeed("grep -q 'result=\"rollback\"' /var/lib/dd-facts/dd_agent.prom")
box.succeed(gone("dd-agent-guard.service"))
box.succeed(gone("dd-agent-guard.timer"))

# a release that leaves the box healthy but cut off from the fleet is not a
# release this box keeps: the switch may not take away what it had
publish(4, f"{nix['stranded']}")
agent(expect_ok=False)
box.succeed(f"[ $(readlink /run/current-system) = {nix['marked']} ]")
box.succeed("grep -q 'release 2' /etc/dd-marker")
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 2 ]")
said("could no longer reach the fleet")

# a file the key did not sign leaves the counter this box applied on its
# status, not 0
publish(7, f"{nix['base']}", tamper=True)
agent(expect_ok=False)
box.succeed("grep -q 'dd_agent_counter{box=\"box\"} 2' /var/lib/dd-facts/dd_agent.prom")
box.succeed("grep -q 'result=\"refused\"' /var/lib/dd-facts/dd_agent.prom")

# one counter is one system: release 2 again, naming something else, is
# refused; release 2 published again unchanged is taken
publish(2, f"{nix['base']}")
agent(expect_ok=False)
box.succeed(f"[ $(readlink /run/current-system) = {nix['marked']} ]")
said("applied 2 as something else")
publish(2, f"{nix['marked']}")
agent()
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 2 ]")
box.succeed(f"grep -qx '2 {nix['marked']}' /var/lib/dd-agent/applied")

# the guard: armed for the whole decision (2 x 20 s of probe and reach,
# plus 300), and when it goes off it puts back the profile as well as the
# system. The agent is killed mid-probe, as a crash would, and the guard
# is fired at once rather than waited for.
probes = int(
    box.succeed("journalctl -u dd-agent -o cat | grep -c '^probe: ' || true").strip()
    or 0
)
publish(6, f"{nix['broken']}")
box.succeed("systemctl start --no-block dd-agent.service")
box.wait_until_succeeds(
    f"[ $(journalctl -u dd-agent -o cat | grep -c '^probe: ') -gt {probes} ]",
    timeout=300,
)
box.succeed(
    "systemctl show -P TimersMonotonic dd-agent-guard.timer | grep -q 'OnActiveUSec=5min 40s'"
)
box.succeed(f"[ $(readlink -f /nix/var/nix/profiles/system) = {nix['broken']} ]")
box.succeed("systemctl kill -s KILL dd-agent.service")
box.wait_until_succeeds(gone("dd-agent.service"))
box.succeed("systemctl start --no-block dd-agent-guard.service")
box.wait_until_succeeds(
    f"[ $(readlink /run/current-system) = {nix['marked']} ]", timeout=300
)
box.wait_until_succeeds(gone("dd-agent-guard.service"), timeout=300)
box.succeed(f"[ $(readlink -f /nix/var/nix/profiles/system) = {nix['marked']} ]")
box.succeed("grep -q 'release 2' /etc/dd-marker")
box.succeed(
    "systemctl stop dd-agent-guard.timer || true; systemctl reset-failed dd-agent-guard.timer dd-agent-guard.service dd-agent.service || true"
)
