#!/usr/bin/env bash
# What each game really takes: every running world's memory peak, as
# systemd counts it for its unit (the guest and its qemu together), kept
# per game at its highest in measured.json. Nothing sizes from it: a guest's
# kernel fills the memory it is given with its own cache, and the host's
# cache of the share's files is charged to the same unit, so a peak is what
# the world was given more than what the game needs. The catalogue's
# memory_mib (data/games/resolve.py) is what is given.
set -euo pipefail
out="$DIR/measured.json"
[ -s "$out" ] || echo '{}' >"$out"
for unit in $(systemctl list-units --no-legend --state=running 'dd-game@*.service' | awk '{print $1}'); do
  id=${unit#dd-game@}
  id=${id%.service}
  game=$(jq -r '.game // empty' "$DIR/records/$id.json" 2>/dev/null) || continue
  [ -n "$game" ] || continue
  peak=$(systemctl show "$unit" -p MemoryPeak --value)
  case "$peak" in '' | *[!0-9]*) continue ;; esac
  jq --arg g "$game" --argjson p "$peak" \
    '.[$g] = ([(.[$g] // 0), $p] | max)' "$out" >"$out.new"
  mv "$out.new" "$out"
done
