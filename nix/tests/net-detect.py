# The net-detect test: the report's links, from the kernel and NetworkManager.
import json

box.wait_for_unit("NetworkManager.service")
box.wait_until_succeeds(
    "nmcli -t -f NAME connection show --active | grep -x house-wired", timeout=60
)
box.wait_until_succeeds(
    "nmcli -t -f NAME connection show --active | grep -x house-wifi", timeout=60
)

report = json.loads(box.succeed("INITRD_MODULES='virtio_net' bash /etc/nics.sh"))
nics = {n["name"]: n for n in report["nics"]}
print(json.dumps(report, indent=2))

for name in ("wired0", "wifi0"):
    n = nics[name]
    assert n["driver"] == "virtio_net" and n["bus"] == "pci", n
    assert n["ids"].startswith("1af4:"), n
    assert n["slot"].startswith("0000:"), n
    assert n["usb"] is False and n["wireless"] is False and n["initrd"] is True, n
    assert n["carrier"] is True, n
assert "eth0" not in nics or nics["eth0"]["driver"] == "virtio_net"

stack = {s["link"]: s for s in report["stack"]}
assert stack["wired0"] == {
    "link": "wired0",
    "profile": "house-wired",
    "metric": 10,
    "dns": ["1.1.1.1", "9.9.9.9"],
    "role": "primary",
}, stack
assert stack["wifi0"] == {
    "link": "wifi0",
    "profile": "house-wifi",
    "metric": 20,
    "dns": ["192.168.1.21", "1.1.1.1"],
    "role": "fallback",
}, stack

# the cable gone: the waiting link is the one that carries the default route
box.succeed("nmcli connection down house-wired")
report = json.loads(box.succeed("bash /etc/nics.sh"))
stack = {s["link"]: s for s in report["stack"]}
assert stack["wifi0"]["role"] == "primary" and "wired0" not in stack, stack
