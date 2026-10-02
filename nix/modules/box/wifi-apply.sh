# A Wi-Fi change a member made on the Network tab, tried on this box: the
# new name and password go into a profile of their own above the house one.
# If the box cannot join with them, that profile goes, the one before it
# comes back, and the box rejoins as it was. Either way the result is left
# for the gate to show (box/verify/src/home.rs).
# Environment: IFACE (the wifi interface), DIR (/run/dd-wifi).
set -u
req="$DIR/request"
# the gate may write in $DIR, so nothing of root's is made there but by a
# rename from root's own folder, and a link is not followed
own="$DIR/root"
[ -f "$req" ] && [ ! -L "$req" ] || { rm -f "$req"; exit 0; }
nonce=$(jq -r .nonce "$req")
ssid=$(jq -r .ssid "$req")
psk=$(jq -r .psk "$req")
rm -f "$req"

name=house-wifi-new
file=/etc/NetworkManager/system-connections/$name.nmconnection
prev="$own/prev.nmconnection"
rm -f "$prev"
[ -f "$file" ] && cp -p "$file" "$prev"

result() {
  jq -n --arg nonce "$nonce" --argjson ok "$1" --arg why "$2" --arg at "$(date +%s)" \
    '{nonce: $nonce, ok: $ok, why: $why, at: ($at | tonumber)}' > "$own/result.json"
  chmod 0644 "$own/result.json"
  mv -fT "$own/result.json" "$DIR/result.json"
}

# written as a keyfile, not given on a command line where ps would show it
umask 077
mkdir -p /etc/NetworkManager/system-connections
{
  printf '[connection]\nid=%s\ntype=wifi\ninterface-name=%s\nautoconnect=true\nautoconnect-priority=60\n\n' "$name" "$IFACE"
  printf '[wifi]\nmode=infrastructure\nssid=%s\ncloned-mac-address=permanent\n\n' "$ssid"
  printf '[wifi-security]\nkey-mgmt=wpa-psk\npsk=%s\n\n' "$psk"
  printf '[ipv4]\nmethod=auto\nroute-metric=20\n\n[ipv6]\nmethod=auto\n'
} > "$file.tmp"
mv "$file.tmp" "$file"
nmcli connection load "$file"

if nmcli --wait 45 connection up "$name" >/dev/null 2>&1; then
  rm -f "$prev"
  result true ""
  echo "joined the new wifi"
else
  # back as it was: the earlier change if there was one, else the house profile
  nmcli connection delete "$name" >/dev/null 2>&1 || rm -f "$file"
  if [ -f "$prev" ]; then
    mv "$prev" "$file"
    nmcli connection load "$file"
    nmcli --wait 45 connection up "$name" >/dev/null 2>&1 || nmcli --wait 45 connection up house-wifi >/dev/null 2>&1
  else
    nmcli --wait 45 connection up house-wifi >/dev/null 2>&1
  fi
  result false "could not join with those details"
  echo "the new wifi did not take; back on the old one" >&2
fi
