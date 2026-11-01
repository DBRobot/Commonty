# A box as the installer sets it up carries the games, and runs them once its
# flag is up, on the dataset the installer named for them.
import json

from waits import wait_socket, wait_unit_active

box.start()
box.wait_for_unit("multi-user.target")

carries = json.loads(box.succeed("cat /etc/dd-placed/carries.json"))
assert "dd-games" in carries, carries
box.fail("systemctl is-active --quiet dd-games.service")

# its pool, as the installer leaves it
box.succeed(
    "zpool create -f tank /dev/vdb && head -c 32 /dev/urandom > /run/k"
    " && zfs create -o encryption=aes-256-gcm -o keyformat=raw -o keylocation=file:///run/k"
    " -o mountpoint=none tank/enc && systemctl restart zfs-datasets"
)
assert (
    box.succeed("zfs get -H -o value mountpoint tank/enc/games").strip()
    == "/var/lib/dd-games"
)

starts = json.loads(box.succeed("cat /etc/dd-placed/starts.json"))
box.succeed("mkdir -p /run/dd-placed && touch /run/dd-placed/dd-games")
box.succeed(
    "systemctl start --no-block " + " ".join(f"'{u}'" for u in starts["dd-games"])
)
wait_unit_active(box, "dd-games.service")
wait_socket(box, "/run/dd-games/dd-games.sock")
assert (
    box.succeed(
        "curl -sf --unix-socket /run/dd-games/dd-games.sock http://x/health"
    ).strip()
    == "ok"
)
# its records are on that dataset
assert box.succeed("findmnt -n -o SOURCE /var/lib/dd-games").strip() == "tank/enc/games"
box.succeed("test -d /var/lib/dd-games/records")

# nginx carries its name beside the box's own
wait_unit_active(box, "nginx.service")
# nginx is not on the test's path: the config its unit runs with
conf = box.succeed(
    'systemctl cat nginx.service | grep -oE "/nix/store/[^ \']+nginx.conf" | sort -u'
).strip()
assert "server_name games." in box.succeed(f"cat {conf}")
