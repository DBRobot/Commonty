# How this box is on the house network, for the Network tab: by cable or
# not, and the wifi it holds with its signal. No passwords in here.
# Environment: IFACE, DIR.
set -u
wired=false
while IFS=: read -r dev type state; do
  [ "$type" = ethernet ] && [ "$state" = connected ] && wired=true
done < <(nmcli -t -f DEVICE,TYPE,STATE device)
wstate=$(nmcli -t -g GENERAL.STATE device show "$IFACE" 2>/dev/null | head -1)
conn=$(nmcli -t -g GENERAL.CONNECTION device show "$IFACE" 2>/dev/null | head -1)
ssid=""
[ -n "$conn" ] && ssid=$(nmcli -t -g 802-11-wireless.ssid connection show "$conn" 2>/dev/null | head -1)
signal=$(nmcli -t -f ACTIVE,SIGNAL device wifi list ifname "$IFACE" --rescan no 2>/dev/null | awk -F: '$1=="yes"{print $2; exit}')
jq -n --argjson wired "$wired" --arg state "$wstate" --arg ssid "$ssid" --arg signal "${signal:-}" --arg at "$(date +%s)" \
  '{wired: $wired, wifi: {state: $state, ssid: $ssid, signal: (if $signal == "" then null else ($signal | tonumber) end)}, at: ($at | tonumber)}' \
  > "$DIR/root/status.json"
chmod 0644 "$DIR/root/status.json"
mv -fT "$DIR/root/status.json" "$DIR/status.json"
