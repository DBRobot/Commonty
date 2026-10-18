# The vms test: a member makes a computer from a config, it boots, is theirs, keeps /home, and keeps out of the box.

import json

d = "/var/lib/dd-vms"
sock = "--unix-socket /run/dd-vms/dd-vms.sock"


def api(user, method, path, body=None, role="member"):
    data = (
        f"-H 'content-type: application/json' --data-binary '{json.dumps(body)}'"
        if body is not None
        else ""
    )
    out = box.succeed(
        f"curl -s -w '\\n%{{http_code}}' {sock} -X {method} -H 'x-dd-user: {user}' -H 'x-dd-role: {role}' {data} http://compute{path}"
    )
    text, code = out.rsplit("\n", 1)
    return int(code), text


box.wait_for_unit("dd-vms.service")
box.wait_until_succeeds("test -S /run/dd-vms/dd-vms.sock", timeout=60)

# nobody: nothing. A guest: no computers.
assert api("", "GET", "/api/state")[0] == 401
config = {
    "memory": 1024,
    "threads": 1,
    "disk": 8,
    "consent": "own",
}
assert api("gus", "PUT", "/api/configs/dev", config, role="guest")[0] == 403

# tom's own boxes only: this box is the fleet owner's, so not here
assert api("tom", "PUT", "/api/configs/dev", config)[0] == 200
code, text = api("tom", "POST", "/api/instances", {"config": "dev", "name": "a"})
assert code == 422 and "isn't yours" in text, text
# anyone's box: refused without the yes, made with it
config["consent"] = "any"
assert api("tom", "PUT", "/api/configs/dev", config)[0] == 200
code, text = api("tom", "POST", "/api/instances", {"config": "dev", "name": "a"})
assert code == 422 and "yes" in text, text
code, text = api(
    "tom", "POST", "/api/instances", {"config": "dev", "name": "a", "yes": True}
)
assert code == 200, text
made = json.loads(text)
i, port = made["id"], made["port"]
# its own key, as its owner downloads it; nobody else gets it
code, private = api("tom", "GET", f"/api/instances/{i}/key")
assert code == 200 and private.startswith("-----BEGIN OPENSSH PRIVATE KEY-----"), (
    private
)
assert api("ann", "GET", f"/api/instances/{i}/key")[0] == 422
box.succeed(f"cat > /root/key <<'K'\n{private}\nK")
box.succeed("chmod 600 /root/key")
assert made["anywhere"], made
# over the totals: tom's computers together (1.5G here; 16G on a box)
code, text = api(
    "tom", "POST", "/api/instances", {"config": "dev", "name": "b", "yes": True}
)
assert code == 422 and "together" in text, text

ssh = f"ssh -i /root/key -p {port} -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null -o ConnectTimeout=5 tom@127.0.0.1"
try:
    box.wait_until_succeeds(f"{ssh} true", timeout=900)
except Exception:
    # what the guest said while it booted, for whoever reads the failure
    print(box.execute(f"tail -c 20000 {d}/instances/{i}/console.log")[1])
    raise

# theirs: wheel, sudo, /home on its own disk
assert "wheel" in box.succeed(f"{ssh} id")
box.succeed(f"{ssh} sudo -n true")
home = box.succeed(f"{ssh} findmnt -no SOURCE /home").strip()
root = box.succeed(f"{ssh} findmnt -no SOURCE /").strip()
assert home != root and home.startswith("/dev/vd"), (home, root)
# ann can't see it or stop it
_, text = api("ann", "GET", "/api/state")
assert i not in text
assert api("ann", "POST", f"/api/instances/{i}/stop")[0] == 422

# its Nix file applied, and the system it built is the one the host made:
# nothing to build, so nothing to fetch
box.wait_until_succeeds(f"test -s {d}/instances/{i}/report/apply.json", timeout=600)
applied = json.loads(box.succeed(f"cat {d}/instances/{i}/report/apply.json"))
assert applied["ok"], api("tom", "GET", f"/api/instances/{i}/log")[1]

# what its disks hold counts toward tom's allowance: into the box's facts
box.succeed("systemctl start dd-vms-usage.service")
used = box.succeed("cat /var/lib/dd-facts/usage-computers.facts")
assert used.startswith("usage/computers/tom ") and int(used.split()[1]) > 0, used

# the firewall's filler is told of it
box.wait_until_succeeds(f"grep -q '{port}' /run/dd-vms/access.json", timeout=30)

# out of the box: its loopback (here, a listener the box itself reaches) is
# refused, and so is anything private
box.succeed("systemd-run --unit probe python3 -m http.server --bind 127.0.0.1 4999")
box.wait_until_succeeds("curl -sf -m 3 -o /dev/null http://127.0.0.1:4999/")
box.fail(f"{ssh} curl -sf -m 5 -o /dev/null http://10.0.2.2:4999/")
box.fail(f"{ssh} curl -sf -m 5 -o /dev/null http://192.168.1.1/")

# /home outlives a stop and a start, and the stop is a clean one
box.succeed(f"{ssh} 'echo kept > ~/kept'")
assert api("tom", "POST", f"/api/instances/{i}/stop")[0] == 200
box.wait_until_succeeds(
    f"systemctl show -p ActiveState dd-vm@{i}.service | grep -q inactive", timeout=120
)
assert "Result=success" in box.succeed(f"systemctl show -p Result dd-vm@{i}.service")
assert api("tom", "POST", f"/api/instances/{i}/start")[0] == 200
box.wait_until_succeeds(f"{ssh} true", timeout=600)
assert box.succeed(f"{ssh} cat kept").strip() == "kept"

# a restart of the manager (a deploy) leaves it running
pid = box.succeed(f"systemctl show -p MainPID --value dd-vm@{i}.service").strip()
box.succeed("systemctl restart dd-vms.service")
box.wait_until_succeeds("test -S /run/dd-vms/dd-vms.sock", timeout=60)
assert (
    box.succeed(f"systemctl show -p MainPID --value dd-vm@{i}.service").strip() == pid
)

# deleted only once stopped, and then it's gone with its disks
assert api("tom", "DELETE", f"/api/instances/{i}")[0] == 422
assert api("tom", "POST", f"/api/instances/{i}/stop")[0] == 200
box.wait_until_succeeds(
    f"systemctl show -p ActiveState dd-vm@{i}.service | grep -q inactive", timeout=120
)
assert api("tom", "DELETE", f"/api/instances/{i}")[0] == 200
box.fail(f"test -e {d}/instances/{i}")
