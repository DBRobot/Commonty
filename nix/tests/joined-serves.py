# A joined box serves what the plan gives it, and only once the release's
# rules for joined boxes say they are attested (place.JoinedRules).
import json

fleet.start()
box.start()
fleet.wait_for_unit("nginx.service")

# the fleet: relay, witness, three validators and their gates (as
# joined-box), and the member's ticket
fleet.succeed(
    "mkdir -p /root/f/w /srv/test && cd /root/f"
    f" && cp {nix['control']} dd-control && cp {nix['verify']} verify"
    f" && cp {nix['worker']}/worker.js {nix['worker']}/wasm_exec.js {nix['worker']}/witness.wasm {nix['worker']}/placement.js w/"
    " && chmod -R u+w ."
)
fleet.succeed(
    "cat > /root/f/w/wrangler.toml <<'EOF'\n"
    'name = "joined-serves"\nmain = "worker.js"\ncompatibility_date = "2026-05-01"\nno_bundle = true\n'
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

# the box, under its key's name from the start (joined-box moves it there):
# by its capability it is built with the house's Tang and carries it, and
# Pi-hole is built in and not carried
box.wait_for_unit("multi-user.target")
assert box.succeed("hostname").strip() == nix["keyName"]
carries = json.loads(box.succeed("cat /etc/dd-placed/carries.json"))
assert "tang" in carries and "pihole" not in carries, carries
box.fail("systemctl is-active --quiet tangd.socket")

# its key in its TPM, its pool, the fleet's config, the release's inputs and
# its member's ticket; and its report, which meets the hardware minimum and
# says it has a pool (as joined-box makes it)
box.wait_until_succeeds(
    "curl -sf -o /dev/null http://fleet/test/config.json", timeout=120
)
box.succeed(
    "mkdir -p /run/dd-test /var/lib/dd && curl -sf http://fleet/test/config.json > /run/dd-test/config.json"
    f" && cp {nix['release']} /run/dd-test/placement.json"
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
# its report, made now its key is in its TPM; the test's own if the VM's
# hardware is too odd to report on
box.execute("systemctl restart dd-box-report")
if box.execute("test -s /var/lib/dd/report/hardware.json")[0] != 0:
    box.succeed(
        f"mkdir -p /var/lib/dd/report && cp {nix['report']} /var/lib/dd/report/hardware.json"
    )
hw = json.loads(box.succeed("cat /var/lib/dd/report/hardware.json"))
hw.setdefault("firmware", {}).update(
    {"uefi": True, "secureBootCapable": True, "tpm": {"version": 2, "p256": True}}
)
hw.setdefault("cpu", {})["threads"] = 4
hw["memoryBytes"] = 8 << 30
hw["bootDisk"] = "/dev/disk/by-id/test-system"
hw["disks"] = [
    {"id": "/dev/disk/by-id/test-system", "bytes": 256 * 10**9, "kind": "ssd"}
]
hw["pools"] = [{"name": "tank", "bytes": 20 << 30}]
hw["nics"] = hw.get("nics") or [{"name": "eth1", "wireless": False, "initrd": True}]
hw["gpus"] = [{"driver": "nvidia", "vram": 8 << 30, "encode": []}]
box.succeed(f"cat > /var/lib/dd/report/hardware.json <<'EOF'\n{json.dumps(hw)}\nEOF")
# what it can run, judged on that report by the release's rules
box.succeed("systemctl restart dd-capability")
box.succeed("grep -q 'service=\"tang\"' /var/lib/dd-facts/can.prom")
box.succeed("systemctl restart dd-join-prepare dd-control")

# it joins on its own key
fleet.wait_until_succeeds(
    f"curl -sf http://127.0.0.1:4191/state | jq -e '.joined.{nix['keyName']}'",
    timeout=300,
)
# and says it carries Tang and can run it
fleet.wait_until_succeeds(
    f"curl -sf http://127.0.0.1:4191/state | jq -e '.boxes.{nix['keyName']}.facts"
    ' | ."carries/tang" == 1 and ."can/tang" > 0\'',
    timeout=180,
)


def plan_gives(svc):
    p = json.loads(box.succeed("curl -sf http://127.0.0.1:4195/plan"))
    return any(d["service"] == svc for d in p["boxes"].get(nix["keyName"], []))


# the release's rules: joined boxes are not attested, so its word gives it
# nothing, and Tang stays shut. Three rounds of its plan, and its flag
# never goes up
box.wait_until_succeeds(
    "curl -sf http://127.0.0.1:4195/plan | jq -e '.boxes'", timeout=120
)
for _ in range(3):
    assert not plan_gives("tang"), box.succeed("curl -sf http://127.0.0.1:4195/plan")
    box.succeed("sleep 10")  # sleep-ok: never-runs check, a placement round each
box.fail("test -e /run/dd-placed/tang")
box.fail("systemctl is-active --quiet tangd.socket")
fleet.fail("curl -sf --max-time 5 http://192.168.1.50:7654/adv")

# the same release with joined boxes attested: the plan gives it Tang, its
# flag goes up, and Tang answers the house
box.succeed(
    f"cp {nix['attested']} /run/dd-test/placement.json && systemctl restart dd-control"
)
box.wait_until_succeeds("test -e /run/dd-placed/tang", timeout=180)
assert plan_gives("tang")
box.wait_for_unit("tangd.socket")
adv = json.loads(
    fleet.wait_until_succeeds("curl -sf http://192.168.1.50:7654/adv", timeout=120)
)
assert "payload" in adv and ("signature" in adv or "signatures" in adv), adv
# its heartbeat says it runs it
fleet.wait_until_succeeds(
    f"curl -sf http://127.0.0.1:4191/state | jq -e '.boxes.{nix['keyName']}.facts.\"runs/tang\" == 1'",
    timeout=120,
)
# and what its tier leaves out (the house's DNS: a household's) never runs
assert not plan_gives("pihole")
box.fail("test -e /run/dd-placed/pihole")
box.fail("systemctl is-active --quiet pihole-ftl.service")
