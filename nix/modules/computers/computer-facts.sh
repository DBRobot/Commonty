#!/usr/bin/env bash
# How big a computer this box can take, for placement (docs/dev/remote-computers.md):
# its memory less what it keeps for itself and its services, its threads
# less one, the room where the disks go. And what each computer here
# holds, for a move's cost. All zeros without KVM, or while the box drains:
# nothing new is placed here.
set -euo pipefail

out=/var/lib/dd-facts/computers.prom
mkdir -p /var/lib/dd-facts
mem=0 threads=0 disk=0
# a box being drained (its owner's word, or leaving the fleet) takes nothing new
draining=no
if grep -qx 'dd_box_draining 1' /var/lib/dd-facts/draining.prom 2>/dev/null; then
  draining=yes
fi
if [ -e /dev/kvm ] && [ "$draining" = no ]; then
  total=$(awk '/^MemTotal:/ {print $2 * 1024}' /proc/meminfo)
  mem=$((total - RESERVE_BYTES))
  [ "$mem" -gt 0 ] || mem=0
  threads=$(($(nproc) - 1))
  [ "$threads" -gt 0 ] || threads=0
  # where the computers' directories are made (DIR, once there is one)
  at=$DIR
  [ -d "$at" ] || at=/var/lib
  disk=$(df -B1 --output=avail "$at" | tail -1 | tr -d ' ')
fi
tmp=$(mktemp "$out.XXXXXX")
trap 'rm -f "$tmp"' EXIT
{
  echo "dd_computer_max_mem_bytes $mem"
  echo "dd_computer_max_threads $threads"
  echo "dd_computer_max_disk_bytes $disk"
  # each computer's own directory, never through a link (du -P, the default)
  if [ -d "$DIR" ]; then
    find -P "$DIR" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | while read -r id; do
      [[ $id =~ ^[0-9a-f]{16}$ ]] || continue
      echo "dd_computer_used_bytes{computer=\"$id\"} $(du -sxPB1 "$DIR/$id" | cut -f1)"
    done
  fi
} >"$tmp"
chmod 0644 "$tmp"
mv "$tmp" "$out"
trap - EXIT
