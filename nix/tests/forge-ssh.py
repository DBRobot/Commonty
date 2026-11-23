start_all()
box.wait_for_unit("dd-forge-sshd.service")
box.wait_for_unit("sshd.service")
P = nix["port"]
box.wait_for_open_port(P, "127.0.0.1")

# loopback only: the tunnel is the one way in
listen = box.succeed(f'ss -Hltn "sport = :{P}"')
assert "127.0.0.1:" + str(P) in listen, listen
assert "0.0.0.0" not in listen and "[::]" not in listen and "*:" not in listen, listen

# the forge's key line, as Forgejo writes them: forced command, no pty, no forwarding
box.succeed("install -d -m 700 /root/k && ssh-keygen -q -t ed25519 -N '' -f /root/k/id")
pub = box.succeed("cat /root/k/id.pub").strip()
line = f'command="echo FORCED $SSH_ORIGINAL_COMMAND",no-port-forwarding,no-X11-forwarding,no-agent-forwarding,no-pty {pub}'
for user, home in [("forgejo", "/var/lib/forgejo"), ("alice", "/home/alice")]:
    box.succeed(f"install -d -m 700 -o {user} {home}/.ssh")
    box.succeed(f"echo '{line}' > {home}/.ssh/authorized_keys")
    box.succeed(f"chown {user} {home}/.ssh/authorized_keys && chmod 600 {home}/.ssh/authorized_keys")

ssh = (
    "ssh -i /root/k/id -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=no "
    f"-o UserKnownHostsFile=/dev/null -o LogLevel=ERROR -p {P}"
)
dest = "forgejo@127.0.0.1"

# an exec request runs the forced command, never what was asked
out = box.succeed(f"{ssh} {dest} id </dev/null")
assert "FORCED id" in out and "uid=" not in out, out
# a shell request: the same forced command, no shell
out = box.succeed(f"{ssh} {dest} </dev/null")
assert "FORCED" in out and "uid=" not in out, out

# a pty is refused
_, out = box.execute(f"{ssh} -tt {dest} true </dev/null 2>&1")
assert "PTY allocation request failed" in out, out

# no forwarding of any kind
status, out = box.execute(f"{ssh} -W 127.0.0.1:22 {dest} </dev/null 2>&1")
assert status != 0, out
status, out = box.execute(
    f"timeout 20 {ssh} -N -R 9000:127.0.0.1:22 -o ExitOnForwardFailure=yes {dest} 2>&1"
)
assert status not in (0, 124), out

# nobody but forgejo, though the key is in alice's authorized_keys too
for user in ["alice", "root"]:
    status, out = box.execute(f"{ssh} {user}@127.0.0.1 true </dev/null 2>&1")
    assert status != 0, (user, out)

# key only: the server offers no password and no keyboard-interactive
_, out = box.execute(f"{ssh} -o PubkeyAuthentication=no {dest} true </dev/null 2>&1")
assert "Permission denied (publickey)" in out, out


# its own host key, kept in its own place, not the admin sshd's, and the same after a restart
def fingerprint(port):
    return box.succeed(
        f"ssh-keyscan -t ed25519 -p {port} 127.0.0.1 2>/dev/null | ssh-keygen -lf - | cut -d' ' -f2"
    ).strip()


door = fingerprint(P)
assert door and door != fingerprint(22), door
box.succeed("test -s /var/lib/dd-forge-sshd/ssh_host_ed25519_key")
box.succeed("systemctl restart dd-forge-sshd.service")
box.wait_for_open_port(P, "127.0.0.1")
assert fingerprint(P) == door
