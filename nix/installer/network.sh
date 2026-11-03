# shellcheck shell=bash
# The box installer on the fleet's own network, for the backup of the disk
# it is about to wipe (backup.sh) and nothing else: `up` joins, `down`
# leaves. The gate gives the key on the member's join ticket, narrowed
# here to a key sealed to a recipient made for this one join, so the front
# door and Cloudflare carry nothing they could use. The key is single-use,
# good ten minutes, and its node ephemeral and tagged an installer: the
# network lets it reach the boxes' front doors (443) and nothing else, and
# nothing reach it (modules/net/headscale.nix, gate network.rs).
# Environment: TICKET (a file), NET_DOOR (the gate's /_dd/network/installer),
# NET_CONTROL (the control server), NET_BRIDGE (1: through the front door,
# with the bridge in front, as a box asks).
set -euo pipefail
d=/run/dd-installer-net
sock=$d/tailscaled.sock
case "${1:-}" in
  up)
    [ -s "$TICKET" ] || { echo "no join ticket on the drive" >&2; exit 1; }
    rm -rf "$d"
    install -d -m 0700 "$d"
    id=$(dd-box-setup --ticket-id "$TICKET")
    # this join's own recipient, in memory (/run) and gone with it
    age-keygen -o "$d/key" 2>/dev/null
    recipient=$(age-keygen -y "$d/key")
    ticket=$(dd-box-setup --narrow-ticket "$TICKET" --recipient "$recipient")
    jq -n --arg t "$ticket" --arg r "$recipient" '{ticket: $t, recipient: $r}' \
      | curl -fsS --max-time 60 -H 'content-type: application/json' --data-binary @- "$NET_DOOR" \
      | age -d -i "$d/key" > "$d/authkey" \
      || { echo "the fleet gave no way onto its network for the backup" >&2; exit 1; }
    login=$NET_CONTROL
    if [ "${NET_BRIDGE:-}" = 1 ]; then
      login=http://127.0.0.1:41643
      netbridge -listen 127.0.0.1:41643 -control "$NET_CONTROL" > "$d/bridge.log" 2>&1 &
      echo $! > "$d/bridge.pid"
    fi
    tailscaled --state=mem: --socket="$sock" --port=0 --tun=commonty0 --no-logs-no-support > "$d/tailscaled.log" 2>&1 &
    echo $! > "$d/tailscaled.pid"
    for _ in $(seq 1 30); do [ -S "$sock" ] && break; sleep 1; done
    tailscale --socket="$sock" up --login-server="$login" --auth-key="file:$d/authkey" \
      --hostname="installer-${id:0:8}" --accept-dns=false --accept-routes=false --netfilter-mode=off --timeout=120s
    rm -f "$d/authkey"
    echo "on the fleet's network for the backup: $(tailscale --socket="$sock" ip -4)" >&2
    ;;
  down)
    [ -d "$d" ] || exit 0
    # off the network at once: the node is gone, not left to time out
    [ -S "$sock" ] && tailscale --socket="$sock" logout 2>/dev/null || true
    for p in tailscaled bridge; do
      [ -s "$d/$p.pid" ] && kill "$(cat "$d/$p.pid")" 2>/dev/null || true
    done
    rm -rf "$d"
    ;;
  *)
    echo "dd-installer-net up|down" >&2
    exit 2
    ;;
esac
