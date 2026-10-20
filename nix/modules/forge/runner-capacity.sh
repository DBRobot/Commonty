# shellcheck shell=bash
# How many CI jobs this box takes at once, from the box as it is when the
# runner starts: one per JOB_THREADS threads, and one per JOB_GIB of the
# memory left once the rest of the box has what it holds. The rest is what
# its services held at their peak (dd-service-memory's services.prom; the
# needs' sizes until that is written), a release build (dd-build), the
# members' runner, and a tenth of the box kept free. Never below one.
# Writes the runner's config with that capacity to $OUT.
#
# In: JOB_THREADS JOB_GIB BUILD_GIB MEMBERS_GIB FALLBACK_GIB IN OUT
#     FACTS (services.prom) MEMINFO (/proc/meminfo) THREADS (nproc)
set -u
threads=${THREADS:-$(nproc)}
mem_kb=$(awk '/^MemTotal:/ {print $2}' "${MEMINFO:-/proc/meminfo}")
if [ -s "$FACTS" ]; then
  # this runner's own jobs are what is being sized; anything else counts
  services=$(awk '/^dd_service_memory_bytes/ && !/service="forgejo-runner"/ {s += $NF} END {printf "%.1f", s / 2^30}' "$FACTS")
  from="measured"
else
  services=$FALLBACK_GIB
  from="the needs' sizes"
fi
line=$(awk -v t="$threads" -v kb="$mem_kb" -v svc="$services" -v build="$BUILD_GIB" \
  -v members="$MEMBERS_GIB" -v jt="$JOB_THREADS" -v jg="$JOB_GIB" -v from="$from" 'BEGIN {
  mem = kb / 2^20
  keep = mem / 10; if (keep < 2) keep = 2
  room = mem - svc - build - members - keep
  bymem = int(room / jg); if (bymem < 0) bymem = 0
  bythreads = int(t / jt)
  c = bymem < bythreads ? bymem : bythreads; if (c < 1) c = 1
  printf "%d %d threads -> %d; %.1fG - %.1fG services (%s) - %dG dd-build - %dG members - %.1fG kept free = %.1fG -> %d\n", \
    c, t, bythreads, mem, svc, from, build, members, keep, room, bymem
}')
capacity=${line%% *}
echo "runner capacity $capacity: ${line#* }"
mkdir -p "$(dirname "$OUT")"
sed "s/^  capacity: 0$/  capacity: $capacity/" "$IN" > "$OUT.new"
grep -q "^  capacity: $capacity$" "$OUT.new" || { echo "no capacity line in $IN" >&2; exit 1; }
mv "$OUT.new" "$OUT"
