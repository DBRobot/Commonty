# A joined box's own system after its first boot: named by its key, moved
# to that name's system by its own agent, joined with its member's ticket,
# and its member's.
import json

# boxNamed is never started: it is the system the box moves to
fleet.start()
box.start()
fleet.wait_for_unit("nginx.service")

# the fleet: relay, witness, three validators and their gates; ada, her
# device, her ticket
fleet.succeed(
    "mkdir -p /root/f/w /srv/test && cd /root/f"
    f" && cp {nix['control']} dd-control && cp {nix['verify']} verify"
    f" && cp {nix['worker']}/worker.js {nix['worker']}/wasm_exec.js {nix['worker']}/witness.wasm w/"
    " && chmod -R u+w ."
)
fleet.succeed(
    "cat > /root/f/w/wrangler.toml <<'EOF'\n"
    'name = "joined-box"\nmain = "worker.js"\ncompatibility_date = "2026-05-01"\nno_bundle = true\n'
    'rules = [\n  { type = "CompiledWasm", globs = ["**/*.wasm"] },\n  { type = "ESModule", globs = ["**/*.js"] },\n]\n'
    '[[durable_objects.bindings]]\nname = "RELAY"\nclass_name = "Relay"\n'
    '[[migrations]]\ntag = "v1"\nnew_sqlite_classes = ["Relay"]\nEOF'
)
fleet.succeed(
    "systemd-run --unit fleet -p WorkingDirectory=/root/f"
    " -E HOME=/root -E WRANGLER_SEND_METRICS=false -E CI=1 -E CLOUDFLARE_CF_FETCH_ENABLED=false"
    " -E PATH=/run/current-system/sw/bin"
    f" python3 {nix['fleet']} /srv/test {nix['pages']}"
)
fleet.wait_until_succeeds("test -e /root/f/ready", timeout=600)

# the release: a commit of a stand-in for the fleet's repository, whose
# joined box under the key's name is the system built for it here, and
# nothing under any other name
named = box.succeed("cat /etc/dd-test/named").strip()
flake = (
    "{ outputs = { self }: { lib.joinedBox = { name, dir }: "
    f'if name == "{nix["keyName"]}" then {{ config.system.build.toplevel = builtins.storePath "{named}"; }} '
    'else throw "asked to build ${name}"; }; }'
)
fleet.succeed(
    "mkdir -p /srv/release /root/flake && cd /root/flake && git init -q && "
    f"cat > flake.nix <<'EOF'\n{flake}\nEOF\n"
    " git add flake.nix && git -c user.email=t@t -c user.name=t commit -qm flake && "
    "git clone -q --bare /root/flake /srv/flake.git && cd /srv/flake.git && git update-server-info"
)
rev = fleet.succeed("git -C /root/flake rev-parse HEAD").strip()
env = "DD_KEYRING_FILE=/root/keys.json"
fleet.succeed("git init -q /root/repo && mkdir -p /root/repo/fleet")
pub = fleet.succeed(
    f"{env} {nix['dd']} release init --repo /root/repo | tail -1"
).strip()
payload = json.dumps({"counter": 1, "rev": rev, "issued": 1000, "boxes": {}})
fleet.succeed(f"cat > /root/payload.json <<'EOF'\n{payload}\nEOF")
fleet.succeed(
    f"{env} {nix['dd']} release sign /root/payload.json --out /srv/release/current.json"
)

# the box, after its first boot (join-install's test): its key to join
# with in its TPM under PCR 7, as dd-unlock-seal makes it (here the test's
# key, so its name is known to build for), and the member's ticket
box.wait_for_unit("multi-user.target")
assert box.succeed("hostname").strip() == "joining"
box.fail("systemctl is-active dd-control")
assert "ConditionResult=no" in box.succeed(
    "systemctl show dd-control -p ConditionResult"
)
print(
    box.execute(
        "ip -4 addr; ip route; getent hosts fleet; curl -sS -o /dev/null http://fleet/test/config.json 2>&1"
    )[1]
)
box.wait_until_succeeds(
    "curl -sf -o /dev/null http://fleet/test/config.json", timeout=120
)
box.succeed(
    "mkdir -p /run/dd-test /var/lib/dd && curl -sf http://fleet/test/config.json > /run/dd-test/config.json"
    f" && echo '{pub}' > /run/dd-test/release.pub"
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
# its pool, encrypted, as the installer leaves it
box.succeed(
    "zpool create -f tank /dev/vdb && head -c 32 /dev/urandom > /run/k"
    " && zfs create -o encryption=aes-256-gcm -o keyformat=raw -o keylocation=file:///run/k"
    " -o mountpoint=none tank/enc && systemctl restart zfs-datasets"
)
box.succeed("systemctl restart dd-join-prepare")
# under its install name it does not try to join
assert "ConditionResult=no" in box.succeed(
    "systemctl show dd-control -p ConditionResult"
)
spki = box.succeed("cat /var/lib/dd/box-key.spki").strip()
name = box.succeed(f"echo {spki} | base64 -d | sha256sum | cut -c1-10").strip()
assert f"j{name}" == nix["keyName"], name

# its agent: named by the key, it takes the release's commit, builds its
# system under that name, and moves to it
box.succeed("systemctl start dd-agent", timeout=900)
assert box.succeed("cat /etc/hostname").strip() == nix["keyName"]
assert box.succeed("readlink -f /run/current-system").strip() == named

# and joins: on its own key, under its name, with its member's ticket
box.wait_for_unit("dd-control.service")


def joined():
    s = json.loads(fleet.succeed("curl -sf http://127.0.0.1:4191/state"))
    return s.get("joined", {}).get(nix["keyName"])


fleet.wait_until_succeeds(
    f"curl -sf http://127.0.0.1:4191/state | jq -e '.joined.{nix['keyName']}'",
    timeout=300,
)
j = joined()
assert j["ticket"] == box.succeed("cat /var/lib/dd/join-ticket").strip(), j
assert j["key"] == spki, j

# the member's gate: the box is hers
token = fleet.succeed("cat /root/f/token").strip()
fleet.wait_until_succeeds(
    f"curl -sf -H 'Authorization: Bearer {token}' http://127.0.0.1:4281/_dd/boxes/mine"
    f" | jq -e '.boxes[] | select(.name == \"{nix['keyName']}\")'",
    timeout=120,
)
