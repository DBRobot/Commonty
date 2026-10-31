# The network test: a box on its own network, a device handed the way in.
import json
import time

from waits import journal_of

box.wait_for_unit("headscale.service")
box.wait_for_unit("headscale-seed.service")
# the seed's work: the boxes' user and the gate's api key; no key to join
# is kept anywhere
box.succeed("headscale users list -o json | jq -e '.[] | select(.name == \"boxes\")'")
box.succeed("test -s /var/lib/dd-verify/headscale-api.key")
box.succeed("test ! -e /var/lib/headscale/box.key")
# readable by the verifier and nobody else (systemd reowns its state dir on start)
owner = box.succeed("stat -c '%G:%a' /var/lib/dd-verify/headscale-api.key").strip()
assert owner == "dd-verify:440", owner
# the box on its own network, as a box (the keeper retries until it is)
box.wait_until_succeeds(
    "tailscale --socket /run/commonty-net/tailscaled.sock status --json | jq -e '.BackendState == \"Running\"'",
    timeout=180,
)
ip = box.succeed("tailscale --socket /run/commonty-net/tailscaled.sock ip -4").strip()
assert ip.startswith("100."), ip
box.succeed(
    'headscale nodes list -o json | jq -e \'.[] | select(.name == "box") | .tags | index("tag:box")\''
)
# the shape dd-control reads joined boxes' addresses from (joinedAddrs):
# a tagged node is listed under tagged-devices, and who its key was minted
# for is its pre-auth key's user
node = [
    n
    for n in json.loads(box.succeed("headscale nodes list -o json"))
    if n["name"] == "box"
][0]
assert node["user"]["name"] == "tagged-devices", node["user"]
assert node["pre_auth_key"]["user"]["name"] == "boxes", node.get("pre_auth_key")
assert any(a.startswith("100.") for a in node["ip_addresses"]), node
# it joined with a key made for that one try: single use, and gone after
box.succeed(
    "headscale preauthkeys list -o json | jq -e 'all(.[]; (.reusable // false) == false)'"
)
box.succeed("test ! -e /run/commonty-net/join.key")

# the network's database, copied for the backup with sqlite's own backup
box.succeed("systemctl start headscale-snapshot.service")
box.succeed(
    "sqlite3 /var/lib/headscale/snapshot/db.sqlite 'pragma integrity_check' | grep -x ok"
)
box.succeed("test -s /var/lib/headscale/snapshot/noise_private.key")
assert (
    box.succeed("stat -c '%U:%a' /var/lib/headscale/snapshot").strip()
    == "headscale:700"
)
# names on the network: every host nginx serves here, at this address
box.wait_until_succeeds(
    "jq -e 'length > 0' /var/lib/headscale/extra-records.json", timeout=60
)
records = json.loads(box.succeed("cat /var/lib/headscale/extra-records.json"))
assert all(r["value"] == ip for r in records), records
# a member's device: made here, admitted by its own root, asks the gate
env = "DD_KEYRING_FILE=/root/keys.json"
box.wait_for_open_port(4181)
dirs = "--directory http://127.0.0.1:4181/_dd/directory"
box.succeed(
    f"{env} {nix['dd']} identity new --born 1990-01 --agree-terms --name sarah {dirs}"
)

# a key on this network is for members. The list starts empty, so sarah has
# to be in it before the door opens for her at all.
box.succeed("mkdir -p /root/fleet/fleet")
box.succeed(
    f"{env} {nix['dd']} member add sarah --repo /root/fleet {dirs} --confirm $({env} {nix['dd']} identity show {dirs} | sed -n 's/^member id: *//p')"
)
box.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    '"$(jq -c . /root/fleet/fleet/members.json)" '
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)

token = box.succeed(f"{env} {nix['dd']} token").strip()
got = json.loads(
    box.succeed(
        f"curl -sf -X POST -H 'Authorization: Bearer {token}' http://127.0.0.1:4181/_dd/network/join"
    )
)
assert got["control_url"] == "http://box:8085", got
assert got["user"] == "sarah", got
assert len(got["key"]) > 20, got
# in headscale: sarah exists, with a key that is hers alone and once (a
# false in this json is a field left out)
box.succeed("headscale users list -o json | jq -e '.[] | select(.name == \"sarah\")'")
keys = json.loads(box.succeed("headscale preauthkeys list -o json"))
hers = [k for k in keys if k.get("user", {}).get("name") == "sarah"]
assert len(hers) == 1 and not hers[0].get("reusable"), keys
# no token, no key; a stranger's token, no key
box.fail("curl -sf -X POST http://127.0.0.1:4181/_dd/network/join")
box.fail(
    "curl -sf -X POST -H 'Authorization: Bearer nope' http://127.0.0.1:4181/_dd/network/join"
)

# and a stranger who signed up is not a member: a real key, a real entry, a
# real token, and no way onto the network the boxes trust
box.succeed(
    f"DD_KEYRING_FILE=/root/tom.json {nix['dd']} identity new --born 1990-01 --agree-terms --name tom {dirs}"
)
tom = box.succeed(f"DD_KEYRING_FILE=/root/tom.json {nix['dd']} token").strip()
code = box.succeed(
    f"curl -s -o /dev/null -w '%{{http_code}}' -X POST -H 'Authorization: Bearer {tom}' "
    "http://127.0.0.1:4181/_dd/network/join"
).strip()
assert code == "403", code
# headscale never heard of him
box.fail("headscale users list -o json | jq -e '.[] | select(.name == \"tom\")'")

# Another box asks for its own key: the time signed with the key in its TPM
# (a soft key here), the answer sealed to the secrets key in the same TPM
# (a plain age key here). The gate learns both from the fleet's data.
box.succeed(
    "openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 | openssl pkcs8 -topk8 -nocrypt -outform DER -out /root/node2.der"
)
box.succeed(
    "openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 | openssl pkcs8 -topk8 -nocrypt -outform DER -out /root/other.der"
)
box.succeed("age-keygen -o /root/node2.age 2>/dev/null")
box.succeed(
    'jq -n --arg k "$(openssl pkey -inform DER -in /root/node2.der -pubout -outform DER | base64 -w0)" '
    "--arg r \"$(age-keygen -y /root/node2.age)\" '{node2: {key: $k, recipient: $r}}' > /run/boxes.json"
)
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_NETWORK_BOXES=/run/boxes.json'\\n\" "
    "> /run/systemd/system/dd-verify.service.d/boxes.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)
door = "http://127.0.0.1:4181/_dd/network/box"
ask = lambda who, key: (
    f"{nix['control']} -box {who} -sign-join -box-key /root/{key}.der"
)
post = f"curl -s -o /root/out -w '%{{http_code}}' -H 'content-type: application/json' --data-binary @- {door}"

# signed by some other key: refused, and nothing minted
before = box.succeed("headscale preauthkeys list -o json | jq length").strip()
assert box.succeed(f"{ask('node2', 'other')} | {post}").strip() == "403"
# a box the fleet does not have
assert box.succeed(f"{ask('node9', 'node2')} | {post}").strip() == "403"
assert box.succeed("headscale preauthkeys list -o json | jq length").strip() == before

# node2 itself: a key only its secrets key opens, tagged a box, once
box.succeed(f"{ask('node2', 'node2')} > /root/ask.json")
assert box.succeed(f"{post} < /root/ask.json").strip() == "200"
box.succeed("grep -q 'BEGIN AGE ENCRYPTED FILE' /root/out")
key = box.succeed("age -d -i /root/node2.age /root/out").strip()
keys = json.loads(box.succeed("headscale preauthkeys list -o json"))
tagged = [k for k in keys if "tag:box" in (k.get("aclTags") or k.get("acl_tags") or [])]
assert tagged and all(not k.get("reusable") for k in tagged), keys
# the same request again, copied off the wire: nothing more
assert box.succeed(f"{post} < /root/ask.json").strip() == "429"

# The admin's devices administer the box; the host's do not. sarah is the
# fleet's admin (dd.verify.admin); kim has the box at home (dd.box.owner) and
# has a device of their own.
box.succeed("headscale users create kim")
kim_id = box.succeed(
    "headscale users list -o json | jq -r '.[] | select(.name == \"kim\") | .id'"
).strip()
kim_key = box.succeed(
    f"headscale preauthkeys create --user {kim_id} --expiration 10m"
).strip()
for vm, k in ((laptop, got["key"]), (other, kim_key)):
    vm.wait_for_unit("tailscaled.service")
    # once the key is taken a retry needs none: the node is registered
    vm.execute(
        f"tailscale up --login-server http://box:8085 --auth-key {k} --accept-dns=false --timeout=60s"
    )
    vm.wait_until_succeeds(
        "tailscale status --json | jq -e '.BackendState == \"Running\"'", timeout=180
    )
box_ip = ip
laptop_ip = laptop.succeed("tailscale ip -4").strip()
# the box's own view of whose each device is, into the set the firewall reads
box.wait_until_succeeds(
    "tailscale --socket /run/commonty-net/tailscaled.sock status --json | jq -e '.Peer | length >= 2'",
    timeout=60,
)
box.succeed("systemctl start dd-owner-devices.service")
other_ip = other.succeed("tailscale ip -4").strip()
admins = box.succeed("ipset list dd-owner")
assert laptop_ip in admins and other_ip not in admins, admins
hosts = box.succeed("ipset list dd-host")
assert other_ip in hosts and laptop_ip not in hosts, hosts
# the admin's laptop reaches ssh on the box; the host's does not
laptop.wait_until_succeeds(f"nc -z -w 5 {box_ip} 22", timeout=60)
other.fail(f"nc -z -w 5 {box_ip} 22")
# and the box's admin login takes no key of the host's
box.fail("grep -q kim /etc/ssh/authorized_keys.d/admin 2>/dev/null")

# Who is who, not known yet, is nobody's loss (2026-10-22: a gate that
# started before dd-control answered read everyone as no member and took the
# fleet owner's laptop off the network for two hours). The gate goes by the
# control plane's agreed state, with dd-control down at its start.
# The replay gate before it would take jo's devices off on its own next
# pass (jo is on no member list): it is stopped while they are made.
box.succeed("systemctl stop dd-verify.service")


def node_names():
    return set(
        box.succeed("headscale nodes list -o json | jq -r '.[].given_name'").split()
    )


# three more devices, made in headscale itself: jo's (jo's membership is
# revoked), ivy's (a member) and zed's, whom the agreed state does not have.
# kim, the box's host, is revoked last, at the end
for user, node, reg in (
    ("jo", "phone", "c" * 24),
    ("ivy", "tablet", "a" * 24),
    ("zed", "pc", "b" * 24),
):
    box.execute(f"headscale users create {user}")
    box.succeed(f"headscale debug create-node --name {node} --user {user} --key {reg}")
    box.succeed(f"headscale nodes register --user {user} --key {reg}")
everyone = {"laptop", "phone", "tablet", "pc"}
assert everyone <= node_names(), node_names()
box.succeed("mkdir -p /run/dd-test")
box.succeed(
    'echo \'{"boxes": {"box": {}}, "validators": [{"name": "box"}]}\' > /run/dd-test/control.json'
)
# sarah (the fleet's admin), kim (the box's host) and ivy are members, jo's
# membership was revoked
box.succeed(
    "jq -n '{height: 5, members: {"
    'sarah: {status: "member", access: "invite"}, '
    'kim: {status: "member", access: "invite"}, '
    'ivy: {status: "member", access: "invite"}, '
    'jo: {status: "revoked", access: "none"}}}\' > /run/dd-test/identity.json'
)
box.succeed(
    "printf '[Service]\\n"
    "Environment=VERIFY_CONTROL=http://127.0.0.1:4195\\n"
    "Environment=VERIFY_CONTROL_SUBMIT=/run/dd-test/submit.sock\\n"
    "Environment=VERIFY_CONTROL_CONFIG=/run/dd-test/control.json\\n"
    "Environment=VERIFY_IDENTITY_FROM=agreed\\n"
    "Environment=VERIFY_BOX=box\\n"
    "Environment=VERIFY_REAP_SECS=2\\n"
    "Environment=VERIFY_REAP_GRACE_SECS=8\\n"
    "Environment=VERIFY_LEAVE_TICK_SECS=2\\n' "
    "> /run/systemd/system/dd-verify.service.d/agreed.conf"
)
box.succeed("systemctl daemon-reload && systemctl start dd-verify.service")
box.wait_for_open_port(4181)
# this run of the gate's lines alone
run = box.succeed("systemctl show -p InvocationID --value dd-verify").strip()


def seen(text):
    return journal_of(box, "dd-verify", run).count(text)


def wait_seen(text, times=1, timeout=60):
    end = time.monotonic() + timeout
    while seen(text) < times:
        assert time.monotonic() < end, f"not {times} x in {timeout}s: {text}"
        box.sleep(1)


# dd-control not answering: the reaper has passed several times, keeping
# everyone and saying why
wait_seen("kept laptop - whether sarah is a member is not known", times=3, timeout=60)
assert everyone <= node_names(), node_names()
assert seen("kept phone - whether jo is a member is not known"), (
    "kept phone - whether jo is a member is not known"
)
assert not seen("network: removed"), "network: removed"
# nothing ended or deleted, and the services that delete hear nothing to do
wait_seen("leaving: nothing ended or deleted", timeout=30)


def code(path):
    return box.succeed(
        f"curl -s -o /dev/null -w '%{{http_code}}' http://127.0.0.1:4181{path}"
    ).strip()


assert code("/internal/leaving") == "503", code("/internal/leaving")
assert code("/internal/storage/members") == "503"

# dd-control answers: who is who is known, then the grace runs, then jo's
# devices come off, and zed's (known now: the agreed state has no zed); the
# admin's, the host's and ivy's stay
box.succeed(
    f"systemd-run --unit dd-control-standin $(command -v python3) {nix['standin']} /run/dd-test/identity.json"
)
wait_seen("NOT reaping: who is a member has been known for", timeout=60)
wait_seen("network: removed phone - jo is not a member", timeout=60)
wait_seen("network: removed pc - zed is not a member", timeout=30)
left = node_names()
assert {"laptop", "other", "tablet"} <= left and not {"phone", "pc"} & left, left
assert code("/internal/leaving") == "200", code("/internal/leaving")
box.succeed("grep -qx 'dd_reap_brake 0' /var/lib/dd-facts/reap-brake.prom")

# The agreed state suddenly drops ivy (a third of who has access): more likely
# wrong than true. Nothing comes off, pass after pass, and the brake is up
# where the fleet's alerts read it; restored, it is off, and ivy kept hers.
# The drop lands while a pass is waiting on headscale (CI run 882: that pass
# had read who has access before, judged ivy by the state after, and took
# her tablet): headscale is paused until the gate is in a call to it, the
# state changes, the gate reads it, and headscale goes on.
gate_pid = box.succeed("systemctl show -p MainPID --value dd-verify").strip()


def gate_waiting_on_headscale():
    mine = box.succeed("ss -Htnp state established '( dport = :8085 )'")
    ports = {
        line.split()[2].rsplit(":", 1)[1]
        for line in mine.splitlines()
        if f"pid={gate_pid}," in line
    }
    theirs = box.succeed("ss -Htn state established '( sport = :8085 )'")
    return any(
        int(line.split()[0]) > 0 and line.split()[3].rsplit(":", 1)[1] in ports
        for line in theirs.splitlines()
    )


def standin_asked(text):
    return box.succeed(
        f"journalctl -u dd-control-standin -o cat | grep -c '{text}' || true"
    ).strip()


# A call headscale read just before it stopped shows nothing waiting; it
# runs out unseen and the next pass's call is the one caught. The clock
# starts at the last look that did not see the caught call: it began after
# that, so it is still waiting while under its 10 s from there.
began = time.monotonic()
box.succeed("systemctl kill -s SIGSTOP headscale.service")
end = began + 30
while True:
    look = time.monotonic()
    if gate_waiting_on_headscale():
        break
    assert look < end, "the gate never called headscale"
    began = look
    time.sleep(0.2)  # sleep-ok: fast polls, under the gate's 10 s call
box.succeed(
    "jq 'del(.members.ivy) | .height = 6' /run/dd-test/identity.json > /run/dd-test/i && mv /run/dd-test/i /run/dd-test/identity.json"
)
# the gate asks for what is newer than 6 once it holds 6
end = time.monotonic() + 30
while standin_asked("GET /identity?since=6 ") == "0":
    assert time.monotonic() < end, "the gate never read height 6"
    time.sleep(0.2)  # sleep-ok: fast polls, under the gate's 10 s call
box.succeed("systemctl kill -s SIGCONT headscale.service")
# under the gate's 10 s on a headscale call, or the pass never judged
held = time.monotonic() - began
print(f"the gate's call held {held:.1f} s")
assert held < 9, f"the gate's call held {held:.1f} s: it ran out"
wait_seen("NOT reaping this pass: the state changed while it was judged", timeout=30)
wait_seen("NOT reaping, the brake is on: 1 of the 3", times=3, timeout=60)
assert not seen("removed tablet"), "removed tablet"
assert "tablet" in node_names(), node_names()
box.succeed("grep -qx 'dd_reap_brake 1' /var/lib/dd-facts/reap-brake.prom")
box.succeed(
    'jq \'.members.ivy = {status: "member", access: "invite"} | .height = 7\' /run/dd-test/identity.json > /run/dd-test/i && mv /run/dd-test/i /run/dd-test/identity.json'
)
wait_seen("the state is consistent again; the brake is off", timeout=60)
box.succeed("grep -qx 'dd_reap_brake 0' /var/lib/dd-facts/reap-brake.prom")
assert {"laptop", "tablet"} <= node_names(), node_names()

# The box's host, revoked, loses their devices like anyone: the reaper keeps
# the admin and the people the release names, not a host (network::reap_keep).
# kim's other joined over real tailscale and nothing before took it off.
# Four more members first, so kim going is one of seven people with access,
# under the brake's quarter (a state that drops the host with others at once
# is still held: network.rs a_revoked_host_loses_their_devices_and_the_brake_still_holds)
assert "other" in node_names(), node_names()
box.succeed(
    "jq '.members += {"
    'ann: {status: "member", access: "invite"}, '
    'bob: {status: "member", access: "invite"}, '
    'cy: {status: "member", access: "invite"}, '
    'dee: {status: "member", access: "invite"}'
    "} | .height = 8' /run/dd-test/identity.json > /run/dd-test/i && mv /run/dd-test/i /run/dd-test/identity.json"
)
# a pass has acted on the state with them in it
box.wait_until_succeeds(
    "jq -e '.acted | any(. == \"dee\")' /var/lib/dd-verify/reap-acted.json",
    timeout=60,
)
box.succeed(
    'jq \'.members.kim = {status: "revoked", access: "none"} | .height = 9\' /run/dd-test/identity.json > /run/dd-test/i && mv /run/dd-test/i /run/dd-test/identity.json'
)
wait_seen("network: removed other - kim is not a member", timeout=60)
left = node_names()
assert "other" not in left and {"laptop", "tablet"} <= left, left
assert not seen("NOT removing other"), "NOT removing other"
box.succeed("grep -qx 'dd_reap_brake 0' /var/lib/dd-facts/reap-brake.prom")
# A remote computer on the boxes' admin ports: only with the tag the gate
# gives a computer the release lists (network.rs computer_tags). Tagged by
# hand here, so the gate's reaper, which knows it did not make that key,
# stands aside. Its address reaches the firewall by the tag alone
box.succeed("systemctl stop dd-verify.service")
boxes_id = box.succeed(
    "headscale users list -o json | jq -r '.[] | select(.name == \"boxes\") | .id'"
).strip()


def join_as_computer(tags):
    k = (
        box.succeed(
            f"headscale preauthkeys create --user {boxes_id} --expiration 10m --tags {tags}"
        )
        .strip()
        .splitlines()[-1]
    )
    # a new node, as a computer's is at each start (its key in memory only)
    other.succeed(
        "systemctl stop tailscaled && rm -f /var/lib/tailscale/tailscaled.state"
        " && systemctl start tailscaled"
    )
    other.wait_until_succeeds("tailscale status --json > /dev/null", timeout=30)
    other.succeed(
        f"tailscale up --login-server http://box:8085 --auth-key {k} --accept-dns=false --timeout=60s"
    )
    ip = other.succeed("tailscale ip -4").strip()
    box.wait_until_succeeds(
        "tailscale --socket /run/commonty-net/tailscaled.sock status --json"
        f" > /tmp/st && grep -c '{ip}' /tmp/st",
        timeout=60,
    )
    box.succeed("systemctl start dd-owner-devices.service")
    return ip


ip = join_as_computer("tag:computer")
assert ip not in box.succeed("ipset list dd-owner")
other.fail(f"nc -z -w 5 {box_ip} 22")
ip = join_as_computer("tag:computer,tag:admin-computer")
assert ip in box.succeed("ipset list dd-owner")
other.wait_until_succeeds(f"nc -z -w 5 {box_ip} 22", timeout=60)
box.succeed("systemctl stop dd-control-standin.service")
