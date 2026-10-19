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
ssh = "ssh -i /root/{k} -p 27200 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 -o IdentitiesOnly=yes tom@127.0.0.1"
one_ssh = ssh.format(k="one")
two_ssh = ssh.format(k="two")
try:
    box.wait_until_succeeds(f"{one_ssh} true", timeout=900)
except Exception:
    print(box.execute(f"tail -c 20000 {d}/console.log")[1])
    raise
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
box.succeed(f"{two_ssh} sync")
hold(until=uptime_ms() - 1)
box.succeed("systemctl start dd-placed-fence.service")
assert "inactive" in box.succeed(
    f"systemctl show -p ActiveState --value dd-computer@{ID}.service"
)
box.fail(f"test -e {flag}")

# held again: it comes back with /home as it was
hold()
box.succeed(f"systemctl start --no-block dd-computer@{ID}.service")
box.wait_until_succeeds(f"{two_ssh} true", timeout=600)
assert box.succeed(f"{two_ssh} cat kept").strip() == "kept"

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
