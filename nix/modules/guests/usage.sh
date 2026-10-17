#!/usr/bin/env bash
# What each member's guests keep on this box, as its disks hold it (not what
# they may grow to): every instance's directory by its record's owner, and
# for games the worlds each member kept. Into this box's facts, so the
# heartbeat carries it and the ledger adds it to their allowance
# (storage/view.nix). Run as root: the directories are the guests' users'.
set -euo pipefail

out=/var/lib/dd-facts/usage-$KIND.facts
declare -A used=()

for rec in "$DIR"/records/*.json; do
  [ -e "$rec" ] || continue
  owner=$(jq -r '.owner // empty' "$rec")
  [[ $owner =~ ^[A-Za-z0-9._-]+$ ]] || continue
  d="$DIR/instances/$(basename "$rec" .json)"
  [ -d "$d" ] || continue
  # what the disk holds: a sparse disk counts what is written, -x: the
  # guest's share never leads off this filesystem
  n=$(du -sxB1 "$d" | cut -f1)
  used[$owner]=$((${used[$owner]:-0} + n))
done
if [ -d "$DIR/worlds" ]; then
  for w in "$DIR"/worlds/*/; do
    [ -d "$w" ] || continue
    owner=$(basename "$w")
    [[ $owner =~ ^[A-Za-z0-9._-]+$ ]] || continue
    n=$(du -sxB1 "$w" | cut -f1)
    used[$owner]=$((${used[$owner]:-0} + n))
  done
fi

: >"$out.tmp"
for m in "${!used[@]}"; do
  printf 'usage/%s/%s %s\n' "$KIND" "$m" "${used[$m]}" >>"$out.tmp"
done
chmod 0644 "$out.tmp"
mv "$out.tmp" "$out"
echo "usage-$KIND: ${#used[@]} member(s) counted"
