# The leaving test: out at once by one's own word, everything gone after
# the window, and nothing of anyone else's with it.
import json

box.wait_for_unit("garage.service")
box.wait_for_open_port(3901)
box.succeed("systemctl restart garage-setup.service")
box.wait_until_succeeds(
    f"set -a; . {nix['rpc']}; garage layout show > /tmp/l && grep -q 'Current cluster layout version: [1-9]' /tmp/l"
)
box.succeed("systemctl restart garage-setup.service")
box.wait_for_open_port(4181)

gate = "http://127.0.0.1:4181"
dirs = f"--directory {gate}/_dd/directory"
dd = nix["dd"]


def as_(who):
    return f"DD_KEYRING_FILE=/root/{who}.json {dd}"


# three members, each with a library; sarah leaves, ann leaves and comes
# back, tom stays
box.succeed("mkdir -p /root/fleet/fleet")
for who in ["sarah", "ann", "tom"]:
    box.succeed(f"{as_(who)} identity new --name {who} {dirs}")
    box.succeed(f"{as_(who)} member add {who} --repo /root/fleet {dirs}")
box.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    '"$(jq -c . /root/fleet/fleet/members.json)" '
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)

libs = {}
for who in ["sarah", "ann", "tom"]:
    out = box.succeed(f"{as_(who)} library new {dirs}")
    libs[who] = out.split("library ")[1].split(":")[0].strip()
    assert len(libs[who]) == 32, out

# something in each library and in sarah's disk images, straight into the
# bucket: the gate deletes by prefix, whatever the bytes are
s3 = (
    "AWS_ACCESS_KEY_ID=GK0123456789abcdef01234567 "
    "AWS_SECRET_ACCESS_KEY=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef "
    "aws --endpoint-url http://127.0.0.1:3900 --region garage"
)
box.succeed("echo ciphertext > /root/blob")
for who in ["sarah", "ann", "tom"]:
    box.succeed(f"{s3} s3 cp /root/blob s3://libraries/{libs[who]}/Files/x")
box.succeed(f"{s3} s3 cp /root/blob s3://libraries/images-sarah/data/x")


def keys(prefix):
    out = box.succeed(
        f"{s3} s3api list-objects-v2 --bucket libraries --prefix {prefix}/ --query 'Contents[].Key' --output json"
    ).strip()
    return json.loads(out or "null") or []


assert keys(libs["sarah"]) and keys("images-sarah")

# what the gate keeps of them: notes on films and names for devices
state = "/var/lib/dd-verify"
for who in ["tom", "sarah"]:
    box.succeed(
        f"mkdir -p {state}/notes/{libs[who]}/progress {state}/names"
        f" && echo n > {state}/notes/{libs[who]}/shelf.bin"
        f" && echo p > {state}/notes/{libs['tom']}/progress/{who}.bin"
        f" && echo d > {state}/names/{who}.bin"
    )
box.succeed(f"chown -R dd-verify: {state}/notes {state}/names")


# signed in two ways: a device's token, and the session it opens
def signed_in(who):
    token = box.succeed(f"{as_(who)} token").strip()
    box.succeed(
        f"curl -s -D /root/{who}.h -o /dev/null -X POST --data-urlencode token={token} {gate}/_dd/app/signin"
    )
    cookie = box.succeed(
        f"grep -i '^set-cookie:' /root/{who}.h | head -1 | sed 's/^[^:]*: //; s/;.*//'"
    ).strip()
    assert cookie, box.succeed(f"cat /root/{who}.h")
    return token, cookie


def me(token=None, cookie=None):
    h = f"-H 'Authorization: Bearer {token}'" if token else f"-H 'Cookie: {cookie}'"
    return box.succeed(
        f"curl -s -o /dev/null -w '%{{http_code}}' {h} {gate}/_dd/me"
    ).strip()


sarah_token, sarah_cookie = signed_in("sarah")
ann_token, ann_cookie = signed_in("ann")
assert me(cookie=sarah_cookie) == "200" and me(token=sarah_token) == "200"

# nothing is due yet, so no service may say it deleted anything
done = f"curl -s -o /dev/null -w '%{{http_code}}' -X POST -H 'Content-Type: application/json' {gate}/internal/leaving/done"
assert (
    box.succeed(f'{done} -d \'{{"name":"sarah","service":"photos"}}\'').strip() == "409"
)

# sarah leaves: at once, nothing of hers opens
box.succeed(f"{as_('sarah')} leave {dirs}")
box.wait_until_succeeds(
    f"curl -sf {gate}/_dd/leaving/sarah | jq -e '.statement.leave.back | not'"
)
assert me(cookie=sarah_cookie) != "200", "her session still opens"
assert me(token=sarah_token) != "200", "her device still opens"
listed = json.loads(box.succeed(f"curl -sf {gate}/internal/leaving"))
sarah = [m for m in listed["members"] if m["name"] == "sarah"]
assert sarah and not sarah[0]["due"], listed
# kept through the window
assert keys(libs["sarah"]) and keys("images-sarah")
box.succeed(f"test -e {state}/names/sarah.bin")
box.wait_until_succeeds(
    "journalctl -u dd-verify | grep -q 'leaving: member .* is out'", timeout=90
)

# ann leaves and comes back inside the window: nothing of hers goes
box.succeed(f"{as_('ann')} leave {dirs}")
assert me(token=ann_token) != "200"
box.succeed("sleep 2")
box.succeed(f"{as_('ann')} leave --back {dirs}")
assert me(token=ann_token) == "200", "back, and let in again"

# a statement from someone else's root is refused
forged = box.succeed(
    f"curl -sf {gate}/_dd/leaving/sarah | jq -c '.statement | .leave.name = \"tom\" | .leave.at = (now | floor)'"
).strip()
assert (
    box.succeed(
        f"curl -s -o /dev/null -w '%{{http_code}}' -X PUT -H 'Content-Type: application/json' -d '{forged}' {gate}/_dd/leaving/tom"
    ).strip()
    != "204"
)

# the window (two minutes) runs out: her libraries, images, notes and names go
box.wait_until_succeeds(
    f'curl -sf {gate}/internal/leaving | jq -e \'.members[] | select(.name == "sarah") | .due and (.done | index("gate")) and (.done | index("libraries"))\'',
    timeout=300,
)
assert keys(libs["sarah"]) == [], keys(libs["sarah"])
assert keys("images-sarah") == []
box.succeed(f"test ! -e {state}/names/sarah.bin")
box.succeed(f"test ! -e {state}/notes/{libs['sarah']}")
box.succeed(f"test ! -e {state}/notes/{libs['tom']}/progress/sarah.bin")
# a service on the box reports its share done now that it is due
assert (
    box.succeed(f'{done} -d \'{{"name":"sarah","service":"photos"}}\'').strip() == "204"
)
box.succeed(
    f'curl -sf {gate}/internal/leaving | jq -e \'.members[] | select(.name == "sarah") | .done | index("photos")\''
)

# nobody else's: tom's library, notes and names, ann's library
assert keys(libs["tom"]) and keys(libs["ann"])
box.succeed(f"test -e {state}/names/tom.bin")
box.succeed(f"test -e {state}/notes/{libs['tom']}/shelf.bin")
box.succeed(f"test -e {state}/notes/{libs['tom']}/progress/tom.bin")
assert me(token=ann_token) == "200"
# the log names nobody: member ids, not names
logs = box.succeed("journalctl -u dd-verify | grep 'leaving:'")
assert "sarah" not in logs, logs
