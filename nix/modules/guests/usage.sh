#!/usr/bin/env bash
# What each member's guests keep on this box, as its disks hold it (not what
# they may grow to): every instance's directory by its record's owner, and
# for games the worlds each member kept. Into this box's facts, so the
# heartbeat carries it and the ledger adds it to their allowance
# (storage/view.nix). Run as root: the directories are the guests' users'.
set -euo pipefail

out=/var/lib/dd-facts/usage-$KIND.facts
declare -A used=()

# a file as it is, never through a link: root reads what a guest's own
# user may have written
read_nofollow() { dd if="$1" iflag=nofollow bs=64k count=16 status=none 2>/dev/null; }
# each instance: whose it is, then its directory. Games keep records
# beside (records/<id>.json, instances/<id>), written by their manager; a
# computer's owner is root's note of the agreed one (OWNERS/<id>)
instances() {
  if [ -d "$DIR/records" ]; then
    find -P "$DIR/records" -mindepth 1 -maxdepth 1 -type f -name '*.json' -printf '%f\n' |
      while read -r f; do echo "$DIR/records/$f $DIR/instances/${f%.json}"; done
  elif [ -d "$DIR" ]; then
    # whose a computer is: root's own note of the agreed owner (OWNERS),
    # never anything in its directory, which its own user may write
    find -P "$DIR" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' |
      while read -r id; do echo "$OWNERS/$id $DIR/$id"; done
  fi
}
while read -r rec d; do
  if [ -n "${OWNERS:-}" ]; then
    owner=$(read_nofollow "$rec" | head -1)
  else
    owner=$(read_nofollow "$rec" | jq -r '.owner // empty' 2>/dev/null) || continue
  fi
  [[ $owner =~ ^[A-Za-z0-9._-]+$ ]] || continue
  [ -d "$d" ] && [ ! -L "$d" ] || continue
  # what the disk holds: a sparse disk counts what is written; -x: nothing
  # leads off this filesystem, -P: no link is followed
  n=$(du -sxPB1 "$d" | cut -f1)
  used[$owner]=$((${used[$owner]:-0} + n))
done < <(instances)
if [ -d "$DIR/worlds" ]; then
  while read -r owner; do
    [[ $owner =~ ^[A-Za-z0-9._-]+$ ]] || continue
    n=$(du -sxPB1 "$DIR/worlds/$owner" | cut -f1)
    used[$owner]=$((${used[$owner]:-0} + n))
  done < <(find -P "$DIR/worlds" -mindepth 1 -maxdepth 1 -type d -printf '%f\n')
fi

: >"$out.tmp"
for m in "${!used[@]}"; do
  printf 'usage/%s/%s %s\n' "$KIND" "$m" "${used[$m]}" >>"$out.tmp"
done
chmod 0644 "$out.tmp"
mv "$out.tmp" "$out"
echo "usage-$KIND: ${#used[@]} member(s) counted"
