#!/usr/bin/env bash
# How big a computer this box can take, for placement (docs/dev/instances.md):
# its memory less what it keeps for itself, its threads less one, the room
# where the disks go. And what each computer here holds, for a move's cost.
# Zeros without KVM, or while the box drains: nothing is placed here.
set -euo pipefail

out=/var/lib/dd-facts/vm.prom
mem=0 threads=0 disk=0
# a box being drained (its owner's word, or leaving the fleet) takes nothing new
draining=$(cat /var/lib/dd-facts/*.prom 2>/dev/null | awk '$1 == "dd_box_draining" && $2 == 1' | head -1)
if [ -e /dev/kvm ] && [ -z "$draining" ]; then
  total=$(awk '/^MemTotal:/ {print $2 * 1024}' /proc/meminfo)
  mem=$((total - RESERVE_BYTES))
  [ "$mem" -gt 0 ] || mem=0
  threads=$(($(nproc) - 1))
  [ "$threads" -gt 0 ] || threads=1
  disk=$(df -B1 --output=avail "$DIR" | tail -1 | tr -d ' ')
fi
{
  echo "dd_vm_max_mem_bytes $mem"
  echo "dd_vm_max_threads $threads"
  echo "dd_vm_max_disk_bytes $disk"
  for d in "$DIR"/instances/*/; do
    [ -d "$d" ] || continue
    id=$(basename "$d")
    [[ $id =~ ^[0-9a-f]{16}$ ]] || continue
    echo "dd_vm_used_bytes{vm=\"$id\"} $(du -sxB1 "$d" | cut -f1)"
  done
} >"$out.tmp"
chmod 0644 "$out.tmp"
mv "$out.tmp" "$out"
