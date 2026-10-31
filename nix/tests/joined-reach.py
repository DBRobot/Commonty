# Who reaches a service on a box that joined: by the agreed plan and whose it
# is, never the box's word; and nothing wider from the box itself.
import json

from waits import start_fresh

name = nix["keyName"]
sock = "--socket /run/commonty-net/tailscaled.sock"
start_all()
fleet.wait_for_unit("headscale-seed.service")
fleet.succeed(
    "mkdir -p /srv && systemd-run --unit files python3 -m http.server 80 --directory /srv"
)

fleet.succeed(
    "mkdir -p /root/f/w /srv/test && cd /root/f"
    f" && cp {nix['control']} dd-control && cp {nix['verify']} verify"
    f" && cp {nix['worker']}/worker.js {nix['worker']}/wasm_exec.js {nix['worker']}/witness.wasm {nix['worker']}/placement.js w/"
    " && chmod -R u+w ."
)
fleet.succeed(
    "cat > /root/f/w/wrangler.toml <<'EOF'\n"
    'name = "joined-reach"\nmain = "worker.js"\ncompatibility_date = "2026-05-01"\nno_bundle = true\n'
    'rules = [\n  { type = "CompiledWasm", globs = ["**/*.wasm"] },\n  { type = "ESModule", globs = ["**/*.js"] },\n]\n'
    '[[durable_objects.bindings]]\nname = "RELAY"\nclass_name = "Relay"\n'
    '[[migrations]]\ntag = "v1"\nnew_sqlite_classes = ["Relay"]\nEOF'
)
fleet.succeed(
    "systemd-run --unit fleet -p WorkingDirectory=/root/f"
    " -E HOME=/root -E WRANGLER_SEND_METRICS=false -E CI=1 -E CLOUDFLARE_CF_FETCH_ENABLED=false"
    " -E PATH=/run/current-system/sw/bin"
    f" -E FLEET_PLACEMENT={nix['attested']}"
    " -E FLEET_HEADSCALE=http://127.0.0.1:8085"
    " -E FLEET_HEADSCALE_KEY=/var/lib/dd-verify/headscale-api.key"
    f" python3 {nix['fleet']} /srv/test {nix['pages']}"
)
fleet.wait_until_succeeds("test -e /root/f/ready", timeout=600)

# the box's key in its TPM, its pool, the fleet's config and its member's
# ticket; its report says it can run the games (as joined-serves makes it)
box.wait_for_unit("multi-user.target")
assert box.succeed("hostname").strip() == name
carries = json.loads(box.succeed("cat /etc/dd-placed/carries.json"))
assert "dd-games" in carries, carries
box.wait_until_succeeds(
    "curl -sf -o /dev/null http://fleet/test/config.json", timeout=120
)
box.succeed(
    "mkdir -p /run/dd-test /var/lib/dd && curl -sf http://fleet/test/config.json > /run/dd-test/config.json"
    " && curl -sf http://fleet/test/ticket > /var/lib/dd/join-ticket && chmod 600 /var/lib/dd/join-ticket"
)
box.succeed(
    "export TPM2TOOLS_TCTI=device:/dev/tpmrm0 && w=$(mktemp -d) && cd $w && mkdir -p /boot/dd/unlock"
    " && tpm2_createprimary -Q -C o -G ecc256 -c srk.ctx"
    " && tpm2_startauthsession -Q -S s.ctx && tpm2_policypcr -Q -S s.ctx -l sha256:7 -L pcr.pol"
    " && tpm2_flushcontext s.ctx"
    f" && tpm2_import -Q -C srk.ctx -G ecc -i {nix['key']} -L pcr.pol -a 'sign|noda'"
    " -u /boot/dd/unlock/sig.pub -r /boot/dd/unlock/sig.priv"
)
box.succeed(
    "zpool create -f tank /dev/vdb && head -c 32 /dev/urandom > /run/k"
    " && zfs create -o encryption=aes-256-gcm -o keyformat=raw -o keylocation=file:///run/k"
    " -o mountpoint=none tank/enc && systemctl restart zfs-datasets"
)
box.execute("systemctl restart dd-box-report")
report = "/var/lib/dd/report/hardware.json"
if box.execute(f"test -s {report}")[0] != 0:
    box.succeed(f"mkdir -p /var/lib/dd/report && echo '{{}}' > {report}")
hw = json.loads(box.succeed(f"cat {report}"))
hw.setdefault("firmware", {}).update(
    {"uefi": True, "secureBootCapable": True, "tpm": {"version": 2, "p256": True}}
)
hw.setdefault("cpu", {}).update({"threads": 4, "arch": "x86_64"})
hw["memoryBytes"] = 8 << 30
hw["bootDisk"] = "/dev/disk/by-id/test-system"
hw["disks"] = [
    {"id": "/dev/disk/by-id/test-system", "bytes": 256 * 10**9, "kind": "ssd"}
]
hw["pools"] = [{"name": "tank", "bytes": 200 << 30}]
hw["virt"] = {"kvm": True, "nested": True}
hw["nics"] = hw.get("nics") or [{"name": "eth1", "wireless": False, "initrd": True}]
hw["gpus"] = [{"driver": "nvidia", "vram": 8 << 30, "encode": []}]
box.succeed(f"cat > {report} <<'EOF'\n{json.dumps(hw)}\nEOF")
box.succeed("systemctl restart dd-capability")
box.succeed("grep -q 'service=\"dd-games\"' /var/lib/dd-facts/can.prom")
box.succeed("systemctl restart dd-join-prepare dd-control")
fleet.wait_until_succeeds(
    f"curl -sf http://127.0.0.1:4191/state | jq -e '.joined.{name}'", timeout=300
)

# everyone on the network: two members' devices, a stranger's box, and the
# box, each as the gate would key it


def user_id(user):
    fleet.execute(f"headscale users create {user}")
    return fleet.succeed(
        f"headscale users list -o json | jq -r '.[] | select(.name == \"{user}\") | .id'"
    ).strip()


def key_for(user, *flags):
    return fleet.succeed(
        f"headscale preauthkeys create --user {user_id(user)} --expiration 10m {' '.join(flags)}"
    ).strip()


fleet.wait_until_succeeds(
    f"tailscale {sock} status --json | jq -e '.BackendState == \"Running\"'",
    timeout=180,
)
fleet_ip = fleet.succeed(f"tailscale {sock} ip -4").strip()
for vm, k in (
    (ada, key_for("ada")),
    (bob, key_for("bob")),
    (stranger, key_for("boxes", "--tags", "tag:joined")),
):
    vm.wait_for_unit("tailscaled.service")
    vm.execute(
        f"tailscale up --login-server http://fleet:8085 --auth-key {k} --accept-dns=false --timeout=60s"
    )
    vm.wait_until_succeeds(
        "tailscale status --json | jq -e '.BackendState == \"Running\"'", timeout=180
    )
k = key_for(f"joined-{name}", "--tags", "tag:joined")
box.wait_for_unit("commonty-net.service")
box.succeed(
    f"tailscale {sock} up --login-server http://fleet:8085 --auth-key {k} --hostname {name}"
    " --accept-dns=false --accept-routes=false --netfilter-mode=off --timeout=60s"
)
box_ip = box.succeed(f"tailscale {sock} ip -4").strip()
box.succeed(f"echo {box_ip} > /run/commonty-net-address")
ada_ip = ada.succeed("tailscale ip -4").strip()
fleet.wait_until_succeeds(
    f"curl -sf http://127.0.0.1:4191/state | jq -e '.joined.{name}.ipv4 > 0'",
    timeout=180,
)

# a listener for each port a member's reach is tried on
box.succeed("systemd-run --unit fake-game nc -lk 27000")
fleet.succeed("systemd-run --unit fake-443 nc -lk 443")


def rules():
    return json.loads(fleet.succeed("cat /var/lib/headscale/policy.json"))["acls"]


def reaches(vm, ip, port):
    return vm.execute(f"nc -z -w 4 {ip} {port}")[0] == 0


def plan_run():
    start_fresh(fleet, "headscale-plan.service")


with subtest("nobody's: joined, with a plan or none, it is open to no one"):
    plan_run()
    assert not any(box_ip in json.dumps(r) for r in rules()), rules()
    for vm in (ada, bob, stranger):
        assert not reaches(vm, box_ip, 443)
    # and its own ask for who may play is refused: it is no member's
    box.succeed(
        "mkdir -p /run/dd-games /run/games-access && cat > /tmp/ask.json <<'EOF'\n"
        '[{"owner": "ada", "players": []}]\nEOF'
    )
    code = box.succeed(
        f"{nix['control']} -box {name} -sign-games-access /tmp/ask.json"
        " | curl -s -o /dev/null -w '%{http_code}' -H 'content-type: application/json'"
        " --data-binary @- http://fleet:4281/_dd/network/games"
    ).strip()
    assert code == "403", code

fleet.succeed(
    f"cd /root/f && python3 {nix['fleet']} item enlist {name} 00112233445566778899aabbccddeeff"
)
fleet.wait_until_succeeds(
    f"curl -sf http://127.0.0.1:4191/identity | jq -e '.enlisted.{name}.owner == \"ada\"'",
    timeout=120,
)

# the plan gives it the games, by its facts and the release's rules
fleet.wait_until_succeeds(
    f"curl -sf http://127.0.0.1:4191/plan | jq -e '.boxes.{name} | any(.service == \"dd-games\")'",
    timeout=180,
)
box.wait_until_succeeds("test -e /run/dd-placed/dd-games", timeout=180)
box.wait_for_unit("nginx.service")
box.wait_until_succeeds("systemctl is-active dd-games.service", timeout=120)
# no real game here (games has one): the manager out of the way, kept out
# by the mask as the plan's flag stays up, and its list written for it
box.succeed("systemctl mask --runtime --now dd-games.service games-access.timer")
box.succeed(
    "mkdir -p /run/dd-games && cat > /run/dd-games/access.json <<'EOF'\n"
    '{"servers": [{"owner": "ada", "players": [], "ports": [27000]}]}\nEOF'
)

with subtest("the app's names: the games where the plan gave them to the fleet"):
    plan_run()
    plan = json.loads(fleet.succeed("curl -sf http://127.0.0.1:4191/plan"))
    # a member's own instance is theirs alone, and takes no fleet name
    own = any(d["service"] == "dd-games" and d.get("own") for d in plan["boxes"][name])
    records = {
        r["name"]: r["value"]
        for r in json.loads(fleet.succeed("cat /var/lib/headscale/extra-records.json"))
    }
    assert records["games.commonty.org"] == (fleet_ip if own else box_ip), records
    assert all(v == fleet_ip for n, v in records.items() if n != "games.commonty.org")

with subtest("hers: its door to her devices, and to no other member's or box's"):
    plan_run()
    assert any(
        r["src"] == ["ada@"] and r["dst"] == [f"{box_ip}:443"] for r in rules()
    ), rules()
    ada.wait_until_succeeds(f"nc -z -w 4 {box_ip} 443", timeout=60)
    ada.succeed(
        "curl -sk -o /dev/null -w '%{http_code}' --max-time 10 "
        f"--resolve games.commonty.org:443:{box_ip} https://games.commonty.org/"
    )
    assert not reaches(bob, box_ip, 443)
    assert not reaches(stranger, box_ip, 443)

with subtest("a game's port: to those its owner's server lets in, by the fleet's gate"):
    start_fresh(box, "games-access.service")
    box.succeed(f"iptables -S dd-games | grep -F -- '-s {ada_ip}/32' | grep -F 27000")
    ada.wait_until_succeeds(f"nc -z -w 4 {box_ip} 27000", timeout=60)
    assert not reaches(bob, box_ip, 27000)
    assert not reaches(stranger, box_ip, 27000)

with subtest("a server claimed for someone else opens to no one"):
    box.succeed(
        'cat > /tmp/ask.json <<\'EOF\'\n[{"owner": "bob", "players": ["ada"]}]\nEOF'
    )
    # again until past the door's beat between a box's asks
    box.wait_until_succeeds(
        f"{nix['control']} -box {name} -sign-games-access /tmp/ask.json"
        " | curl -sf -H 'content-type: application/json' --data-binary @-"
        " http://fleet:4281/_dd/network/games > /tmp/answer.json"
        " && jq -e '.[0].players == [] and .[0].addresses == []' /tmp/answer.json",
        timeout=60,
    )

with subtest("no wider path from the box itself"):
    assert box.execute(f"nc -z -w 4 {fleet_ip} 443")[0] != 0
    assert box.execute(f"nc -z -w 4 {ada_ip} 22")[0] != 0

with subtest("she takes it off: her door closes"):
    fleet.succeed(f"cd /root/f && python3 {nix['fleet']} item unenlist {name}")
    fleet.wait_until_succeeds(
        f"curl -sf http://127.0.0.1:4191/identity | jq -e '.enlisted.{name} == null'",
        timeout=120,
    )
    plan_run()
    assert not any(box_ip in json.dumps(r) for r in rules()), rules()
    ada.wait_until_fails(f"nc -z -w 4 {box_ip} 443", timeout=60)
