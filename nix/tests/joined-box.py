# A joined box's own system after its first boot: named by its key, moved
# to that name's system by its own agent, joined with its member's ticket;
# its member's once her root enlists it on that ticket, which opens the
# services to her - every one an invited member has, her gate reading the
# agreed state - and nobody's, and the services shut, once she removes it.
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
    f" && cp {nix['worker']}/worker.js {nix['worker']}/wasm_exec.js {nix['worker']}/witness.wasm {nix['worker']}/placement.js w/"
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
try:
    box.succeed("systemctl start dd-agent", timeout=900)
except Exception:
    print(
        box.execute(
            "journalctl --no-pager _SYSTEMD_INVOCATION_ID=$(systemctl show -p InvocationID --value dd-agent)"
            " | tail -100; ps axf -o pid,etime,args"
        )[1]
    )
    raise
# and collected no garbage on the way: the VM's store is the host's whole
# store under an overlay, so a collection would walk all of it over 9p and
# leave a whiteout for each path it removed (nix/lib/vm-test.nix)
gone = box.succeed("find /nix/.rw-store/upper -maxdepth 1 -type c | wc -l").strip()
assert gone == "0", f"nix collected garbage in the VM: {gone} store paths removed"
assert box.succeed("cat /etc/hostname").strip() == nix["keyName"]
assert box.succeed("readlink -f /run/current-system").strip() == named


def dump():
    """On a failed wait: the validators' and gates' logs, and how the box's
    dd-control came up (or did not), so a flake says where it stalled"""
    print(
        fleet.execute(
            'cd /root/f && for f in n1 n2 n3 g1 relay; do echo "--- $f.log"; tail -n 150 $f.log; done;'
            " curl -s http://127.0.0.1:4191/state | head -c 4000; echo; uptime"
        )[1]
    )
    print(
        box.execute(
            "uptime; systemctl list-jobs --no-pager; systemctl status dd-control --no-pager;"
            " systemd-analyze critical-chain dd-control.service --no-pager;"
            " systemd-analyze blame --no-pager | head -n 30;"
            " journalctl -b --no-pager -o short-monotonic | tail -n 400"
        )[1]
    )


def wait(machine, cmd, timeout):
    try:
        machine.wait_until_succeeds(cmd, timeout=timeout)
    except Exception:
        dump()
        raise


# below the hardware minimum it can't join, and says why (the VM is
# smaller than any box): then its report says it meets it, and has a GPU
# (a TPM 2.0, UEFI with Secure Boot, 4 threads, 8 GiB, a 256 GB system
# disk, a network link, 8 GiB of VRAM; the test's VM has some of it)
try:
    box.wait_for_unit("dd-control.service")
except Exception:
    dump()
    raise
wait(box, "grep -q needs /var/lib/dd-control/join-refused", timeout=120)
print(box.succeed("cat /var/lib/dd-control/join-refused"))
hw = json.loads(box.succeed("cat /var/lib/dd/report/hardware.json"))
hw.setdefault("firmware", {}).update(
    {"uefi": True, "secureBootCapable": True, "tpm": {"version": 2}}
)
hw.setdefault("cpu", {})["threads"] = 4
hw["memoryBytes"] = 8 << 30
hw["bootDisk"] = "/dev/disk/by-id/test-system"
hw["disks"] = [{"id": "/dev/disk/by-id/test-system", "bytes": 256 * 10**9}]
hw["nics"] = hw.get("nics") or [{"name": "eth1"}]
hw["gpus"] = [{"driver": "nvidia", "vram": 8 << 30, "encode": []}]
box.succeed(f"cat > /var/lib/dd/report/hardware.json <<'EOF'\n{json.dumps(hw)}\nEOF")

# and joins: on its own key, under its name, with its member's ticket's
# claim, never the ticket


def joined():
    s = json.loads(fleet.succeed("curl -sf http://127.0.0.1:4191/state"))
    return s.get("joined", {}).get(nix["keyName"])


wait(
    fleet,
    f"curl -sf http://127.0.0.1:4191/state | jq -e '.joined.{nix['keyName']}'",
    timeout=300,
)
j = joined()
assert j["ticket"] == "00112233445566778899aabbccddeeff", j
ticket = box.succeed("cat /var/lib/dd/join-ticket").strip()
st = fleet.succeed("curl -sf http://127.0.0.1:4191/state")
assert ticket[-40:] not in st, "the ticket is in the agreed state"
wait(box, "test ! -e /var/lib/dd-control/join-refused", timeout=120)
assert j["key"] == spki, j

# a stranger makes no backup password and backs nothing up
box.succeed("systemctl start dd-backup-seal")
box.fail("test -e /var/lib/dd/backup-password")
box.fail("test -e /var/lib/dd-backup-seal/sealed.json")

# not hers on its ticket alone, and signing up opened nothing: the box waits
# for her root, and the services are shut to her
token = fleet.succeed("cat /root/f/token").strip()
auth = f"-H 'Authorization: Bearer {token}'"
mine = f"curl -sf {auth} http://127.0.0.1:4281/_dd/boxes/mine"
verify = f"curl -s -o /dev/null -w '%{{http_code}}' {auth} http://127.0.0.1:4281/verify"


def refused(item, why):
    """Wait for the judge's word on one item, and that it refused it."""
    wait(
        fleet,
        f"curl -sf http://127.0.0.1:4191/identity | jq -e '.outcomes[\"{item}\"] != null'",
        timeout=120,
    )
    got = json.loads(fleet.succeed("curl -sf http://127.0.0.1:4191/identity"))[
        "outcomes"
    ][item]
    assert why in got, got


code = fleet.execute(
    f"curl -s -o /dev/null -w '%{{http_code}}' {auth} http://127.0.0.1:4281/_dd/boxes/mine"
)[1]
if code != "200":
    print(
        fleet.execute(
            "curl -s 'http://127.0.0.1:4191/identity?name=ada'; tail -60 /root/f/g1.log"
        )[1]
    )
    raise Exception(f"/_dd/boxes/mine: {code}")
assert json.loads(fleet.succeed(mine))["boxes"] == []
waiting = json.loads(
    fleet.succeed(f"curl -sf {auth} http://127.0.0.1:4281/_dd/network/joined")
)
assert waiting == [
    {
        "name": nix["keyName"],
        "ticket": "00112233445566778899aabbccddeeff",
        "joined": j["at"] // 1000 * 1000,
        "enlisted": False,
    }
], waiting
assert fleet.succeed(verify).strip() == "403"

# its heartbeats carry what its report says: it qualifies by its GPU
wait(
    fleet,
    f"curl -sf http://127.0.0.1:4191/state | jq -e '.boxes.{nix['keyName']}.facts | .gpus == 1 and .gpu_vram_bytes >= 8589934592 and .tpm_major == 2 and .uefi == 1'",
    timeout=180,
)

# a ticket the box did not join with: the judge refuses it, and the box
# stays nobody's (another member's ticket, and the rest, are state/ident's
# tests)
refused(
    fleet.succeed(
        f"cd /root/f && python3 {nix['fleet']} item enlist {nix['keyName']} ffeeddccbbaa99887766554433221100"
    ).strip(),
    "that box joined with another ticket",
)
assert json.loads(fleet.succeed(mine))["boxes"] == []

# ada's root enlists it on its ticket, through her gate
fleet.succeed(
    f"cd /root/f && python3 {nix['fleet']} item enlist {nix['keyName']} 00112233445566778899aabbccddeeff"
)
wait(
    fleet,
    f'{mine} | jq -e \'.boxes[] | select(.name == "{nix["keyName"]}" and .ticket == "00112233445566778899aabbccddeeff")\'',
    timeout=120,
)
ident = json.loads(fleet.succeed("curl -sf 'http://127.0.0.1:4191/identity?name=ada'"))[
    "member"
]
assert ident["access"] == "box", ident
assert ident["enlisted"][0]["qualifies"] == "gpu" and ident["enlisted"][0]["counts"], (
    ident
)
# hers: it makes its backup's password, sealed to her keys, and puts the copy
# beside the repository
box.wait_until_succeeds(
    "systemctl start dd-backup-seal && test -s /var/lib/dd-backup-seal/sealed.json",
    timeout=120,
)
sealed = json.loads(box.succeed("cat /var/lib/dd-backup-seal/sealed.json"))
assert sealed["owner"] == "ada" and sealed["keys"], sealed
assert box.succeed("stat -c %a /var/lib/dd/backup-password").strip() == "400"
password = box.succeed("cat /var/lib/dd/backup-password").strip()
assert password not in json.dumps(sealed)

# the services are open to her now
wait(fleet, f"test $({verify}) = 200", timeout=60)
assert json.loads(
    fleet.succeed(f"curl -sf {auth} http://127.0.0.1:4281/_dd/network/joined")
)[0]["enlisted"]

# ...every one of them, as for a member by an invite, on her gate in agreed
# mode. She signs in: a browser session of hers (the gate's own cookie,
# made with its secret as a sign-in makes it; the passkey ceremony is the
# gate's own tests')
cookie = fleet.succeed(
    "python3 -c 'import base64,hashlib,hmac,time;"
    's=open("/root/f/g1/session.secret","rb").read();w="ada~vmtest";e=int(time.time())+3600;'
    'm=base64.urlsafe_b64encode(hmac.new(s,f"{w}.{e}".encode(),hashlib.sha256).digest()).decode().rstrip("=");'
    'print(f"dd_session={w}.{e}.{m}")\''
).strip()
ck = f"-H 'cookie: {cookie}'"
me = json.loads(fleet.succeed(f"curl -sf {ck} http://127.0.0.1:4281/_dd/me"))
assert me["user"] == "ada", me


def door(host, how=ck):
    """What nginx's auth subrequest to her gate says for a service's door"""
    return fleet.succeed(
        f"curl -s -o /dev/null -w '%{{http_code}}' {how} -H 'x-original-host: {host}.commonty.test'"
        f" -H 'x-original-method: GET' -H 'x-original-uri: /' http://127.0.0.1:4281/verify"
    ).strip()


# Files, Mail, Photos, Passwords, the forge and the games: each door lets
# her through, in the browser and with her device's token
services = ["files", "mail", "photos", "vault", "git", "games"]
for host in services:
    assert door(host) == "200", (host, door(host))
    assert door(host, auth) == "200", (host, door(host, auth))
# the directory is members' to read, and says she is one, under her root
fleet.succeed(f"curl -sf {ck} http://127.0.0.1:4281/_dd/directory")
st = json.loads(
    fleet.succeed(f"curl -sf {auth} http://127.0.0.1:4281/_dd/directory/ada/standing")
)
assert st["member"] and st["access"] == "box" and st["from"] == "agreed", st
# a key to the fleet's network, for a device of hers
key = json.loads(
    fleet.succeed(f"curl -sf -X POST {auth} http://127.0.0.1:4281/_dd/network/join")
)
assert key["key"] == "test-preauth-key" and key["user"] == "ada", key
# a guest brought in: a friend link
link = json.loads(
    fleet.succeed(
        f"curl -sf -X POST {ck} -H 'content-type: application/json' -d '{{}}'"
        " http://127.0.0.1:4281/_dd/friends/link"
    )
)
assert "/_dd/friend/" in link["url"], link
# a remote computer of hers: the judge takes it on the box her item found
# (the box state the log kept with it), and it is up while her box counts
cid = "00000000000000a1"
item = fleet.succeed(
    f"cd /root/f && python3 {nix['fleet']} item computer {cid}"
).strip()
wait(
    fleet,
    f"curl -sf http://127.0.0.1:4191/computers | jq -e '.outcomes[\"{item}\"] != null'",
    timeout=120,
)
comps = json.loads(fleet.succeed("curl -sf http://127.0.0.1:4191/computers"))
assert comps["outcomes"][item] == "ok", comps["outcomes"][item]
assert comps["computers"][cid]["owner"] == "ada" and comps["computers"][cid]["up"], (
    comps
)
directory = json.loads(fleet.succeed("curl -sf http://127.0.0.1:4191/directory"))
kept = [i for i in directory["items"] if i.get("boxes")]
assert kept and nix["keyName"] in kept[-1]["boxes"], kept
assert "load" not in kept[-1]["boxes"][nix["keyName"]].get("facts", {}), kept[-1]

# she removes it: nobody's at once, and the services shut again
fleet.succeed(f"cd /root/f && python3 {nix['fleet']} item unenlist {nix['keyName']}")
wait(fleet, f"{mine} | jq -e '.boxes == []'", timeout=120)
wait(fleet, f"test $({verify}) = 403", timeout=60)
ident = json.loads(fleet.succeed("curl -sf 'http://127.0.0.1:4191/identity?name=ada'"))[
    "member"
]
assert ident["access"] == "none" and ident["enlisted"] == [], ident
# nobody's: its backup stops
box.wait_until_succeeds(
    "systemctl start dd-backup-seal && test ! -e /var/lib/dd-backup-seal/sealed.json",
    timeout=120,
)

# and every door with them: no service, no network key, no guest, her
# computer down
for host in services:
    assert door(host) == "403", (host, door(host))
assert (
    fleet.succeed(
        f"curl -s -o /dev/null -w '%{{http_code}}' -X POST {auth} http://127.0.0.1:4281/_dd/network/join"
    ).strip()
    == "403"
)
assert (
    fleet.succeed(
        f"curl -s -o /dev/null -w '%{{http_code}}' -X POST {ck} -H 'content-type: application/json'"
        " -d '{}' http://127.0.0.1:4281/_dd/friends/link"
    ).strip()
    == "403"
)
wait(
    fleet,
    f"curl -sf http://127.0.0.1:4191/computers | jq -e '.computers[\"{cid}\"].up == false'",
    timeout=120,
)
# and its ticket is used: enlisting it again on it is refused
refused(
    fleet.succeed(
        f"cd /root/f && python3 {nix['fleet']} item enlist {nix['keyName']} 00112233445566778899aabbccddeeff"
    ).strip(),
    "that ticket enlisted",
)
assert json.loads(fleet.succeed(mine))["boxes"] == []
