# The wired-watch test: a cable that passes nothing with its carrier up, the
# wifi taking over, and the cable back when it answers again.


def active():
    return box.succeed("nmcli -t -f NAME connection show --active").split()


def facts(line):
    box.wait_until_succeeds(
        f"grep -x '{line}' /var/lib/dd-facts/wired.prom", timeout=120
    )


def first_default(dev):
    routes = box.succeed("ip -4 route show default").splitlines()
    assert routes and f"dev {dev}" in routes[0], routes


def logged(text, timeout):
    box.wait_until_succeeds(
        f"journalctl -u dd-wired-watch.service --no-pager -o cat | grep -F '{text}'",
        timeout=timeout,
    )


router.wait_for_unit("network.target")
box.wait_for_unit("NetworkManager.service")
# both links up, the cable first
box.wait_until_succeeds(
    "nmcli -t -f NAME connection show --active | grep -x house-wired", timeout=60
)
box.wait_until_succeeds(
    "nmcli -t -f NAME connection show --active | grep -x house-wifi", timeout=60
)
box.wait_until_succeeds("ping -n -c 1 -W 1 -I wired0 192.168.1.1", timeout=60)
first_default("wired0")
facts("dd_wired_state 0")

# the cable goes quiet: everything it sends is lost, its carrier stays up,
# and NetworkManager sees nothing wrong
box.succeed("tc qdisc add dev wired0 root netem loss 100%")
box.succeed("test $(cat /sys/class/net/wired0/carrier) = 1")
logged("does not answer over wired0", 90)
facts("dd_wired_state 1")
# a VM's network card is no USB device: the reset is skipped, said so, and
# the wifi takes over
logged("wired0 is not a USB device here: no reset to try", 240)
logged("taking house-wired down", 30)
facts("dd_wired_state 2")
facts("dd_wired_resets 0")
assert "house-wired" not in active(), active()
assert "house-wifi" in active(), active()
first_default("wifi0")
# the router by the wifi, both ways
box.succeed("ping -n -c 2 -W 2 192.168.1.1")
router.succeed("ping -n -c 2 -W 2 192.168.1.30")
since = int(
    box.succeed(
        "sed -n 's/^dd_wired_since_seconds //p' /var/lib/dd-facts/wired.prom"
    ).strip()
)
assert since > 0, since

# the cable passes traffic again: the router hears it, and it is the box's
# link once more
box.succeed("tc qdisc del dev wired0 root")
logged("the box is back on its wired link", 180)
facts("dd_wired_state 0")
assert "house-wired" in active(), active()
box.succeed("ping -n -c 2 -W 2 -I wired0 192.168.1.1")
first_default("wired0")
# the watchdog's units never fail, whatever the link did
box.succeed("! systemctl is-failed -q dd-wired-watch.service")
