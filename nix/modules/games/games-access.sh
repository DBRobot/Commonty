#!/usr/bin/env bash
# Each running game server's ports, open to the devices of the people
# invited to it (modules/games/games.nix). The manager says which servers
# are up and whom their owners invited; the gate answers with devices, and
# only for players still the owner's friends. The chain is rebuilt when the
# answer changes, and emptied when either cannot say. A box with no network
# door of its own (one that joined) asks the fleet's gate, signed (DOOR, BOX,
# CONTROL_BIN): what its answer holds is that gate's, by the agreed state.
set -euo pipefail
# the chain it fills: the games', or members' computers' (computers.nix)
CHAIN=${CHAIN:-dd-games}

mkdir -p "$STATE"
rules=$(mktemp "$STATE/rules.XXXXXX")
trap 'rm -f "$rules" "$rules.ask"' EXIT

ask_gate() {
  if [ -z "${DOOR:-}" ]; then
    curl -sf --max-time 10 -H 'content-type: application/json' --data-binary @- "$GATE/internal/games/access"
    return
  fi
  cat >"$rules.ask"
  # no server, no one to ask for: its TPM and the door are left alone
  [ "$(jq length "$rules.ask")" != 0 ] || { echo '[]'; return; }
  "$CONTROL_BIN" -box "$BOX" -sign-games-access "$rules.ask" |
    curl -sf --max-time 20 -H 'content-type: application/json' --data-binary @- "$DOOR"
}

# one line per device and port: "<address> <port>". Anything that does not
# read as it should - no answer, the wrong shape, a jq error anywhere - is
# no rules at all: every port closed, never the rules before left standing
rules_now() {
  local servers answer
  servers=$(jq -ce '.servers | if type == "array" then . else error("not a list") end' "$ACCESS") || return 1
  answer=$(jq -c '[.[] | {owner, players}]' <<<"$servers" | ask_gate) || return 1
  jq -e --argjson s "$servers" 'type == "array" and length == ($s | length)' <<<"$answer" >/dev/null || return 1
  jq -r --argjson a "$answer" '
    to_entries[] | .key as $k | .value.ports[] as $p
    | $a[$k].addresses[] | "\(.) \($p)"' <<<"$servers" | sort -u
}
if ! rules_now >"$rules"; then
  echo "$CHAIN access: no answer from the manager or the gate, or not one that reads; every port is closed"
  : >"$rules"
  # and the chain emptied whatever was applied before
  rm -f "$STATE/applied"
fi

if cmp -s "$rules" "$STATE/applied"; then
  exit 0
fi

# the chain is the firewall's; while it reloads, or on a box without it,
# there is nothing to fill, and the firewall starts this again once it is up
for ipt in iptables ip6tables; do
  if ! $ipt -w -n -L "$CHAIN" >/dev/null 2>&1; then
    echo "$CHAIN access: no $CHAIN chain in $ipt yet; nothing to do"
    exit 0
  fi
done

for ipt in iptables ip6tables; do
  $ipt -w -F "$CHAIN"
done
n=0
while read -r addr port; do
  # the gate's word, but still only an address and a port go in a rule
  [[ $port =~ ^[0-9]{1,5}$ ]] || continue
  if [[ $addr =~ ^[0-9]{1,3}(\.[0-9]{1,3}){3}$ ]]; then
    ipt=iptables
  elif [[ $addr =~ ^[0-9a-fA-F:]+$ ]]; then
    ipt=ip6tables
  else
    continue
  fi
  for proto in tcp udp; do
    $ipt -w -A "$CHAIN" -s "$addr" -p "$proto" --dport "$port" -j nixos-fw-accept
  done
  n=$((n + 1))
done <"$rules"
mv "$rules" "$STATE/applied"
echo "$CHAIN access: $n device and port pair(s) open"
