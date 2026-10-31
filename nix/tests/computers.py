# The computers test: a member's computer runs where its flag is, from the
# agreed record, with the keys that record lets in; it keeps out of the box;
# the fence stops it when its lease may have lapsed; a stop is clean and
# /home outlives it; draining zeroes what the box offers.

import json

ID = "0123456789abcdef"
d = f"/var/lib/private/dd-computer/{ID}"
flag = f"/run/dd-placed/computer-{ID}"
sock = "--unix-socket /run/dd-computers/dd-computers.sock"

box.wait_for_unit("control-stand-in.service")
box.wait_for_unit("dd-computers.service")
box.wait_until_succeeds("test -S /run/dd-computers/dd-computers.sock", timeout=60)

for k in ("one", "two"):
    box.succeed(f"ssh-keygen -q -t ed25519 -N '' -C {k} -f /root/{k}")
one = box.succeed("cat /root/one.pub").strip()
two = box.succeed("cat /root/two.pub").strip()


def record(up=True, keys=None, mem=1024, cid=ID, port=27200):
    return {
        "owner": "tom",
        "id": cid,
        "name": "dev",
        "config": "default",
        "memory": mem,
        "threads": 1,
        "disk": 8,
        "nix": "",
        "players": [],
        "keys": keys if keys is not None else [one],
        "consent": "own",
        "anywhere": False,
        "port": port,
        "forwards": [],
        "asked": up,
        "up": up,
    }


def agree(*computers):
    """what dd-control would serve: the agreed computers, and the plan"""
    view = {"computers": {c["id"]: c for c in computers}, "configs": {}}
    plan = {"boxes": {"box": [{"service": f"computer-{c['id']}"} for c in computers]}}
    box.succeed(f"echo '{json.dumps(view)}' > /var/lib/stand-in/view.json")
    box.succeed(f"echo '{json.dumps(plan)}' > /var/lib/stand-in/plan.json")


def uptime_ms():
    return int(float(box.succeed("cut -d' ' -f1 /proc/uptime")) * 1000)


def hold(until=None):
    """the flag dd-control raises while it holds the lease"""
    u = until if until is not None else uptime_ms() + 10**9
    box.succeed(f"echo '{json.dumps({'epoch': 1, 'until': u})}' > {flag}")


def api(user, path):
    out = box.succeed(
        f"curl -s -w '\\n%{{http_code}}' {sock} -H 'x-dd-user: {user}' -H 'x-dd-role: member' http://compute{path}"
    )
    text, code = out.rsplit("\n", 1)
    return int(code), text


agree(record())

# no flag: no computer, and nothing prepared for one
box.succeed(f"systemctl start dd-computer@{ID}.service")
assert "inactive" in box.succeed(
    f"systemctl show -p ActiveState --value dd-computer@{ID}.service"
)
box.succeed(f"systemctl start dd-computer-prepare@{ID}.service")
box.fail(f"test -e {d}/record.json")

# held here, but over its owner's total with another of theirs here: refused
other = "fedcba9876543210"
agree(record(), record(cid=other, mem=1024, port=27201))
box.succeed(f"echo '{{}}' > /run/dd-placed/computer-{other}")
hold()
box.fail(f"systemctl start dd-computer-prepare@{ID}.service")
box.succeed(f"rm /run/dd-placed/computer-{other}")
agree(record())

# started as dd-control starts it
box.succeed(f"systemctl start --no-block dd-computer@{ID}.service")
# every ssh is bounded: a guest that stops answering fails the step in a
# minute, with what it said, never the run at its global limit
ssh = (
    "timeout 60 ssh -i /root/{k} -p 27200 -o StrictHostKeyChecking=no"
    " -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 -o IdentitiesOnly=yes"
    " -o ServerAliveInterval=5 -o ServerAliveCountMax=3 tom@127.0.0.1"
)
one_ssh = ssh.format(k="one")
two_ssh = ssh.format(k="two")
try:
    box.wait_until_succeeds(f"{one_ssh} true", timeout=900)
except Exception:
    print(box.execute(f"tail -c 20000 {d}/console.log")[1])
    raise
# its first boot is the system the box built: no rebuild of its own (one
# takes most of a small computer's memory, and once starved its sshd)
box.fail(f"{one_ssh} test -e /var/lib/dd/apply.log")
rec = json.loads(box.succeed(f"cat {d}/record.json"))
assert rec["owner"] == "tom" and rec["port"] == 27200, rec

# theirs: wheel, sudo, /home on its own disk; the other key not let in
assert "wheel" in box.succeed(f"{one_ssh} id")
box.succeed(f"{one_ssh} sudo -n true")
home = box.succeed(f"{one_ssh} findmnt -no SOURCE /home").strip()
root = box.succeed(f"{one_ssh} findmnt -no SOURCE /").strip()
assert home != root and home.startswith("/dev/vd"), (home, root)
box.fail(f"{two_ssh} true")

# a user of its own: its directory is its alone, and another computer's is
# out of its reach
assert box.succeed(f"stat -c %a {d}").strip() == "700"
B = "00000000000000b2"
agree(record(), record(cid=B, mem=512, port=27201) | {"owner": "ann"})
box.succeed(f"echo '{{}}' > /run/dd-placed/computer-{B}")
box.succeed(f"systemctl start dd-computer-prepare@{B}.service")
box.succeed(f"test -s /var/lib/private/dd-computer/{B}/record.json")
# on disk both are 0700 and nobody's: systemd maps each into its own
# computer's units alone (an id-mapped StateDirectory)
assert box.succeed(f"stat -c %a /var/lib/private/dd-computer/{B}").strip() == "700"
a_uid = box.succeed(f"id -u dd-computer-{ID}").strip()
box.fail(f"runuser -u dd-computer-{ID} -- ls /var/lib/private/dd-computer/{B}")
box.fail(
    f"runuser -u dd-computer-{ID} -- cat /var/lib/private/dd-computer/{B}/config/owner.json"
)
box.fail(
    f"setpriv --reuid={a_uid} --regid={a_uid} --clear-groups cat /var/lib/private/dd-computer/{B}/record.json"
)
box.succeed(f"rm /run/dd-placed/computer-{B}")
agree(record())

# out of the box: its loopback, its own addresses (one outside the private
# ranges, as a public IPv6 one would be), anything private
box.succeed("ip addr add 203.0.113.5/32 dev lo")
box.succeed("systemd-run --unit probe python3 -m http.server --bind 0.0.0.0 4999")
box.wait_until_succeeds("curl -sf -m 3 -o /dev/null http://127.0.0.1:4999/")
box.succeed("curl -sf -m 3 -o /dev/null http://203.0.113.5:4999/")
box.fail(f"{one_ssh} curl -sf -m 5 -o /dev/null http://10.0.2.2:4999/")
box.fail(f"{one_ssh} curl -sf -m 5 -o /dev/null http://203.0.113.5:4999/")
box.fail(f"{one_ssh} curl -sf -m 5 -o /dev/null http://10.0.2.2:4195/computers")
box.fail(f"{one_ssh} curl -sf -m 5 -o /dev/null http://192.168.1.1/")

# a key the owner saves reaches it without a restart, and the old one goes
agree(record(keys=[two]))
box.succeed("systemctl start dd-computers-share.service")
box.wait_until_succeeds(f"{two_ssh} true", timeout=120)
box.wait_until_fails(f"{one_ssh} true", timeout=120)

# what the page shows its owner, and nobody else
code, text = api("tom", "/api/state")
mine = json.loads(text)
assert (
    code == 200 and mine["computers"][0]["id"] == ID and mine["computers"][0]["here"]
), text
assert mine["computers"][0]["box"] == "box", text
code, text = api("ann", "/api/state")
assert code == 200 and json.loads(text)["computers"] == [], text
assert api("", "/api/state")[0] == 401

# the firewall's filler is told of it, its usage counted
box.wait_until_succeeds(
    "jq -e '.servers[] | select(.id == \""
    + ID
    + "\") | .ports == [27200]' /run/dd-computers/access.json",
    timeout=30,
)
# who may reach it: the firewall's chain from the gate's answer; one that
# does not read closes every port, never leaves the ones before
box.succeed("systemctl stop dd-verify.service")
box.succeed("systemd-run --unit fake-gate python3 /etc/fake-gate.py")
box.succeed(
    """echo '[{"players": ["tom"], "addresses": ["100.64.0.9"]}]' > /var/lib/stand-in/gate.json"""
)
box.wait_until_succeeds("curl -sf -X POST -d '[]' http://127.0.0.1:4181/", timeout=30)
box.succeed("systemctl start computers-access.service")
assert int(box.succeed("iptables -S dd-computers | grep -c 100.64.0.9 || true")) == 2
box.succeed(
    """echo '[{"players": ["tom"], "addresses": "oops"}]' > /var/lib/stand-in/gate.json"""
)
box.succeed("systemctl start computers-access.service")
assert int(box.succeed("iptables -S dd-computers | grep -c -- '-A' || true")) == 0
box.succeed("systemctl stop fake-gate.service")

box.succeed("systemctl start dd-computers-usage.service")
used = box.succeed("cat /var/lib/dd-facts/usage-computers.facts")
assert used.startswith("usage/computers/tom ") and int(used.split()[1]) > 0, used
# whose it is comes from the agreed state, never its own directory: a record
# its user rewrote bills nobody else
box.succeed(f"""echo '{{"owner": "ann"}}' > {d}/record.json""")
box.succeed("systemctl start dd-computers-usage.service")
again = box.succeed("cat /var/lib/dd-facts/usage-computers.facts")
counts = lambda t: dict(line.split() for line in t.splitlines())
# ann's little computer from above counts what it did; tom's disks stay his
assert counts(again).get("usage/computers/ann") == counts(used).get(
    "usage/computers/ann"
), (used, again)
assert int(counts(again)["usage/computers/tom"]) > 0, again
assert box.succeed(f"stat -c %U /var/lib/dd-computer-owners/{ID}").strip() == "root"

# cut off: its lease may have lapsed, and the fence stops it and takes its
# flag down, whether dd-control is there or not
box.succeed(f"{two_ssh} 'echo kept > ~/kept'")
# and something of the system's own, outside /home, with the key its ssh
# knows it by
box.succeed(f"{two_ssh} 'sudo touch /var/lib/dd/old-system && sync'")
hostkey = lambda: box.succeed(
    "ssh-keyscan -t ed25519 -p 27200 127.0.0.1 2>/dev/null | grep -v '^#' | cut -d' ' -f2-"
).strip()
known = hostkey()
assert known.startswith("ssh-ed25519 "), known
assert box.succeed(f"cat {d}/system.base").strip().endswith("-dd-computer-image")
hold(until=uptime_ms() - 1)
box.succeed("systemctl start dd-placed-fence.service")
assert "inactive" in box.succeed(
    f"systemctl show -p ActiveState --value dd-computer@{ID}.service"
)
box.fail(f"test -e {flag}")

# held again: it comes back with /home as it was. Its system disk is
# the release's: one made from another image (or before the image it came
# from was kept, as here) is made again, and /home and its host keys stay
box.succeed(f"rm {d}/system.base")
hold()
box.succeed(f"systemctl start --no-block dd-computer@{ID}.service")
box.wait_until_succeeds(f"{two_ssh} true", timeout=600)
assert box.succeed(f"{two_ssh} cat kept").strip() == "kept"
box.fail(f"{two_ssh} test -e /var/lib/dd/old-system")
assert hostkey() == known
box.succeed(f"test -e $(cat {d}/system.base)/nixos.qcow2")

# powered off: dd-control stops it, waits, then its flag goes; a clean stop
agree(record(up=False, keys=[two]))
box.succeed(f"systemctl stop dd-computer@{ID}.service")
box.succeed(f"rm {flag}")
assert "Result=success" in box.succeed(
    f"systemctl show -p Result dd-computer@{ID}.service"
)
# and agreed down, it is not prepared again, its flag or not
hold()
box.fail(f"systemctl start dd-computer-prepare@{ID}.service")
box.succeed(f"rm {flag}")

# what the box offers placement: its room, and what each computer holds;
# all zeros while it drains
box.succeed("systemctl start dd-computer-facts.service")
facts = box.succeed("cat /var/lib/dd-facts/computers.prom")
assert "dd_computer_max_mem_bytes 0" not in facts, facts
assert f'dd_computer_used_bytes{{computer="{ID}"}} ' in facts, facts
assert box.succeed("stat -c %a /var/lib/dd-facts/computers.prom").strip() == "644"
box.succeed("echo 'dd_box_draining 1' > /var/lib/dd-facts/draining.prom")
box.succeed("systemctl start dd-computer-facts.service")
facts = box.succeed("cat /var/lib/dd-facts/computers.prom")
for name in ("mem_bytes", "threads", "disk_bytes"):
    assert f"dd_computer_max_{name} 0\n" in facts, facts
assert int(box.succeed("ls /var/lib/dd-facts | grep -c '^computers.prom'")) == 1

# ---- the whole road, as a member takes it from their laptop: `dd computer`
# through nginx and the gate, judged, placed and leased by the box's own
# dd-control with the relay's witness, started here, and SSH into it
box.succeed("rm /var/lib/dd-facts/draining.prom")
box.succeed("systemctl start dd-computer-facts.service")
box.succeed("systemctl stop control-stand-in.service")
box.succeed("rm -f /run/dd-placed/computer-*")

# somewhere on the web, as the guest sees it: not the box, not a private
# network (a namespace of its own, at a documentation address)
box.succeed(
    "ip netns add web && ip link add web0 type veth peer name web1 netns web"
    " && ip addr add 198.51.100.1/24 dev web0 && ip link set web0 up"
    " && ip -n web addr add 198.51.100.2/24 dev web1 && ip -n web link set web1 up"
    " && ip -n web link set lo up && mkdir -p /srv/web && echo reached > /srv/web/hello"
)
box.succeed("systemctl start web-far.service")
box.wait_until_succeeds("curl -sf -m 3 http://198.51.100.2/hello")

box.succeed(
    "mkdir -p /root/f/w && cd /root/f"
    f" && cp {nix['worker']}/worker.js {nix['worker']}/wasm_exec.js {nix['worker']}/witness.wasm w/"
    " && chmod -R u+w ."
)
box.succeed(
    "cat > /root/f/w/wrangler.toml <<'EOF'\n"
    'name = "computers"\nmain = "worker.js"\ncompatibility_date = "2026-05-01"\nno_bundle = true\n'
    'rules = [\n  { type = "CompiledWasm", globs = ["**/*.wasm"] },\n  { type = "ESModule", globs = ["**/*.js"] },\n]\n'
    '[[durable_objects.bindings]]\nname = "RELAY"\nclass_name = "Relay"\n'
    '[[migrations]]\ntag = "v1"\nnew_sqlite_classes = ["Relay"]\nEOF'
)
box.succeed(
    "systemd-run --unit fleet -p WorkingDirectory=/root/f"
    " -E HOME=/root -E WRANGLER_SEND_METRICS=false -E CI=1 -E CLOUDFLARE_CF_FETCH_ENABLED=false"
    " -E PATH=/run/current-system/sw/bin"
    f" python3 {nix['fleet']} {nix['control']}"
)
box.wait_until_succeeds(
    "test -e /root/f/ready || systemctl is-failed -q fleet.service", timeout=600
)
if box.execute("test -e /root/f/ready")[0] != 0:
    print(
        box.execute(
            "journalctl -u fleet --no-pager | tail -40; tail -40 /root/f/relay.log"
        )[1]
    )
    raise AssertionError("the test fleet did not come up")
box.succeed("systemctl start dd-control.service")
box.wait_until_succeeds(
    "curl -sf http://127.0.0.1:4195/state | jq -e '.boxes | has(\"box\")'", timeout=180
)
# the gate, now reading who is who from the agreed state
box.succeed("systemctl restart dd-verify.service")
box.wait_for_open_port(4181)
box.wait_until_succeeds(
    "curl -sf -X PUT -H 'content-type: application/json' --data-binary @/root/f/entry.json"
    " http://127.0.0.1:4181/_dd/directory/tom",
    timeout=180,
)
box.wait_until_succeeds(
    "curl -sf http://127.0.0.1:4195/identity | jq -e '.members.tom.status == \"member\"'",
    timeout=120,
)

# nobody signed in: sent to sign in, never nginx's 500 (compute.<domain>
# had no gate in front of it until the gate's hosts named it)
code = box.succeed(
    "curl -sk -o /dev/null -w '%{http_code}' --resolve compute.test.invalid:443:127.0.0.1"
    " https://compute.test.invalid/api/state"
).strip()
assert code == "302", code

box.succeed(
    "mkdir -p /root/.ssh && printf 'Host *\\n  StrictHostKeyChecking no\\n"
    "  UserKnownHostsFile /dev/null\\n  ConnectTimeout 5\\n' > /root/.ssh/config"
)
tom = (
    f"HOME=/root DD_KEYRING_FILE=/root/tom.json {nix['dd']} computer"
    " --directory http://home.test.invalid:8080/_dd/directory"
)


def listed(text):
    """whether `dd computer list` shows this line, now"""
    return box.execute(f"{tom} list > /tmp/list && grep -cF '{text}' /tmp/list")[0] == 0


# a config, with a Nix file (the empty one the image already applied, so the
# guest does not rebuild offline), and a computer made from it
box.succeed("printf '{ ... }: { }\\n' > /root/dev.nix")
box.succeed(f"{tom} config dev --memory 1024 --threads 1 --disk 8 --nix /root/dev.nix")
box.wait_until_succeeds(
    f"{tom} list > /tmp/list && grep -c '^config dev .*my boxes' /tmp/list", timeout=120
)
# anyone's box is asked first, and a no makes nothing
box.succeed(f"{tom} config wide --memory 1024 --threads 1 --disk 8 --anywhere")
box.wait_until_succeeds(
    f'{tom} list > /tmp/list && grep -c "^config wide .*anyone\'s box" /tmp/list',
    timeout=120,
)
out = box.fail(f"echo n | {tom} make wide nope 2>&1")
assert "someone else's box" in out and "not made" in out, out
box.succeed(f"{tom} make dev")
try:
    box.wait_until_succeeds(
        f"{tom} list > /tmp/list && grep -c 'on box, ssh -p 27200 tom@127.0.0.1' /tmp/list",
        timeout=300,
    )
except Exception:
    print(
        box.execute(
            "curl -s http://127.0.0.1:4195/plan; journalctl -u dd-control --no-pager | tail -30"
        )[1]
    )
    raise
# against this box's limits (dd.computers.memoryMiB above; the policy's
# on a real box)
assert listed("using 1.0 GiB of 1.5 GiB, 1 of 8 threads, 8 of 500 GiB"), box.succeed(
    "cat /tmp/list"
)
assert not listed(" nope ")
cid = box.succeed(
    "curl -sf http://127.0.0.1:4195/computers"
    " | jq -er '.computers | to_entries[] | select(.value.name == \"dev\") | .key'"
).strip()
assert box.succeed("stat -c %a /root/.config/commonty/computers").strip() == "700"
assert (
    box.succeed("stat -c %a /root/.config/commonty/computers/dev.device-key").strip()
    == "600"
)

# SSH in, with the key `dd` made on this device and put in the signed item
ssh = f"timeout 60 env {tom} ssh dev"
try:
    box.wait_until_succeeds(f"{ssh} true", timeout=900)
except Exception:
    print(
        box.execute(
            f"systemctl status dd-computer@{cid} --no-pager; tail -c 20000 /var/lib/private/dd-computer/{cid}/console.log"
        )[1]
    )
    raise
assert box.succeed(f"{ssh} whoami").strip() == "tom"
# the web, from inside
assert (
    box.succeed(f"{ssh} curl -sf -m 10 http://198.51.100.2/hello").strip() == "reached"
)
box.fail(f"{ssh} test -e /var/lib/dd/apply.log")

# ---- the Commonty network: a control server out on the web (in the web
# namespace), the box on it as a box, and a key for the computer handed
# into its share as its box hands one in (computers.nix, netJoin), as the
# computer's own user, 0600
box.succeed("systemctl start headscale.service")
box.wait_until_succeeds("headscale users list >/dev/null", timeout=60)
box.succeed("headscale users create boxes")
boxes = box.succeed(
    "headscale users list -o json | jq -r '.[] | select(.name == \"boxes\") | .id'"
).strip()


def net_key(tag):
    return (
        box.succeed(
            f"headscale preauthkeys create --user {boxes} --expiration 30m --tags tag:{tag}"
        )
        .strip()
        .splitlines()[-1]
    )


box.succeed(
    f"tailscale up --login-server=https://198.51.100.2 --auth-key={net_key('box')}"
    " --hostname=box --accept-dns=false --timeout=60s"
)
box_ip = box.succeed("tailscale ip -4").strip()
# a port of the box's that the rules give a computer no way to (its
# firewall lets it in: the rules decide)
box.succeed(
    f"systemd-run --unit fleet-4998 python3 -m http.server --bind {box_ip} 4998 --directory /srv/web"
)
box.wait_until_succeeds(f"curl -sf -m 3 http://{box_ip}:4998/hello")
net = json.dumps(
    {
        "key": net_key("computer"),
        "control_url": "https://198.51.100.2",
        "hostname": f"computer-{cid}",
        "expires": 0,
    }
)
box.succeed(f"echo '{net}' > /root/net.json")
share = f"/var/lib/private/dd-computer/{cid}/config"
# in the computer's own mount namespace: there alone is its directory its
# user's (systemd's id-mapped StateDirectory)
box.succeed(
    f"nsenter -t $(systemctl show -p MainPID --value dd-computer@{cid}.service) -m --"
    f' runuser -u dd-computer-{cid} -- sh -c \'umask 077 && cat > "$1.new" && mv "$1.new" "$1"\''
    f" - {share}/net.json < /root/net.json"
)
box.succeed("rm /root/net.json")
assert box.succeed(f"stat -c %a {share}/net.json").strip() == "600"

# stopped and started again from the laptop: /home is as it was
box.succeed(f"{ssh} 'echo kept > ~/kept && sync'")
box.succeed(f"{tom} stop dev")
box.wait_until_succeeds(
    f"! systemctl is-active -q dd-computer@{cid}.service && test ! -e /run/dd-placed/computer-{cid}",
    timeout=240,
)
box.wait_until_succeeds(
    f"{tom} list > /tmp/list && grep -c '^dev  *stopped' /tmp/list", timeout=60
)
box.succeed(f"{tom} start dev")
box.wait_until_succeeds(f"{ssh} true", timeout=900)
assert box.succeed(f"{ssh} cat kept").strip() == "kept"

# the bridge to the control server reads the key's file as root and runs as
# its own user: up, never restarted (it once could not read the file, 0600,
# and restarted for good)
box.wait_until_succeeds(f"{ssh} systemctl is-active -q dd-netbridge", timeout=60)
assert (
    box.succeed(f"{ssh} systemctl show -p NRestarts --value dd-netbridge").strip()
    == "0"
)
assert box.succeed(f"{ssh} sudo cat /run/dd-netbridge/control_url").strip() == (
    "https://198.51.100.2"
)
# the test's control server has the test's certificate, which the owner
# tells their computer to trust (a real one's is the web's)
box.succeed(f"{ssh} 'sudo tee /etc/ssl/certs/dd-test.pem >/dev/null' < {nix['ca']}")
box.succeed(f"{ssh} sudo systemctl restart dd-netbridge tailscaled")
try:
    box.wait_until_succeeds(
        f"{ssh} sudo systemctl start dd-net && {ssh} sudo tailscale status --json"
        " | jq -e '.BackendState == \"Running\"'",
        timeout=300,
    )
except Exception:
    print(
        box.execute(
            f"{ssh} 'systemctl status --no-pager -n 30 dd-net dd-netbridge tailscaled'"
        )[1]
    )
    raise
# joined as itself, a computer, and on to a box's door; nothing else of it
node = json.loads(
    box.succeed(
        f"headscale nodes list -o json | jq -c '.[] | select(.name == \"computer-{cid}\")'"
    )
)
assert "tag:computer" in json.dumps(node), node
box.wait_until_succeeds(
    f"{ssh} curl -sk -m 10 -o /dev/null -w '%{{http_code}}'"
    f" --resolve compute.test.invalid:443:{box_ip} https://compute.test.invalid/api/state"
    " > /tmp/code && grep -cx 302 /tmp/code",
    timeout=180,
)
box.fail(f"{ssh} curl -sf -m 10 http://{box_ip}:4998/hello")

# ---- a live apply: a changed Nix file reaches the running computer, is
# built and switched to without a restart, and the apply ends (once, the
# switch waited on multi-user.target, which waited on the apply)
boot = box.succeed(f"{ssh} cat /proc/sys/kernel/random/boot_id").strip()
system = box.succeed(f"{ssh} readlink /run/current-system").strip()
box.succeed(
    "NIX_SSHOPTS='-p 27200 -i /root/.config/commonty/computers/dev.device-key -o IdentitiesOnly=yes'"
    " nix --extra-experimental-features nix-command copy --no-check-sigs"
    " --to 'ssh-ng://tom@127.0.0.1?remote-program=/run/current-system/sw/bin/nix-daemon'"
    f" {nix['appliedInputs']}"
)
box.succeed(
    f"{tom} config dev --memory 1024 --threads 1 --disk 8 --nix {nix['applied']}"
)
# apply reads the config as agreed: not before the new one is
box.wait_until_succeeds(
    "curl -sf http://127.0.0.1:4195/computers > /tmp/view && grep -c dd-applied /tmp/view",
    timeout=120,
)
box.succeed(f"{tom} apply dev")
try:
    box.wait_until_succeeds(
        f"{ssh} cat /etc/dd-applied > /tmp/out && grep -cx applied /tmp/out",
        timeout=900,
    )
    box.wait_until_succeeds(
        f"{ssh} systemctl show -p ActiveState --value dd-apply > /tmp/out"
        " && grep -cx inactive /tmp/out",
        timeout=300,
    )
except Exception:
    print(
        box.execute(
            f"{ssh} 'systemctl list-jobs; sudo tail -40 /var/lib/dd/apply.log'"
        )[1]
    )
    raise
assert box.succeed(f"{ssh} cat /proc/sys/kernel/random/boot_id").strip() == boot
assert box.succeed(f"{ssh} readlink /run/current-system").strip() != system
assert (
    box.succeed(f"{ssh} systemctl show -p Result --value dd-apply").strip() == "success"
)
want = box.succeed(f"sha256sum < {nix['applied']} | cut -d' ' -f1").strip()
said = json.loads(
    box.succeed(
        f"head -c 4096 /var/lib/private/dd-computer/{cid}/report.img | tr -d '\\0'"
    )
)
assert said["hash"] == want and said["ok"] is True, said
# and the next one runs: the apply is not stuck
box.succeed(f"{ssh} sudo systemctl start dd-apply")
