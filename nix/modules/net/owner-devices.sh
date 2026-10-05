# shellcheck shell=bash
# The owner's devices' addresses on the fleet's network, into the sets the
# firewall lets in on the admin ports (roles/core.nix). From this box's own
# view of the network: each peer and the user it belongs to. Swapped in
# whole, so a device that left is out by the next run.
set -eu
st=$(tailscale --socket "$SOCK" status --json)
ids=$(jq -r --arg o "$OWNER" '.User // {} | to_entries[]
  | select(.value.LoginName == $o or (.value.LoginName | startswith($o + "@"))) | .key' <<<"$st")
fill() {
  ipset create "$1-new" hash:ip family "$2" -exist
  ipset flush "$1-new"
  for id in $ids; do
    jq -r --argjson id "$id" --arg f "$3" '.Peer // {} | .[]
      | select(.UserID == $id and ((.Tags // []) | length) == 0)
      | .TailscaleIPs[] | select(test($f))' <<<"$st" \
      | while read -r a; do ipset add "$1-new" "$a" -exist; done
  done
  ipset create "$1" hash:ip family "$2" -exist
  ipset swap "$1-new" "$1"
  ipset destroy "$1-new"
}
fill dd-owner inet '^[0-9.]+$'
fill dd-owner6 inet6 ':'
echo "owner $OWNER: $(ipset list dd-owner | grep -c '^[0-9]') devices"
