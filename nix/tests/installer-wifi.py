import json

blank.wait_for_unit("NetworkManager.service")
blank.wait_for_unit("dd-box-setup.service")
blank.succeed("systemd-run --unit=ap hostapd /etc/hostapd.conf")
# the house's router: an address for whoever joins
blank.succeed("ip addr add 10.9.0.1/24 dev wlan0")
blank.succeed(
    "systemd-run --unit=dhcp dnsmasq -d --port=0 -i wlan0 --bind-interfaces "
    "--dhcp-range=10.9.0.10,10.9.0.50,1h"
)
blank.wait_until_succeeds("ip -o link show wlan0 | grep -c 'state UP'", timeout=60)

token = blank.succeed("cat /run/dd-setup-token/token").strip()
mine = f"-H 'x-setup-token: {token}'"


def ask(path, body=None):
    if body is None:
        return blank.succeed(f"curl -s {mine} http://127.0.0.1:8090/api/{path}")
    return blank.succeed(
        f"curl -s {mine} -H 'content-type: application/json' "
        f"-d '{json.dumps(body)}' http://127.0.0.1:8090/api/{path}"
    )


blank.wait_until_succeeds(
    f"curl -sf {mine} -o /tmp/nets http://127.0.0.1:8090/api/wifi && grep -c 'House Wi-Fi' /tmp/nets",
    timeout=90,
)
assert json.loads(ask("network"))["wifi"] is None

# a wrong password: refused, with NetworkManager's reason, and nothing kept
wrong = ask("wifi", {"ssid": "House Wi-Fi", "psk": "not the password"})
print(wrong)
assert "could not join House Wi-Fi" in wrong, wrong
assert json.loads(ask("network"))["wifi"] is None
blank.fail("test -e /run/dd-setup/psk")
blank.fail("test -e /run/dd-setup/wifi.json")

# the right one, typed: joined, and kept for the install
assert json.loads(
    ask("wifi", {"ssid": "House Wi-Fi", "psk": "correct horse battery"})
) == {"joined": "House Wi-Fi"}
assert json.loads(ask("network"))["wifi"] == "House Wi-Fi"
blank.fail("test -e /run/dd-setup/psk")
kept = json.loads(blank.succeed("cat /run/dd-setup/wifi.json"))
assert kept["ssid"] == "House Wi-Fi"

# the drive's, as the app wrote it: off the network first, then joined from it
blank.succeed("nmcli connection delete dd-setup")
blank.wait_until_succeeds(
    f"curl -sf {mine} http://127.0.0.1:8090/api/network | jq -e '.wifi == null'",
    timeout=30,
)
blank.succeed(
    "mkdir -p /root/drive && "
    """echo '{"ssid": "House Wi-Fi", "psk": "correct horse battery"}' > /root/drive/wifi.json"""
)
assert json.loads(ask("network"))["drive"] == "House Wi-Fi"
assert json.loads(ask("wifi", {"drive": True})) == {"joined": "House Wi-Fi"}
assert json.loads(ask("network"))["wifi"] == "House Wi-Fi"
