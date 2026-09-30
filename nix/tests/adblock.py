# The adblock test: the house is filtered, our names answer per network,
# and the household switches it from the menu.
import json

start_all()

# Pi-hole's api, as only the gate may use it: with the password, signed
# in once (Pi-hole turns away a sign-in every few seconds)
box.wait_for_open_port(8053, addr="127.0.0.1")
sid = box.wait_until_succeeds(
    "curl -sf -X POST http://127.0.0.1:8053/api/auth -d '{\"password\": \"test-pihole-password\"}' | jq -er .session.sid",
    timeout=120,
).strip()
ftl = f"curl -sf -H 'X-FTL-SID: {sid}'"
box.wait_for_unit("pihole-ftl.service")
# the list is in once Pi-hole says it is blocking something
box.wait_until_succeeds(f"{ftl} http://127.0.0.1:8053/api/stats/summary | jq -e '.gravity.domains_being_blocked > 0'", timeout=600)
# without it, nothing
box.succeed("curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:8053/api/stats/summary | grep -qx 401")
# and what it asks the internet goes through Unbound, over TLS
box.wait_for_unit("unbound.service")
box.wait_for_open_port(5335, addr="127.0.0.1")
box.succeed("grep -q '127.0.0.1#5335' /etc/pihole/pihole.toml")
conf = "/etc/unbound/unbound.conf"
box.succeed(f"grep -Eq 'forward-tls-upstream: *yes' {conf}")
box.succeed(f"grep -q '9.9.9.9@853#dns.quad9.net' {conf}")
house.wait_for_unit("multi-user.target")
away.wait_for_unit("multi-user.target")

# the weekly update runs, and Pi-hole still has its list after it
box.succeed("systemctl start dd-pihole-lists.service")
# and again, after Pi-hole restarts, as a release does: the lists are
# already there, and loading them again must not fail
box.succeed("systemctl restart pihole-ftl.service")
box.succeed("systemctl restart dd-pihole-lists.service")
box.succeed("systemctl restart dd-pihole-lists.service")
assert box.succeed("sqlite3 /var/lib/pihole/gravity.db 'select count(*) from adlist'").strip() == "1"
box.wait_until_succeeds(f"{ftl} http://127.0.0.1:8053/api/stats/summary | jq -e '.gravity.domains_being_blocked > 0'", timeout=120)

# the house is filtered
house.wait_until_succeeds("dig +short @192.168.1.2 ads.example.test | grep -qx 0.0.0.0", timeout=60)
# our names are answered as the world answers them: never with the box's
# house address, where it takes no web traffic (a page that never loads)
house.fail("dig +short +time=3 +tries=1 @192.168.1.2 home.test.invalid | grep -qx 192.168.1.2")

# the household: sarah lives here, with a session as the app's browser has
env = "DD_KEYRING_FILE=/root/keys.json"
dirs = "--directory http://127.0.0.1:4181/_dd/directory"
box.wait_for_open_port(4181)
for who in ("sarah", "tom"):
    box.succeed(f"DD_KEYRING_FILE=/root/{who}.json {nix['dd']} identity new --name {who} {dirs}")
box.succeed("mkdir -p /root/fleet/fleet")
for who in ("sarah", "tom"):
    box.succeed(f"DD_KEYRING_FILE=/root/{who}.json {nix['dd']} member add {who} --repo /root/fleet {dirs}")
box.succeed("mkdir -p /run/systemd/system/dd-verify.service.d")
box.succeed(
    "printf \"[Service]\\nEnvironment='VERIFY_MEMBERS=%s'\\n\" "
    "\"$(jq -c . /root/fleet/fleet/members.json)\" "
    "> /run/systemd/system/dd-verify.service.d/members.conf"
)
box.succeed("systemctl daemon-reload && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)

def session(who):
    token = box.succeed(f"DD_KEYRING_FILE=/root/{who}.json {nix['dd']} token").strip()
    head = box.succeed(f"curl -s -D - -o /dev/null -X POST --data-urlencode token={token} http://127.0.0.1:4181/_dd/app/signin")
    return [l for l in head.splitlines() if l.lower().startswith("set-cookie:")][0].split(":", 1)[1].split(";")[0].strip()

sarah = f"-H 'Cookie: {session('sarah')}'"
tom = f"-H 'Cookie: {session('tom')}'"
json_ = "-H 'content-type: application/json'"

# her menu has the switch; tom does not live here and has none
me = json.loads(box.succeed(f"curl -sf {sarah} http://127.0.0.1:4181/_dd/me"))
assert any(i.get("toggle") for g in me["menu"] for i in g), me["menu"]
me = json.loads(box.succeed(f"curl -sf {tom} http://127.0.0.1:4181/_dd/me"))
assert not any(i.get("toggle") for g in me["menu"] for i in g), me["menu"]
box.succeed(f"curl -s -o /dev/null -w '%{{http_code}}' {tom} http://127.0.0.1:4181/_dd/adblock/state | grep -qx 403")

state = json.loads(box.succeed(f"curl -sf {sarah} http://127.0.0.1:4181/_dd/adblock/state"))
assert state["on"] is True and state["lan"] == "192.168.1.2", state
assert state["sites"] >= 2, state

# off: the house gets real answers again (here, none: no internet)
box.succeed(f"curl -sf {sarah} {json_} -d '{{\"on\": false}}' http://127.0.0.1:4181/_dd/adblock/switch")
house.wait_until_fails("dig +short +time=2 +tries=1 @192.168.1.2 ads.example.test | grep -qx 0.0.0.0", timeout=30)
assert json.loads(box.succeed(f"curl -sf {sarah} 'http://127.0.0.1:4181/_dd/adblock/state?brief'"))["on"] is False
# a release restarts both, and Pi-hole comes up blocking: the gate puts
# the household's choice back
box.succeed("systemctl restart pihole-ftl.service && systemctl restart dd-verify.service")
box.wait_for_open_port(4181)
box.wait_until_succeeds(f"curl -sf {sarah} 'http://127.0.0.1:4181/_dd/adblock/state?brief' | jq -e '.on == false'", timeout=90)
# on again
box.succeed(f"curl -sf {sarah} {json_} -d '{{\"on\": true}}' http://127.0.0.1:4181/_dd/adblock/switch")
house.wait_until_succeeds("dig +short @192.168.1.2 ads.example.test | grep -qx 0.0.0.0", timeout=30)
# a pause says when it ends
paused = json.loads(box.succeed(f"curl -sf {sarah} {json_} -d '{{\"pause_minutes\": 5}}' http://127.0.0.1:4181/_dd/adblock/switch"))
assert paused["on"] is False and paused["resumesIn"], paused
box.succeed(f"curl -sf {sarah} {json_} -d '{{\"on\": true}}' http://127.0.0.1:4181/_dd/adblock/switch")

# a site blocking broke, let through for the house, and blocked again
box.succeed(f"curl -sf {sarah} {json_} -d '{{\"domain\": \"https://Tracker.Example.test/x\"}}' http://127.0.0.1:4181/_dd/adblock/allow")
state = json.loads(box.succeed(f"curl -sf {sarah} http://127.0.0.1:4181/_dd/adblock/state"))
assert [a["domain"] for a in state["allowed"]] == ["tracker.example.test"], state["allowed"]
assert state["allowed"][0]["by"] == "sarah", state["allowed"]
house.wait_until_fails("dig +short +time=2 +tries=1 @192.168.1.2 tracker.example.test | grep -qx 0.0.0.0", timeout=30)
box.succeed(f"curl -sf {sarah} -X DELETE http://127.0.0.1:4181/_dd/adblock/allow/tracker.example.test")
house.wait_until_succeeds("dig +short @192.168.1.2 tracker.example.test | grep -qx 0.0.0.0", timeout=30)
# not from another site's form
box.succeed(f"curl -s -o /dev/null -w '%{{http_code}}' {sarah} -d 'on=false' http://127.0.0.1:4181/_dd/adblock/switch | grep -qE '^(403|415)$'")
# and the page is hers
box.succeed(f"curl -sf {sarah} http://127.0.0.1:4181/_dd/adblock | grep -q adblock.js")
