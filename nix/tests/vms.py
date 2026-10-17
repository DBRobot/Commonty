# The vms test: a member's computer boots, is theirs, keeps /home, and keeps out of the box.

import json

d = "/var/lib/dd-vms"
box.wait_for_unit("multi-user.target")
box.succeed("ssh-keygen -q -t ed25519 -N '' -f /root/key")
key = box.succeed("cat /root/key.pub").strip()
record = {
    "owner": "tom",
    "name": "dev",
    "memory": 1024,
    "threads": 1,
    "disk": 8,
    "port": 27200,
    "keys": [key],
    "up": True,
}
box.succeed(f"cat > {d}/records/t1.json <<'J'\n{json.dumps(record)}\nJ")
box.succeed(f"chown dd-vms:dd-vms {d}/records/t1.json")
ssh = "ssh -i /root/key -p 27200 -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 tom@127.0.0.1"

# the record's up, so the boot job starts it, as a reboot would
box.succeed("systemctl start dd-vms-up.service")
box.wait_until_succeeds(f"{ssh} true", timeout=1200)

# theirs: wheel, sudo, /home on its own disk
assert "wheel" in box.succeed(f"{ssh} id")
box.succeed(f"{ssh} sudo -n true")
home = box.succeed(f"{ssh} findmnt -no SOURCE /home").strip()
root = box.succeed(f"{ssh} findmnt -no SOURCE /").strip()
assert home != root and home.startswith("/dev/vd"), (home, root)

# its Nix file applied, and the system it built is the one the host made:
# nothing to build, so nothing to fetch
box.wait_until_succeeds(f"test -s {d}/instances/t1/report/apply.json", timeout=600)
applied = json.loads(box.succeed(f"cat {d}/instances/t1/report/apply.json"))
assert applied["ok"], box.succeed(f"cat {d}/instances/t1/report/apply.log")

# out of the box: its loopback (here, a listener the box itself reaches) is
# refused, and so is anything private
box.succeed("systemd-run --unit probe python3 -m http.server --bind 127.0.0.1 4999")
box.wait_until_succeeds("curl -sf -m 3 -o /dev/null http://127.0.0.1:4999/")
box.fail(f"{ssh} curl -sf -m 5 -o /dev/null http://10.0.2.2:4999/")
box.fail(f"{ssh} curl -sf -m 5 -o /dev/null http://192.168.1.1/")

# /home outlives a restart, and the stop is a clean one
box.succeed(f"{ssh} 'echo kept > ~/kept'")
box.succeed("systemctl stop dd-vm@t1.service")
assert "Result=success" in box.succeed("systemctl show -p Result dd-vm@t1.service")
box.succeed("systemctl start dd-vm@t1.service")
box.wait_until_succeeds(f"{ssh} true", timeout=600)
assert box.succeed(f"{ssh} cat kept").strip() == "kept"

