# shellcheck shell=bash
# How many CI jobs this box takes at once, from the box as it is when the
# runner starts: one per JOB_THREADS threads, and one per JOB_GIB of the
# room the jobs have. That room is the work slice's limit (builds and CI
# share it, box/work-memory.nix), or less where the box's services, at
# their measured peak (dd-service-memory's services.prom; the needs' sizes
# until that is written), and a tenth of the box kept free leave less.
# A release build is inside the slice, so it is not counted again: a job
# past the slice's limit is killed alone (OOMPolicy=continue). Never below one.
# Writes the runner's config with that capacity to $OUT.
#
# In: JOB_THREADS JOB_GIB FALLBACK_GIB IN OUT FACTS (services.prom)
#     WORK_MAX (work.slice's memory.max) MEMINFO (/proc/meminfo) THREADS (nproc)
set -u
threads=${THREADS:-$(nproc)}
mem_kb=$(awk '/^MemTotal:/ {print $2}' "${MEMINFO:-/proc/meminfo}")
# "max" or missing: no slice, the box's memory alone bounds the jobs
work=$(cat "${WORK_MAX:-/sys/fs/cgroup/work.slice/memory.max}" 2>/dev/null || echo max)
if [ -s "$FACTS" ]; then
  # this runner's own jobs are what is being sized; anything else counts
  services=$(awk '/^dd_service_memory_bytes/ && !/service="forgejo-runner"/ {s += $NF} END {printf "%.1f", s / 2^30}' "$FACTS")
  from="measured"
else
  services=$FALLBACK_GIB
  from="the needs' sizes"
fi
line=$(awk -v t="$threads" -v kb="$mem_kb" -v svc="$services" -v work="$work" \
  -v jt="$JOB_THREADS" -v jg="$JOB_GIB" -v from="$from" 'BEGIN {
  mem = kb / 2^20
  keep = mem / 10; if (keep < 2) keep = 2
  left = mem - svc - keep
  slice = work ~ /^[0-9]+$/ ? work / 2^30 : mem
  room = slice < left ? slice : left
  bymem = int(room / jg); if (bymem < 0) bymem = 0
  bythreads = int(t / jt)
  c = bymem < bythreads ? bymem : bythreads; if (c < 1) c = 1
  printf "%d %d threads -> %d; min(%.1fG work slice, %.1fG - %.1fG services (%s) - %.1fG kept free) = %.1fG / %dG a job -> %d\n", \
    c, t, bythreads, slice, mem, svc, from, keep, room, jg, bymem
}')
capacity=${line%% *}
echo "runner capacity $capacity: ${line#* }"
mkdir -p "$(dirname "$OUT")"
sed "s/^  capacity: 0$/  capacity: $capacity/" "$IN" > "$OUT.new"
grep -q "^  capacity: $capacity$" "$OUT.new" || { echo "no capacity line in $IN" >&2; exit 1; }
mv "$OUT.new" "$OUT"
