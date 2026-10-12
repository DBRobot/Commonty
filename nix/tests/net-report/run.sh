# shellcheck shell=bash
# report/nics.sh on node1's and node2's links as they were on 2026-10-11:
# sysfs trees made from what each box showed, nmcli and iw as they answered.
# Args: nics.sh, node2.json (what it must make of node2). Needs jq, gawk,
# coreutils, gnused, gnugrep on PATH.
set -eu
nics=$1
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT

# a sysfs device dir: its files, and its driver
dev() { # path driver [file=value...]
  local p=$1 drv=$2
  shift 2
  mkdir -p "$t/sys$p" "$t/sys/bus/drivers/$drv"
  ln -sfn "$t/sys/bus/drivers/$drv" "$t/sys$p/driver"
  for kv in "$@"; do printf '%s\n' "${kv#*=}" > "$t/sys$p/${kv%%=*}"; done
}
# a link: its name, its device, mac, carrier, speed; phy for Wi-Fi
link() { # name devpath mac carrier speed [phy]
  local n=$1
  mkdir -p "$t/sys/class/net/$n"
  ln -sfn "$t/sys$2" "$t/sys/class/net/$n/device"
  echo "$3" > "$t/sys/class/net/$n/address"
  echo "$4" > "$t/sys/class/net/$n/carrier"
  [ -z "$5" ] || echo "$5" > "$t/sys/class/net/$n/speed"
  if [ -n "${6:-}" ]; then
    mkdir -p "$t/sys/class/ieee80211/$6"
    ln -sfn "$t/sys/class/ieee80211/$6" "$t/sys/class/net/$n/phy80211"
  fi
}

mkdir -p "$t/bin"
# iw: a dual band card
cat > "$t/bin/iw" <<'EOF'
#!/bin/sh
cat <<'OUT'
Wiphy phy0
	Band 1:
		Frequencies:
			* 2412.0 MHz [1] (22.0 dBm)
			* 2472.0 MHz [13] (22.0 dBm)
	Band 2:
		Frequencies:
			* 5180.0 MHz [36] (22.0 dBm)
			* 5825.0 MHz [165] (22.0 dBm)
OUT
EOF
# nmcli: the house profiles, cable first; the Wi-Fi's DHCP DNS is node1's
cat > "$t/bin/nmcli" <<'EOF'
#!/bin/sh
case "$*" in
  *general*) echo running; exit 0 ;;
esac
dev=$(eval echo \${$#})
case "$dev" in
  enp*)
    printf 'GENERAL.DEVICE:%s\nGENERAL.CONNECTION:house-wired\nIP4.ROUTE[1]:dst = 0.0.0.0/0, nh = 192.168.1.1, mt = 10\nIP4.ROUTE[2]:dst = 192.168.1.0/24, nh = 0.0.0.0, mt = 10\nIP4.DNS[1]:1.1.1.1\nIP4.DNS[2]:9.9.9.9\n' "$dev" ;;
  wl*)
    printf 'GENERAL.DEVICE:%s\nGENERAL.CONNECTION:house-wifi\nIP4.ROUTE[1]:dst = 192.168.1.0/24, nh = 0.0.0.0, mt = 20\nIP4.ROUTE[2]:dst = 0.0.0.0/0, nh = 192.168.1.1, mt = 20\nIP4.DNS[1]:192.168.1.20\nIP4.DNS[2]:1.1.1.1\n' "$dev" ;;
esac
EOF
chmod +x "$t/bin/iw" "$t/bin/nmcli"

# node2: a Realtek usb adapter behind the AMD xhci, a MediaTek card on pci
x=/devices/pci0000:00/0000:00:08.1/0000:03:00.4
dev $x xhci_hcd class=0x0c0330 vendor=0x1022 device=0x15b9
dev $x/usb4/4-1 r8152-cfgselector idVendor=0bda idProduct=8153 speed=5000 manufacturer=Realtek "product=USB 10/100/1000 LAN"
dev $x/usb4/4-1/4-1:1.0 r8152
link enp3s0f4u1 $x/usb4/4-1/4-1:1.0 00:e0:4c:3a:46:58 1 1000
w=/devices/pci0000:00/0000:00:02.3/0000:02:00.0
dev $w mt7921e class=0x028000 vendor=0x14c3 device=0x7961 subsystem_vendor=0x11ad subsystem_device=0x3802
link wlp2s0 $w d0:39:57:65:37:a1 1 "" phy0
# no device behind it: not reported
mkdir -p "$t/sys/class/net/lo"

out=$(PATH="$t/bin:$PATH" SYS="$t/sys" INITRD_MODULES="r8152 xhci_pci" bash "$nics")
check() { # jq filter that must be true
  jq -e "$1" <<<"$out" >/dev/null || { echo "net-report: $1 is false in:" >&2; echo "$out" >&2; exit 1; }
}
check '.nics | length == 2'
check '.nics[0] == {name: "enp3s0f4u1", mac: "00:E0:4C:3A:46:58", driver: "r8152", wireless: false, usb: true, initrd: true,
  bus: "usb", ids: "0bda:8153", slot: "0000:03:00.4 4-1", product: "Realtek USB 10/100/1000 LAN",
  host: {driver: "xhci_hcd", ids: "1022:15b9"}, usbMbps: 5000, speedMbps: 1000, carrier: true}'
check '.nics[1] == {name: "wlp2s0", mac: "D0:39:57:65:37:A1", driver: "mt7921e", wireless: true, usb: false, initrd: false,
  bus: "pci", ids: "14c3:7961 11ad:3802", slot: "0000:02:00.0", speedMbps: null, carrier: true, bands: ["2.4", "5"]}'
check '.stack == [
  {link: "enp3s0f4u1", profile: "house-wired", metric: 10, dns: ["1.1.1.1", "9.9.9.9"], role: "primary"},
  {link: "wlp2s0", profile: "house-wifi", metric: 20, dns: ["192.168.1.20", "1.1.1.1"], role: "fallback"}]'
# and it is the report the config's own test reads (net-hardware.nix)
jq -S . <<<"$out" > "$t/got.json"
jq -S . "$2" | cmp - "$t/got.json"

# NetworkManager not running, iw finding no phy: links still reported,
# the stack unknown
printf '#!/bin/sh\nexit 8\n' > "$t/bin/nmcli"
printf '#!/bin/sh\nexit 1\n' > "$t/bin/iw"
out=$(PATH="$t/bin:$PATH" SYS="$t/sys" bash "$nics")
check '.stack == null and .nics[1].bands == null and .nics[0].initrd == false'

# node1: a pci adapter in a vm's virtio form: its ids from the function above
rm -rf "$t/sys/class/net"/*
v=/devices/pci0000:00/0000:00:03.0
dev $v virtio-pci class=0x020000 vendor=0x1af4 device=0x1000
dev $v/virtio0 virtio_net
link eth0 $v/virtio0 52:54:00:12:34:56 1 -1
out=$(PATH="$t/bin:$PATH" SYS="$t/sys" bash "$nics")
check '.nics == [{name: "eth0", mac: "52:54:00:12:34:56", driver: "virtio_net", wireless: false, usb: false, initrd: false,
  bus: "pci", ids: "1af4:1000", slot: "0000:00:03.0", speedMbps: null, carrier: true}]'
echo "net-report: ok"
