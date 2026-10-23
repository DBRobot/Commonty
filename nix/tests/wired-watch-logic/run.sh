# shellcheck shell=bash
# box/wired-watch.sh's choices, run by run against a made-up box: a sysfs
# tree with a USB (or PCI) cable adapter, and nmcli, ping, arping, date and
# sleep that answer from files here. Every way it can fall, and that it
# never leaves the box with neither link up.
# Args: wired-watch.sh. Needs coreutils, gnugrep, gnused on PATH.
set -eu
watch=$1
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
mkdir -p "$t/bin"

# nmcli: running or not, the wired profile up or down, the wifi connected
cat >"$t/bin/nmcli" <<'X'
#!/usr/bin/env bash
case "$*" in
  "-t -f RUNNING general") [ ! -e "$T/nm-down" ] ;;
  "connection up house-wired") echo up >"$T/profile"; echo up >>"$T/calls" ;;
  "connection down house-wired") echo down >"$T/profile"; echo down >>"$T/calls" ;;
  "-t -g GENERAL.STATE device show wifi0")
    if [ -e "$T/wifi-on" ]; then echo "100 (connected)"; else echo "30 (disconnected)"; fi ;;
  *) echo "nmcli: unexpected: $*" >&2; exit 2 ;;
esac
X
# ping over one link: the cable answers only with its profile up
cat >"$t/bin/ping" <<'X'
#!/usr/bin/env bash
while [ $# -gt 0 ]; do [ "$1" = -I ] && dev=$2; shift; done
case "$dev" in
  wifi0) [ -e "$T/wifi-ok" ] ;;
  *) [ -e "$T/cable-ok" ] && [ "$(cat "$T/profile")" = up ] ;;
esac
X
# arping -D from no address: the router hears a working cable either way
cat >"$t/bin/arping" <<'X'
#!/usr/bin/env bash
if [ -e "$T/cable-ok" ]; then echo "Received 1 response(s)"; exit 1; fi
echo "Received 0 response(s)"
X
printf '#!/bin/sh\nexit 0\n' >"$t/bin/ip"
printf '#!/bin/sh\ncat "$T/now"\n' >"$t/bin/date"
# the sleep inside a USB reset (authorized 0): the reset mends the cable
# where the test says it does
cat >"$t/bin/sleep" <<'X'
#!/usr/bin/env bash
if [ -e "$T/reset-mends" ] && [ "$(cat "$T/sys/devices/usb1/1-1/authorized")" = 0 ]; then touch "$T/cable-ok"; fi
X
# the build sandbox has no /usr/bin/env or /bin/sh
sed -i "1s|.*|#!$(command -v bash)|" "$t/bin"/*
chmod +x "$t/bin"/*

mac=00:E0:4C:3A:46:58
# a box: its cable adapter on usb (or pci), and its state cleared
box() { # usb|pci
  rm -rf "$t/sys" "$t/run" "$t/facts" "$t/calls" "$t"/cable-ok "$t"/wifi-ok "$t"/wifi-on "$t"/reset-mends "$t"/nm-down
  mkdir -p "$t/sys/class/net/enx0" "$t/sys/bus/usb" "$t/sys/bus/pci" "$t/run" "$t/facts"
  echo "${mac,,}" >"$t/sys/class/net/enx0/address"
  if [ "$1" = usb ]; then
    mkdir -p "$t/sys/devices/usb1/1-1/1-1:1.0"
    echo 1 >"$t/sys/devices/usb1/1-1/authorized"
    ln -s "$t/sys/bus/usb" "$t/sys/devices/usb1/1-1/1-1:1.0/subsystem"
    ln -s "$t/sys/devices/usb1/1-1/1-1:1.0" "$t/sys/class/net/enx0/device"
  else
    mkdir -p "$t/sys/devices/pci0/0000:01:00.0"
    ln -s "$t/sys/bus/pci" "$t/sys/devices/pci0/0000:01:00.0/subsystem"
    ln -s "$t/sys/devices/pci0/0000:01:00.0" "$t/sys/class/net/enx0/device"
  fi
  echo up >"$t/profile"
  : >"$t/calls"
  touch "$t/cable-ok" "$t/wifi-ok" "$t/wifi-on"
}
# one run at a time: it must always exit 0
at() { # seconds
  echo "$1" >"$t/now"
  T=$t PATH="$t/bin:$PATH" SYS="$t/sys" MAC=$mac GATEWAY=192.168.1.1 PROFILE=house-wired \
    WIFI=wifi0 DIR="$t/run" FACTS="$t/facts" GRACE=90 RETRY=600 \
    bash "$watch" >>"$t/log" 2>&1 || { cat "$t/log"; echo "FAIL: a run at $1 exited non-zero"; exit 1; }
}
fails=0
is() { # what value description
  if [ "$2" = "$3" ]; then echo "ok    $1"; else echo "FAIL  $1: '$2', not '$3'"; fails=$((fails + 1)); fi
}
state() { grep -o '^dd_wired_state [0-9]*' "$t/facts/wired.prom" | cut -d' ' -f2; }
resets() { sed -n 's/^dd_wired_resets //p' "$t/facts/wired.prom"; }
profile() { cat "$t/profile"; }
said() { grep -cF "$1" "$t/log" || true; }
: >"$t/log"

box usb
at 1000
is "a cable that answers: ok" "$(state)" 0

# the cable goes quiet; a usb reset mends it: no fallback, the profile up all along
box usb
at 1000
rm "$t/cable-ok"; touch "$t/reset-mends"
at 1030
is "no answer: failing" "$(state)" 1
at 1060
is "inside the grace: no reset yet" "$(resets)" 0
at 1130
is "after the grace: one reset, and the cable back" "$(state) $(resets)" "0 1"
is "the profile never went down" "$(grep -c down "$t/calls" || true)" 0

# the reset does not mend it, the wifi answers: the wifi carries the box
box usb
at 1000
rm "$t/cable-ok"
at 1030
at 1130
is "reset tried, then on the wifi" "$(state) $(resets) $(profile)" "2 1 down"
at 1160
is "on the wifi, the cable silent: the profile stays down" "$(profile)" down
# the cable answers again: heard from no address, the profile up, back on it
touch "$t/cable-ok"
at 1190
is "the cable back: on it again" "$(state) $(profile)" "0 up"

# on the wifi, and the wifi goes too: the cable up at once
box usb
at 1000
rm "$t/cable-ok"
at 1030
at 1130
is "on the wifi" "$(profile)" down
rm "$t/wifi-ok"
at 1160
is "the wifi silent too: the cable up again, failing" "$(state) $(profile)" "1 up"
# neither answers for a while: the reset is tried once more, the cable stays up
at 1260
at 1290
at 1400
is "neither answers: one more reset, the profile up" "$(resets) $(profile)" "2 up"
# the wifi back: the box moves to it again
touch "$t/wifi-ok"
at 1430
is "the wifi back, the cable not: on the wifi" "$(state) $(profile)" "2 down"

# neither link answers from the start (the router, say): the cable stays up
box usb
rm "$t/cable-ok" "$t/wifi-ok"
at 1000
at 1100
at 1130
at 1500
is "the router silent on both: reset once, the profile never down" "$(resets) $(profile) $(grep -c down "$t/calls" || true)" "1 up 0"

# the wifi card not connected: no fallback either
box usb
rm "$t/cable-ok" "$t/wifi-on"
at 1000
at 1100
is "no wifi connection: the profile stays up" "$(state) $(profile)" "1 up"

# a pci adapter: no reset to try, straight to the wifi
box pci
rm "$t/cable-ok"
at 1000
at 1100
is "pci: no reset, on the wifi" "$(state) $(resets) $(profile)" "2 0 down"
is "and said so" "$(said 'enx0 is not a USB device here')" 1

# on the wifi, the router does not answer probes from no address: a full
# try every RETRY, and down again while the cable is still silent
box pci
rm "$t/cable-ok"
at 1000
at 1100
: >"$t/calls"
at 1400
is "inside RETRY: no try" "$(grep -c up "$t/calls" || true)" 0
at 1710
is "after RETRY: tried, and down again" "$(tr '\n' ' ' <"$t/calls")" "up down "

# NetworkManager not running: nothing touched
box usb
rm "$t/cable-ok"
touch "$t/nm-down"
at 1000
at 1200
is "NetworkManager down: nothing done" "$(ls "$t/run") $(tr -d '\n' <"$t/calls")" " "

[ "$fails" = 0 ] || { echo "--- the watchdog's lines"; cat "$t/log"; exit 1; }
