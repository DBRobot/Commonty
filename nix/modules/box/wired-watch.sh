# shellcheck shell=bash
# Is the cable to the router passing traffic? A USB adapter can come back
# from a reset with its carrier up and nothing getting through, and then
# NetworkManager sees nothing wrong: the wired route stays first and the box
# is gone from the house until someone pulls the plug (node2, 2026-10-11).
# Run every 30 s: the router asked over the cable itself; after GRACE seconds
# of no answer the adapter is reset if it is USB; if that does not bring it
# back the wired profile goes down so the wifi carries everything, and the
# cable is tried again until it answers. Never a reboot.
# Environment: MAC (the wired adapter's), GATEWAY, PROFILE (house-wired),
# WIFI (the wifi interface), DIR (this unit's state), FACTS (node_exporter's textfile folder), GRACE,
# RETRY (seconds between full tries of the cable while on wifi).
set -u
# NetworkManager not up (stopped under a switch, or still starting): next time
nmcli -t -f RUNNING general >/dev/null 2>&1 || exit 0

now=$(date +%s)
# state: ok, failing or wifi; since when; when the failing began; resets
# since boot and the last one; the last full try of the cable while on wifi;
# when this failing was last acted on
state=ok since=$now fail=0 resets=0 reset_at=0 tried=0 acted=0
[ -s "$DIR/state" ] && read -r state since fail resets reset_at tried acted <"$DIR/state"

save() {
  printf '%s %s %s %s %s %s %s\n' "$state" "$since" "$fail" "$resets" "$reset_at" "$tried" "$acted" >"$DIR/state.tmp"
  mv -f "$DIR/state.tmp" "$DIR/state"
  [ -d "$FACTS" ] || return 0
  local n=0
  case $state in failing) n=1 ;; wifi) n=2 ;; esac
  # every line a number, or the collector drops the whole file
  printf '# the wired link to the router: 0 ok, 1 failing (being reset), 2 down and the box on wifi; since when; adapter resets since boot\ndd_wired_state %d\ndd_wired_since_seconds %d\ndd_wired_resets %d\ndd_wired_last_reset_seconds %d\n' \
    "$n" "$since" "$resets" "$reset_at" >"$FACTS/wired.prom.tmp"
  chmod 0644 "$FACTS/wired.prom.tmp"
  mv -f "$FACTS/wired.prom.tmp" "$FACTS/wired.prom"
}
to() { # to <state>: a new state, from now
  [ "$1" = "$state" ] || since=$now
  state=$1
}

iface() {
  local d
  for d in /sys/class/net/*; do
    [ "$(cat "$d/address" 2>/dev/null)" = "${MAC,,}" ] && { echo "${d##*/}"; return 0; }
  done
  return 1
}
# the router, over the cable and nothing else
answers() {
  ping -q -n -I "$1" -c 3 -i 0.5 -W 2 "$GATEWAY" >/dev/null 2>&1
}
# the router over the cable with the wired profile down: an ARP probe from
# no address, which the router answers like any ARP request. No route moves
heard() {
  local out
  ip link set dev "$1" up 2>/dev/null
  # -D answers 1 when something replied
  out=$(arping -D -I "$1" -c 3 "$GATEWAY" 2>/dev/null) || true
  [[ $out == *"Received "[1-9]* ]]
}
# wait up to $1 seconds for the cable to answer: the adapter coming back,
# and the profile up on it
comes_back() {
  local i=0 dev
  while [ "$i" -lt "$1" ]; do
    if dev=$(iface) && answers "$dev"; then return 0; fi
    sleep 5
    i=$((i + 5))
  done
  return 1
}
# the USB device behind an interface, if it is one: deauthorized and
# authorized again, which is a disconnect and a fresh probe of the driver
usb_reset() {
  local dev usb
  dev=$(readlink -f "/sys/class/net/$1/device" 2>/dev/null) || return 1
  [ "$(basename "$(readlink -f "$dev/subsystem" 2>/dev/null)")" = usb ] || return 1
  usb=${dev%/*}
  [ -w "$usb/authorized" ] || return 1
  echo "resetting the USB device ${usb##*/} behind $1"
  echo 0 >"$usb/authorized" || return 1
  sleep 2
  echo 1 >"$usb/authorized"
}
wired_down() {
  echo "the wifi carries the box now: taking $PROFILE down"
  nmcli connection down "$PROFILE" >/dev/null 2>&1 || true
  to wifi
  tried=$now
}

dev=$(iface) || dev=""
if [ "$state" = wifi ]; then
  # back the moment the router hears the cable again, and every RETRY a
  # full try in case this router does not answer a probe from no address
  if [ -n "$dev" ] && { heard "$dev" || [ $((now - tried)) -ge "$RETRY" ]; }; then
    tried=$now
    echo "trying the cable again: bringing $PROFILE up"
    if nmcli connection up "$PROFILE" >/dev/null 2>&1 && comes_back 20; then
      echo "the router answers over $dev again: the box is back on its wired link"
      to ok
      fail=0
    else
      echo "the router still does not answer over the cable: $PROFILE down again"
      nmcli connection down "$PROFILE" >/dev/null 2>&1 || true
    fi
  fi
  save
  exit 0
fi

if [ -n "$dev" ] && answers "$dev"; then
  [ "$state" = ok ] || echo "the router answers over $dev again"
  to ok
  fail=0
  save
  exit 0
fi

if [ "$fail" = 0 ]; then
  echo "the router ($GATEWAY) does not answer over ${dev:-the cable (no interface has $MAC)}"
  fail=$now
  to failing
fi
if [ $((now - fail)) -lt "$GRACE" ]; then
  save
  exit 0
fi

# once a failing: the reset, if there is one to try
if [ "$acted" -lt "$fail" ]; then
  acted=$now
  echo "no answer over the cable for $((now - fail)) s"
  if [ -n "$dev" ] && usb_reset "$dev"; then
    resets=$((resets + 1))
    reset_at=$now
    save
    if comes_back 45; then
      echo "the router answers over the cable after the reset"
      to ok
      fail=0
      save
      exit 0
    fi
    echo "still no answer after the reset"
  else
    echo "${dev:-the wired adapter} is not a USB device here: no reset to try"
  fi
fi
# only for a wifi the router answers on: with the wifi down too there is
# nothing to fall back on, and a router that answers neither is the router
# (or one that does not answer a ping), not the cable
if [[ $(nmcli -t -g GENERAL.STATE device show "$WIFI" 2>/dev/null) == 100* ]] && answers "$WIFI"; then
  wired_down
elif [ "$acted" = "$now" ]; then
  echo "the router does not answer over the wifi ($WIFI) either: $PROFILE stays up"
fi
save
