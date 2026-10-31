# shellcheck shell=bash
# What the agreed plan changes of the network, for boxes that joined on their
# own key. Who reaches one: its owner's devices its front door, once a member
# enlisted it and the plan gives it a service; any member's its game ports
# where the plan gives it games, its own firewall then keeping them to the
# people invited (games-access.sh). And where the app's DNS answers a name of
# a service the plan gives only boxes that joined. All from this box's
# dd-control, nothing a box says; with no fresh agreement, what stands stays.
# Environment: CONTROL, SOCK, STATE, BASE, BASE_POLICY, POLICY, NAMES, FOLLOW,
# GIVABLE, GAME_PORTS.
set -u
st=$(curl -fsS --max-time 10 "$CONTROL/state") || exit 0
plan=$(curl -fsS --max-time 10 "$CONTROL/plan") || exit 0
own=$(tailscale --socket "$SOCK" ip -4 2>/dev/null | head -n 1)
[ -n "$own" ] || exit 0
now=$(date +%s)
jq -e --argjson now "$now" '(.time // 0) / 1000 > ($now - 120)' <<<"$st" >/dev/null || exit 0
# whose each box is: none known, no one's
identity=$(curl -fsS --max-time 10 "$CONTROL/identity") || identity='{}'

facts=$(mktemp)
trap 'rm -f "$facts" "$facts.rules" "$facts.names"' EXIT
jq -n --argjson st "$st" --argjson plan "$plan" --argjson id "$identity" \
  --argjson follow "$FOLLOW" --argjson givable "$GIVABLE" '
  def dotted: [(. / 16777216 | floor), ((. / 65536 | floor) % 256), ((. / 256 | floor) % 256), (. % 256)]
    | map(tostring) | join(".");
  # a box that joined, at its address on the fleet network: 100.64.0.0/10
  (($st.joined // {}) | to_entries
    | map(select((.value.ipv4 | type) == "number" and (.value.ipv4 / 4194304 | floor) == 401))
    | map({key, value: (.value.ipv4 | dotted)}) | from_entries) as $addr
  | (($plan.boxes // {}) | to_entries
      | map(.key as $b | (.value // [])[] | {box: $b, svc: .service, own: (.own // false)})
      | map(select(.svc as $s | $givable | index($s)))) as $given
  | {addr: $addr, given: $given,
     owners: (($id.enlisted // {}) | to_entries | map(select(
         .value.owner as $o
         | ($o | test("^[a-z0-9][a-z0-9._-]{0,63}$"))
           and (($id.members // {})[$o].status == "member")))
       | map({key, value: .value.owner}) | from_entries),
     follow: $follow}' >"$facts"

jq --argjson ports "$GAME_PORTS" '
  . as $f
  | [$f.given[].box] | unique | map(select($f.addr[.] != null)) as $boxes
  | [ $boxes[] as $b
      | ($f.addr[$b]) as $ip
      | (if $f.owners[$b] then
           {action: "accept", src: ["\($f.owners[$b])@"], dst: ["\($ip):443"]}
         else empty end),
        (if $f.owners[$b] and ([$f.given[] | select(.box == $b and .svc == "dd-games")] | length > 0) then
           {action: "accept", src: ["autogroup:member", "tag:computer"], dst: ($ports | map("\($ip):\(.)"))}
         else empty end) ]' "$facts" >"$facts.rules"

jq --argjson names "$NAMES" --arg own "$own" --arg base "$BASE" '
  . as $f
  | $names | map(
      . as $n
      | ($n | rtrimstr("." + $base)) as $h
      # a member'"'"'s own instance is no one else'"'"'s to find at the fleet'"'"'s name
      | ([$f.given[] | select(.svc == $f.follow[$h] and (.own | not)) | .box] | unique) as $on
      | {name: $n, type: "A",
         value: (if ($on | length) > 0 and all($on[]; $f.addr[.] != null) then $f.addr[$on[0]] else $own end)})
  ' "$facts" >"$facts.names"

policy=$(jq --slurpfile rules "$facts.rules" '.acls += $rules[0]' "$BASE_POLICY") || exit 0
if [ "$(jq -cS . "$POLICY" 2>/dev/null)" != "$(jq -cS . <<<"$policy")" ]; then
  printf '%s\n' "$policy" >"$POLICY.tmp"
  chown headscale:headscale "$POLICY.tmp"
  mv "$POLICY.tmp" "$POLICY"
  systemctl kill -s HUP --kill-who=main headscale.service || true
  echo "the network's rules: $(jq length "$facts.rules") from the plan"
fi
if [ "$(jq -cS . "$STATE/extra-records.json" 2>/dev/null)" != "$(jq -cS . "$facts.names")" ]; then
  # before this box has named itself (dd.net.afterJoin), that is its to write
  if [ "$(jq length "$STATE/extra-records.json" 2>/dev/null || echo 0)" != 0 ]; then
    cp "$facts.names" "$STATE/extra-records.json.tmp"
    chown headscale:headscale "$STATE/extra-records.json.tmp"
    mv "$STATE/extra-records.json.tmp" "$STATE/extra-records.json"
    echo "the names: as the plan gives them"
  fi
fi
exit 0
