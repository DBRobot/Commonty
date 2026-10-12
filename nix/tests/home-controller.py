import json
import re
import time

start_all()
for m in (house, stranger):
    m.wait_for_unit("tangd.socket")
controller.wait_for_unit("multi-user.target")


def thp(m):
    # the key Tang signs its advertisement with: what fleet/tang.json lists
    out = m.succeed("tang-show-keys 7654").split()
    return out[0]


house_thp = thp(house)
stranger_thp = thp(stranger)


def pins(*entries):
    controller.succeed(
        "cat > /tmp/pins.json <<'EOF'\n%s\nEOF" % json.dumps(list(entries))
    )


with subtest("a Tang that is not the listed one is never used"):
    # the stranger's address with the house's thumbprint, and the house's
    # address with the stranger's: neither binds, and nothing is written
    pins(
        {"url": "http://stranger:7654", "thp": house_thp},
        {"url": "http://house:7654", "thp": stranger_thp},
    )
    controller.fail("bash %s /dev/vdb /tmp/pins.json" % nix["lock"])
    controller.fail("cryptsetup isLuks /dev/vdb")

with subtest("locked to the house's Tang"):
    pins(
        {"url": "http://stranger:7654", "thp": house_thp},
        {"url": "http://house:7654", "thp": house_thp},
    )
    out = controller.succeed("bash %s /dev/vdb /tmp/pins.json" % nix["lock"])
    assert "locked to http://house:7654" in out, out
    controller.succeed("cryptsetup isLuks /dev/vdb")
    # one key slot: the one Clevis holds; the random key is gone
    slots = controller.succeed(
        "cryptsetup luksDump /dev/vdb | grep -c '^  [0-9]*: luks2'"
    ).strip()
    assert slots == "1", slots

with subtest("it opens while the house's Tang answers"):
    controller.succeed("clevis luks unlock -d /dev/vdb -n root")
    controller.succeed("blkid /dev/mapper/root | grep -q 'LABEL=\"controller\"'")
    controller.succeed("cryptsetup close root")

with subtest("and not when it does not"):
    house.succeed("systemctl stop tangd.socket")
    controller.fail("timeout 30 clevis luks unlock -d /dev/vdb -n root")
    house.succeed("systemctl start tangd.socket")
    house.wait_for_unit("tangd.socket")
    controller.succeed("clevis luks unlock -d /dev/vdb -n root")
    controller.succeed("cryptsetup close root")

with subtest("Tang answers the house network alone"):
    controller.succeed("curl -fsS --max-time 5 http://house:7654/adv >/dev/null")
    other = house.succeed(
        'ip -4 -o addr | awk \'$4 ~ /^192\\.168\\.2\\./ {sub("/.*", "", $4); print $4}\''
    ).strip()
    assert other.startswith("192.168.2."), other
    away.succeed("ping -c1 -W2 %s" % other)
    away.fail("curl -fsS --max-time 5 http://%s:7654/adv >/dev/null" % other)


# the door, and Home Assistant behind it
controller.wait_for_unit("home-assistant.service")
controller.wait_for_open_port(8123)
controller.wait_for_unit("dd-door.service")
controller.wait_for_open_port(8080)

with subtest("pairing needs the card's key"):
    controller.succeed("head -c 32 /dev/urandom > /tmp/wrong-key")
    controller.fail(
        "python3 %s pair %s /tmp/wrong-key commonty.test"
        % (nix["client"], nix["tester"])
    )

with subtest("paired with the card's key"):
    out = json.loads(
        controller.succeed(
            "python3 %s pair %s /var/lib/test-pair commonty.test"
            % (nix["client"], nix["tester"])
        )
    )
    name = out["name"]
    assert name.endswith(".home.commonty.test"), out
    # the network key for the network's unit; the pairing key gone; :8080 shut
    key = controller.succeed("cat /var/lib/commonty-door/network-key").strip()
    assert key == "test-network-key", key
    controller.fail("test -e /var/lib/test-pair")
    controller.wait_for_closed_port(8080)

with subtest("the household's device signs in, a stranger's does not"):
    me = json.loads(
        controller.succeed(
            "python3 %s signin %s %s" % (nix["client"], nix["tester"], name)
        )
    )
    assert me["cookie"].startswith("door="), me
    controller.fail("python3 %s signin %s %s 8" % (nix["client"], nix["tester"], name))
    # no cookie, no Home Assistant
    code = controller.succeed(
        "curl -s -o /dev/null -w '%%{http_code}' -H 'Host: %s' -H 'Accept: application/json' http://127.0.0.1/api/"
        % name
    )
    assert code == "401", code

with subtest("each person is their own Home Assistant user"):
    # made by the door, at an address of their own
    controller.wait_until_succeeds(
        "grep -q '^\"127.0.1.' /var/lib/hass/commonty-users.yaml", timeout=180
    )
    controller.wait_for_open_port(8123)
    cookie = me["cookie"]
    page = controller.wait_until_succeeds(
        "curl -fsS -H 'Accept: text/html' -H 'Host: %s' -H 'Cookie: %s' http://127.0.0.1/ | grep -q /_door/static && echo ok"
        % (name, cookie),
        timeout=180,
    )
    # Home Assistant reads its trusted users when it starts, and the door
    # restarts it once the file changes (behind the first backup, which
    # pairing sets going): asked until it has, it signs the address in as
    # its one user, with no page
    flow = (
        "curl -sS -H 'Host: %s' -H 'Cookie: %s' -H 'content-type: application/json' "
        '-d \'{"client_id": "http://%s/", "handler": ["trusted_networks", null], '
        '"redirect_uri": "http://%s/?auth_callback=1"}\' http://127.0.0.1/auth/login_flow'
        % (name, cookie, name, name)
    )
    said = ""
    for _ in range(150):
        said = controller.execute(flow)[1]
        if re.search(r'"type": *"create_entry"', said):
            break
        time.sleep(2)
    else:
        # what it said last, and who the door says is in, to see why
        print(said)
        print(
            controller.execute(
                "cat /var/lib/hass/commonty-users.yaml; journalctl -u dd-door -u home-assistant --no-pager | grep -iv 'hass.*  ' | tail -40"
            )[1]
        )
        raise AssertionError("Home Assistant never signed the person in")
    # from the door's own address, 127.0.0.1, it is not trusted
    direct = json.loads(
        controller.succeed(
            "curl -fsS -H 'content-type: application/json' "
            '-d \'{"client_id": "http://127.0.0.1:8123/", "handler": ["trusted_networks", null], '
            '"redirect_uri": "http://127.0.0.1:8123/?auth_callback=1"}\' http://127.0.0.1:8123/auth/login_flow || echo \'{"type": "refused"}\''
        )
    )
    assert direct["type"] != "create_entry", direct

with subtest("the home is backed up, and only the household can open it"):
    # the first backup goes up as pairing finishes: Home Assistant's own,
    # and the door's state, locked to the people in
    controller.wait_until_succeeds("test -s /var/lib/test-gate/backup", timeout=300)
    head = controller.succeed("head -c 4 /var/lib/test-gate/backup")
    assert head == "CBK1", head
    got = json.loads(
        controller.succeed("python3 %s /var/lib/test-gate/backup" % nix["openBackup"])
    )
    assert "home-assistant.tar" in got["names"], got
    assert "door-state.json" in got["names"], got
    # nothing in it is readable without the key: not the state, not a name
    controller.fail("grep -q tester /var/lib/test-gate/backup")
    # and Home Assistant's own copy does not stay on the card
    controller.wait_until_succeeds("! ls /var/lib/hass/backups/*.tar", timeout=60)
