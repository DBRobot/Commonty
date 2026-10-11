# shellcheck shell=bash
# A writer whose lease may have lapsed stops, whether or not dd-control is
# there to see it: each leased service's flag says until when its box may
# run it, in ms since boot (/proc/uptime, suspend included), the clock
# dd-control kept it by (servers/system/control/cmd/dd-control/placed.go).
set -u
dir=/run/dd-placed
carries=/etc/dd-placed/carries.json
[ -d "$dir" ] && [ -r "$carries" ] || exit 0
read -r up _ </proc/uptime
now=$((${up%.*} * 1000 + 10#${up#*.} * 10))
for f in "$dir"/*; do
  [ -s "$f" ] || continue # a stateless service's flag is empty
  case "$f" in *.new) continue ;; esac
  until=$(jq -r '.until // empty' "$f" 2>/dev/null) || continue
  [ -n "$until" ] && [ "$now" -ge "$until" ] || continue
  svc=$(basename "$f")
  mapfile -t units < <(jq -r --arg s "$svc" '.[$s] // [] | .[]' "$carries")
  echo "$svc: its lease may have lapsed; stopping ${units[*]}"
  if [ "${#units[@]}" -eq 0 ] || systemctl stop "${units[@]}"; then
    rm -f "$f"
  fi
done
exit 0
