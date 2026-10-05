# The network test: a box on its own network, a device handed the way in.
import json

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
box.succeed(f"{env} {nix['dd']} identity new --name sarah {dirs}")

# a key on this network is for members. The list starts empty, so sarah has
# to be in it before the door opens for her at all.
box.succeed("mkdir -p /root/fleet/fleet")
box.succeed(f"{env} {nix['dd']} member add sarah --repo /root/fleet {dirs}")
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
assert got["control_url"] == "http://127.0.0.1:8085", got
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
    f"DD_KEYRING_FILE=/root/tom.json {nix['dd']} identity new --name tom {dirs}"
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
