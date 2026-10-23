# The switch-remote test: neither a forge that is down nor an ACME order
# that hangs holds up a switch, fails it, or rolls it back; and the box
# learns each release wherever it is kept.

import json

start_all()
forge.wait_for_unit("nginx.service")
forge.wait_for_unit("relay.service")
forge.wait_for_unit("blackhole.service")
forge.wait_for_open_port(8080)
forge.wait_for_open_port(14000)
box.wait_for_unit("dd-verify.service")
box.wait_for_open_port(4181)
box.wait_for_unit("sshd.service")
box.wait_for_unit("nginx.service")
# the test runs the agent itself (as the release test does)
box.succeed(
    "mkdir -p /run/systemd/system/dd-agent.timer.d && "
    "printf '[Timer]\\nOnActiveSec=\\nOnUnitActiveSec=\\nOnActiveSec=1d\\n' "
    "> /run/systemd/system/dd-agent.timer.d/test.conf && systemctl daemon-reload"
)

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


def sign(counter, path):
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


def on_forge():
    forge.succeed(
        "mkdir -p /srv/release && install -m 644 /root/current.json /srv/release/current.json"
    )


def on_relay(file="/root/current.json"):
    forge.succeed(
        f"curl -fsS -X PUT --data-binary @{file} http://127.0.0.1:8080/release"
    )


def relay_counter():
    return json.loads(forge.succeed("cat /srv/relay/release"))["payload"]["counter"]


def gone(unit):
    return f'case "$(systemctl show -P ActiveState {unit})" in inactive|failed) ;; *) false ;; esac'


# where the journal stood as the last run began
run = {"since": ""}


def agent(expect_ok=True):
    run["since"] = box.succeed(
        "journalctl -n 1 -o export | sed -n 's/^__CURSOR=//p'"
    ).strip()
    before = box.succeed("systemctl show -P InvocationID dd-agent.service").strip()
    box.succeed("systemctl start --no-block dd-agent.service")
    box.wait_until_succeeds(
        "id=$(systemctl show -P InvocationID dd-agent.service) && "
        f'[ -n "$id" ] && [ "$id" != "{before}" ] && ' + gone("dd-agent.service"),
        timeout=600,
    )
    result = box.succeed("systemctl show -P Result dd-agent.service").strip()
    assert (result == "success") == expect_ok, (result, run_log())


# a run that ends before journald reads its output (no source answering:
# 40 ms) leaves its lines no invocation id; the agent's name since the run
# began finds them (CI, 2026-10-23)
def run_log():
    return box.succeed(
        f"journalctl --sync && journalctl -o cat --after-cursor='{run['since']}' "
        "_SYSTEMD_INVOCATION_ID=$(systemctl show -P InvocationID dd-agent.service) "
        "+ SYSLOG_IDENTIFIER=dd-agent"
    )


def said(text):
    box.wait_until_succeeds(
        f"journalctl --sync && journalctl -o cat --after-cursor='{run['since']}' "
        "_SYSTEMD_INVOCATION_ID=$(systemctl show -P InvocationID dd-agent.service) "
        f"+ SYSLOG_IDENTIFIER=dd-agent | grep -c -F '{text}'",
        timeout=30,
    )


# release 1, from the forge: the agent hands it to the relay, which has none
sign(1, nix["base"])
on_forge()
agent()
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 1 ]")
said("handed to http://forge:8080/release")
assert relay_counter() == 1

# the certificate's order is a job of its own, never started by a switch
# or wanted by the certificate's unit, and its timer starts it after boot.
# It asks the ACME server, which takes the connection and never answers
box.succeed(f'[ -z "$(systemctl show -P WantedBy {nix["order"]})" ]')
box.succeed(
    f"systemctl show -P Wants {nix['cert']} > /tmp/wants; ! grep -c order-renew /tmp/wants"
)
box.succeed(f"systemctl start --no-block {nix['order']}")
box.wait_until_succeeds(
    f"[ $(systemctl show -P ActiveState {nix['order']}) = activating ]", timeout=60
)
box.wait_until_succeeds(
    "ss -Htn state established '( dport = :14000 )' | grep -c .", timeout=120
)

# the forge goes away: what is sent to it is dropped, as a box that is down
forge.succeed(
    "iptables -I INPUT -p tcp --dport 80 -j DROP && "
    "ip6tables -I INPUT -p tcp --dport 80 -j DROP"
)
box.fail("curl -sf --max-time 5 http://forge/current.json")

# release 2, signed while the forge is down, reaches the relay (a box that
# read it, or publish): the box switches to it. The switch changes the
# certificate's names while its order still hangs, and starts the runner,
# whose forge does not answer
sign(2, nix["next"])
on_relay()
agent()
box.succeed(f"[ $(readlink /run/current-system) = {nix['next']} ]")
box.succeed("grep -q next /etc/dd-marker")
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 2 ]")
box.succeed("grep -q 'result=\"ok\"' /var/lib/dd-facts/dd_agent.prom")
said("source down: http://forge/current.json")
said("now running")
box.succeed(gone("dd-agent-guard.timer"))

# the order hangs still, left alone by the switch, and its failure would be
# nobody's but the ACME server's
box.succeed(f"[ $(systemctl show -P ActiveState {nix['order']}) = activating ]")
box.succeed(f"grep -qx {nix['order']} /etc/dd/remote-jobs")
# the runner waits for its forge, running, never failed
box.wait_until_succeeds(
    "journalctl -u gitea-runner-box -o cat > /tmp/runner.log; grep -c 'does not answer' /tmp/runner.log",
    timeout=60,
)
box.succeed("[ $(systemctl show -P ActiveState gitea-runner-box.service) = active ]")
box.succeed("[ $(systemctl show -P NRestarts gitea-runner-box.service) = 0 ]")

# a hung order is ended, not left for ever; ended, it holds nothing
box.succeed(f"[ $(systemctl show -P TimeoutStartUSec {nix['order']}) = 15min ]")
box.succeed(f"systemctl kill -s KILL {nix['order']}")
box.wait_until_succeeds(gone(nix["order"]), timeout=60)

# release 3 again from the relay alone: one the box runs already is taken
# without a switch
sign(3, nix["next"])
on_relay()
agent()
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 3 ]")
said("already running")

# the forge back, with release 4, and the relay serving a file the key did
# not sign under a higher counter: the box takes 4 from the forge, and the
# relay is handed it
forge.succeed(
    "iptables -D INPUT -p tcp --dport 80 -j DROP && "
    "ip6tables -D INPUT -p tcp --dport 80 -j DROP"
)
sign(4, nix["later"])
on_forge()
forge.succeed(
    'sed \'s/"counter": 4/"counter": 99/\' /root/current.json > /root/forged.json'
)
on_relay("/root/forged.json")
agent()
box.succeed(f"[ $(readlink /run/current-system) = {nix['later']} ]")
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 4 ]")
said("source refused: http://forge:8080/release")
said("release 4: handed to http://forge:8080/release")
assert relay_counter() == 4

# and with neither answering, the box stays as it is and says so
forge.succeed("systemctl stop relay.service")
forge.succeed("systemctl stop nginx.service")
agent(expect_ok=False)
said("no source answered")
box.succeed(f"[ $(readlink /run/current-system) = {nix['later']} ]")
box.succeed("[ $(cat /var/lib/dd-agent/counter) = 4 ]")
