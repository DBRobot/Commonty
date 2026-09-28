#!/usr/bin/env bash
# Each running game server's ports, open to the devices of the people
# invited to it (modules/games/games.nix). The manager says which servers
# are up and whom their owners invited; the gate answers with devices, and
# only for players still the owner's friends. The chain is rebuilt when the
# answer changes, and emptied when either cannot say.
set -euo pipefail

mkdir -p "$STATE"
rules=$(mktemp "$STATE/rules.XXXXXX")
trap 'rm -f "$rules"' EXIT

if servers=$(jq -ce '.servers' "$ACCESS" 2>/dev/null) &&
  answer=$(jq -c '[.[] | {owner, players}]' <<<"$servers" |
    curl -sf --max-time 10 -H 'content-type: application/json' --data-binary @- "$GATE/internal/games/access") &&
  [ "$(jq length <<<"$answer")" = "$(jq length <<<"$servers")" ]; then
  # one line per device and port: "<address> <port>"
  jq -r --argjson a "$answer" '
    to_entries[] | .key as $k | .value.ports[] as $p
    | $a[$k].addresses[] | "\(.) \($p)"' <<<"$servers" | sort -u >"$rules"
else
  echo "games-access: no answer from the manager or the gate; every game port is closed"
  : >"$rules"
fi

if cmp -s "$rules" "$STATE/applied"; then
  exit 0
fi

for ipt in iptables ip6tables; do
  $ipt -w -F dd-games
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
    $ipt -w -A dd-games -s "$addr" -p "$proto" --dport "$port" -j nixos-fw-accept
  done
  n=$((n + 1))
done <"$rules"
mv "$rules" "$STATE/applied"
echo "games-access: $n device and port pair(s) open"
