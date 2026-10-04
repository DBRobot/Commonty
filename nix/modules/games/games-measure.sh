#!/usr/bin/env bash
# What each game really takes: every running world's memory peak, as
# systemd counts it for its unit (the guest and its qemu together), kept
# per game at its highest in measured.json. Nothing publishes a dedicated
# server's memory; this is where nix/modules/games/needs.nix's figures will
# come from (they are the class defaults until a game has run).
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
