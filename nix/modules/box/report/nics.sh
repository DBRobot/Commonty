# shellcheck shell=bash
# This box's network links, for its report (detect.sh): every adapter with
# a device behind it, what it is and how it is attached, and how the box
# uses each (its stack). What the box config makes of it is in
# nix/lib/net-hardware.nix.
#
# Per adapter: its name and mac (the house profile names the cable by mac),
# its driver, how it hangs on the box - pci, or usb behind which host
# controller - and the ids that say what it is whatever its mac or name;
# its line speed and whether a cable is in; a Wi-Fi adapter's bands.
# The stack: which link carries the default route and which waits, by
# metric, the profile each is on and the DNS servers each hands out.
# Nothing that changes while it runs: no address a DHCP lease gives.
#
# Env: SYS (default /sys, a test's tree), INITRD_MODULES (space separated).
# iw and nmcli are used when on PATH.
set -eu
sys=${SYS:-/sys}

# a sysfs file's value, or empty
val() { [ -r "$1" ] && tr -d '\n' < "$1" || true; }
hex() { val "$1" | sed 's/^0x//'; }

# the bands a Wi-Fi adapter's phy offers, from iw: 2.4, 5, 6 (GHz)
bands() {
  local info
  info=$(iw phy "$1" info 2>/dev/null) || { echo null; return; }
  awk '
    /^[[:space:]]*Band [0-9]+:/ { b = $2; sub(":", "", b); next }
    /^[[:space:]]*\* [0-9.]+ MHz/ && b != "" {
      f = $2 + 0
      g = f < 3000 ? "2.4" : (f < 5925 ? "5" : "6")
      if (!(g in seen)) { seen[g] = 1; out = out (out == "" ? "" : ",") "\"" g "\"" }
    }
    END { print "[" out "]" }' <<<"$info"
}

nics=$(for d in "$sys"/class/net/*; do
  [ -e "$d/device" ] || continue
  i=$(basename "$d")
  dev=$(readlink -f "$d/device")
  drv=$(basename "$(readlink -f "$d/device/driver")")
  wl=false; { [ -e "$d/wireless" ] || [ -e "$d/phy80211" ]; } && wl=true
  ini=false; case " ${INITRD_MODULES:-} " in *" $drv "*) ini=true ;; esac
  carrier=null; case "$(val "$d/carrier" 2>/dev/null)" in 1) carrier=true ;; 0) carrier=false ;; esac
  speed=$(val "$d/speed" 2>/dev/null); case "$speed" in '' | -*) speed=null ;; esac
  bus=other; ids=""; product=""; slot=""; host=null; usbmbps=null
  case "$dev" in
    */usb[0-9]*)
      bus=usb
      # the usb device is the interface's parent
      u=$(dirname "$dev")
      ids="$(val "$u/idVendor"):$(val "$u/idProduct")"
      product="$(val "$u/manufacturer") $(val "$u/product")"
      usbmbps=$(val "$u/speed"); [ -n "$usbmbps" ] || usbmbps=null
      # the host controller: the first pci device above it (class 0c03xx)
      h=$u
      while [ "$h" != "$sys" ] && [ "$h" != / ]; do
        case "$(val "$h/class")" in 0x0c03*) break ;; esac
        h=$(dirname "$h")
      done
      if [ -e "$h/class" ]; then
        slot="$(basename "$h") $(basename "$u")"
        host=$(jq -nc --arg driver "$(basename "$(readlink -f "$h/driver")")" \
          --arg ids "$(hex "$h/vendor"):$(hex "$h/device")" '{driver: $driver, ids: $ids}')
      fi
      ;;
    */pci[0-9]*)
      bus=pci
      p=$dev
      # a virtio adapter's device is under its pci function
      [ -e "$p/vendor" ] || p=$(dirname "$p")
      ids="$(hex "$p/vendor"):$(hex "$p/device")"
      sub="$(hex "$p/subsystem_vendor"):$(hex "$p/subsystem_device")"
      [ "$sub" = ":" ] || ids="$ids $sub"
      slot=$(basename "$p")
      ;;
  esac
  b=null
  if [ "$wl" = true ] && [ -e "$d/phy80211" ]; then b=$(bands "$(basename "$(readlink -f "$d/phy80211")")"); fi
  jq -nc --arg name "$i" --arg mac "$(tr a-f A-F < "$d/address")" --arg driver "$drv" \
    --argjson wireless $wl --argjson usb "$([ $bus = usb ] && echo true || echo false)" --argjson initrd $ini \
    --arg bus $bus --arg ids "$ids" --arg product "$(echo "$product" | sed 's/^ *//; s/ *$//')" --arg slot "$slot" \
    --argjson host "$host" --argjson usbMbps "$usbmbps" --argjson speedMbps "$speed" \
    --argjson carrier "$carrier" --argjson bands "$b" '
    {name: $name, mac: $mac, driver: $driver, wireless: $wireless, usb: $usb, initrd: $initrd,
     bus: $bus, ids: $ids, slot: $slot}
    + (if $product == "" then {} else {product: $product} end)
    + (if $bus == "usb" then {host: $host, usbMbps: $usbMbps} else {} end)
    + {speedMbps: $speedMbps, carrier: $carrier}
    + (if $wireless then {bands: $bands} else {} end)'
done | jq -s .)

# how the box uses them: NetworkManager's view, when it runs here
stack=null
if command -v nmcli >/dev/null 2>&1 && nmcli -t -f RUNNING general >/dev/null 2>&1; then
  stack=$(for n in $(jq -r '.[].name' <<<"$nics"); do
    nmcli -t -f GENERAL.DEVICE,GENERAL.CONNECTION,IP4.ROUTE,IP4.DNS dev show "$n" 2>/dev/null | awk -F: -v dev="$n" '
      $1 == "GENERAL.CONNECTION" { profile = substr($0, length($1) + 2) }
      $1 ~ /^IP4\.DNS/ { dns = dns (dns == "" ? "" : ",") "\"" $2 "\"" }
      $1 ~ /^IP4\.ROUTE/ && $0 ~ /dst = 0\.0\.0\.0\/0/ { m = $0; sub(/.*mt = /, "", m); sub(/[^0-9].*/, "", m); metric = m }
      END { printf "{\"link\":\"%s\",\"profile\":%s,\"metric\":%s,\"dns\":[%s]}\n", dev,
        (profile == "" ? "null" : "\"" profile "\""), (metric == "" ? "null" : metric), dns }'
  done | jq -s '
    map(select(.profile != null))
    | (map(select(.metric != null)) | sort_by(.metric) | first | .link // null) as $primary
    | map(. + {role: (if .link == $primary then "primary" elif .metric != null then "fallback" else "other" end)})')
fi

jq -n --argjson nics "$nics" --argjson stack "$stack" '{nics: $nics, stack: $stack}'
