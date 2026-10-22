# The leaving test: out and deleted at once by one's own word; suspended by
# the owner, read-only for the window, lifted with nothing lost, and deleted
# once the window runs out; nothing of anyone else's.
import json

box.wait_for_unit("dd-store-setup.service")
box.wait_for_unit("dd-library-keys.service")
box.wait_for_open_port(4181)

gate = "http://127.0.0.1:4181"
dirs = f"--directory {gate}/_dd/directory"
dd = nix["dd"]


def as_(who):
    return f"DD_KEYRING_FILE=/root/{who}.json {dd}"


# three members, each with a library; sarah leaves, the owner suspends ann,
# tom stays
box.succeed("mkdir -p /root/fleet/fleet")
for who in ["sarah", "ann", "tom"]:
    box.succeed(
        f"{as_(who)} identity new --born 1990-01 --agree-terms --name {who} {dirs}"
    )
    box.succeed(
        f"{as_(who)} member add {who} --repo /root/fleet {dirs} --confirm $({as_(who)} identity show {dirs} | sed -n 's/^member id: *//p')"
    )
box.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")


# the release's member list, as the gate is started with it
def release_members():
    box.succeed(
        "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
        '"$(jq -c . /root/fleet/fleet/members.json)" '
        "> /run/systemd/system/dd-verify.service.d/members.conf"
    )
    box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
    box.wait_for_open_port(4181)


release_members()

libs = {}
for who in ["sarah", "ann", "tom"]:
    out = box.succeed(f"{as_(who)} library new {dirs}")
    libs[who] = out.split("library ")[1].split(":")[0].strip()
    assert len(libs[who]) == 32, out

# something in each library and in sarah's disk images, straight into the
# bucket: the gate deletes by prefix, whatever the bytes are
s3 = (
    "set -a; . /var/lib/dd-store/keys/libraries.env; set +a; "
    "aws --endpoint-url http://127.0.0.1:3910 --region us-east-1"
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
tom_token, _ = signed_in("tom")
assert me(cookie=sarah_cookie) == "200" and me(token=sarah_token) == "200"

# nothing is due yet, so no service may say it deleted anything
done = f"curl -s -o /dev/null -w '%{{http_code}}' -X POST -H 'Content-Type: application/json' {gate}/internal/leaving/done"
assert (
    box.succeed(f'{done} -d \'{{"name":"sarah","service":"photos"}}\'').strip() == "409"
)

# leaving asks first, since it cannot be undone
box.fail(f"{as_('sarah')} leave {dirs}")
assert me(token=sarah_token) == "200"

# sarah leaves: at once nothing of hers opens, and it is all due to go
box.succeed(f"{as_('sarah')} leave --yes {dirs}")
box.wait_until_succeeds(
    f"curl -sf {gate}/_dd/leaving/sarah | jq -e '.statement.leave.back | not'"
)
assert me(cookie=sarah_cookie) != "200", "her session still opens"
assert me(token=sarah_token) != "200", "her device still opens"
listed = json.loads(box.succeed(f"curl -sf {gate}/internal/leaving"))
sarah = [m for m in listed["members"] if m["name"] == "sarah"]
assert sarah and sarah[0]["due"], listed
# and there is no coming back, nor a second leaving to put it off
box.fail(f"{as_('sarah')} leave --yes {dirs}")

# the owner suspends ann: every sign-in ends, and for the window she may only
# read and take her own things
ann_id = f"$({as_('ann')} identity show {dirs} | sed -n 's/^member id: *//p')"
box.fail(
    f"{as_('tom')} member suspend ann --reason '' --repo /root/fleet {dirs} --confirm {ann_id}"
)
box.succeed(
    f"{as_('tom')} member suspend ann --reason 'sent spam' --repo /root/fleet {dirs} --confirm {ann_id}"
)
assert "sent spam" in box.succeed("cat /root/fleet/fleet/members.json")
release_members()
box.wait_until_succeeds(
    f"[ \"$(curl -s -o /dev/null -w '%{{http_code}}' -H 'Cookie: {ann_cookie}' {gate}/_dd/me)\" != 200 ]",
    timeout=90,
)
seen = json.loads(
    box.succeed(f"curl -sf -H 'Authorization: Bearer {ann_token}' {gate}/_dd/me")
)
assert seen["role"] == "suspended" and seen["suspended"]["reason"] == "sent spam", seen
assert not seen["services"] or all("files." in x["url"] for x in seen["services"]), seen


def dav(token, method, path):
    return box.succeed(
        f"curl -s -o /dev/null -w '%{{http_code}}' -X {method} -H 'Authorization: Bearer {token}' --data-binary @/root/blob {gate}/_dd/dav/{path}"
        if method == "PUT"
        else f"curl -s -o /dev/null -w '%{{http_code}}' -X {method} -H 'Authorization: Bearer {token}' {gate}/_dd/dav/{path}"
    ).strip()


assert dav(ann_token, "GET", f"{libs['ann']}/Files/x") == "200", (
    "she cannot take her files"
)
assert dav(ann_token, "PUT", f"{libs['ann']}/Files/y") != "201", (
    "a suspended member wrote"
)
assert dav(ann_token, "GET", f"{libs['tom']}/Files/x") == "403"
listed = json.loads(box.succeed(f"curl -sf {gate}/internal/leaving"))
ann = [m for m in listed["members"] if m["name"] == "ann"]
assert ann and not ann[0]["due"], listed

# lifted within the window: back, nothing lost
box.succeed(
    f"{as_('tom')} member unsuspend ann --repo /root/fleet {dirs} --confirm {ann_id}"
)
release_members()
assert (
    json.loads(
        box.succeed(f"curl -sf -H 'Authorization: Bearer {ann_token}' {gate}/_dd/me")
    )["role"]
    == "member"
)
assert keys(libs["ann"]), "a lifted suspension lost her things"

# suspended again, and this time the window runs out
box.succeed(
    f"{as_('tom')} member suspend ann --reason 'sent spam again' --repo /root/fleet {dirs} --confirm {ann_id}"
)
release_members()
removed = json.loads(box.succeed("jq -c .removed /root/fleet/fleet/members.json"))
assert len(removed) == 1, removed
ann_member, ann_removed = next(iter(removed.items()))


# the gate takes her as newly out once per removal time, and logs it in
# the same look that would delete what is due
def ann_looked_at(times):
    box.wait_until_succeeds(
        f"[ $(journalctl -u dd-verify -o cat | grep -c 'leaving: member {ann_member[:8]} is out') -ge {times} ]",
        timeout=120,
    )


def ann_kept(why):
    assert keys(libs["ann"]), why
    listed = json.loads(box.succeed(f"curl -sf {gate}/internal/leaving"))
    ann = [m for m in listed["members"] if m["name"] == "ann"]
    assert ann and not ann[0]["due"] and "libraries" not in ann[0]["done"], ann


ann_looked_at(2)

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

# at the gate's next look her libraries, images, notes and names go
box.wait_until_succeeds(
    f'curl -sf {gate}/internal/leaving | jq -e \'.members[] | select(.name == "sarah") | .due and (.done | index("gate")) and (.done | index("libraries"))\'',
    timeout=150,
)
# and ann's, suspended, are kept: the gate has looked at her since (above),
# and her window is the real one, which no slowness of the box runs out
ann_kept("a suspension keeps things for the window")
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

# ann's library is kept until her window runs out, then goes. The time is
# not waited out: the release says she was removed longer ago
window = json.loads(box.succeed(f"curl -sf {gate}/internal/leaving"))["window"]


def ann_removed_ago(secs):
    box.succeed(
        f"jq '.removed[\"{ann_member}\"] = {ann_removed - secs}' /root/fleet/fleet/members.json > /root/m.json"
        " && mv /root/m.json /root/fleet/fleet/members.json"
    )
    release_members()


# six hours of it left: a new removal time is a new leaving to the gate,
# so it looks at her again, and keeps everything
ann_removed_ago(window - 6 * 3600)
ann_looked_at(3)
ann_kept("deleted before the window ran out")
# and the whole window gone
ann_removed_ago(window)
box.wait_until_succeeds(
    f'curl -sf {gate}/internal/leaving | jq -e \'.members[] | select(.name == "ann") | .due and (.done | index("libraries"))\'',
    timeout=300,
)
assert keys(libs["ann"]) == []

# nobody else's: tom's library, notes and names
assert keys(libs["tom"])
box.succeed(f"test -e {state}/names/tom.bin")
box.succeed(f"test -e {state}/notes/{libs['tom']}/shelf.bin")
box.succeed(f"test -e {state}/notes/{libs['tom']}/progress/tom.bin")
assert me(token=tom_token) == "200"
# the log names nobody: member ids, not names
logs = box.succeed("journalctl -u dd-verify | grep 'leaving:'")
assert "sarah" not in logs, logs
