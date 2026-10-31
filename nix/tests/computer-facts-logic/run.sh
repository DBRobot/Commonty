# shellcheck shell=bash
# computers/computer-facts.sh's memory room on a made-up box: 64 GiB, the
# needs' sizes keeping 48 of it, the services measured at 10.
# Args: computer-facts.sh. Needs coreutils, gawk, gnugrep, findutils on PATH.
set -eu
facts_sh=$1
t=$(mktemp -d)
trap 'rm -rf "$t"' EXIT
G=$((1 << 30))
echo "MemTotal: $((64 * G / 1024)) kB" >"$t/meminfo"
touch "$t/kvm"
mkdir -p "$t/dir/0123456789abcdef"
measured() {
  {
    echo "dd_service_memory_bytes{service=\"forgejo\"} $((6 * G))"
    echo "dd_service_memory_bytes{service=\"garage\"} $((4 * G))"
  } >"$t/services.prom"
}
# room: run it, print dd_computer_max_mem_bytes in GiB (decimals kept)
room() {
  rm -rf "$t/facts"
  FACTS=$t/facts MEMINFO=$t/meminfo KVM=$t/kvm DIR=$t/dir \
    RESERVE_BYTES=${RESERVE:-$((48 * G))} MEASURED=${MEASURED-$t/services.prom} \
    BASE_BYTES=$((2 * G)) HEADROOM=1.500000 STALE_SECONDS=1800 bash "$facts_sh"
  awk -v g="$G" '$1 == "dd_computer_max_mem_bytes" { print $2 / g }' "$t/facts/computers.prom"
}
is() {
  if [ "$2" != "$3" ]; then
    echo "FAIL $1: got $2 GiB, want $3" >&2
    exit 1
  fi
  echo "ok $1: $2 GiB"
}

# measured: 64 - 2 - 1.5 * 10 = 47, more than the needs' 16
measured
is "measured over the needs' sizes" "$(room)" 47

# the computers' own units are the room, never kept from it
echo "dd_service_memory_bytes{service=\"computers\"} $((20 * G))" >>"$t/services.prom"
is "running computers not charged as services" "$(room)" 47
grep -q 'dd_computer_used_bytes{computer="0123456789abcdef"} ' "$t/facts/computers.prom"

# no measure, or a stale one, or one with no service in it: the needs' sizes
rm "$t/services.prom"
is "no measure" "$(room)" 16
measured
touch -d '31 minutes ago' "$t/services.prom"
is "stale measure" "$(room)" 16
measured
touch -d '20 minutes ago' "$t/services.prom"
is "recent measure" "$(room)" 47
: >"$t/services.prom"
is "empty measure" "$(room)" 16
echo "dd_service_memory_bytes{service=\"computers\"} $G" >"$t/services.prom"
is "only computers measured" "$(room)" 16

# the owner's fixed reserve: the measure is not read
measured
is "fixed reserve" "$(MEASURED='' RESERVE=$((8 * G)) room)" 56

# never below nothing
echo "dd_service_memory_bytes{service=\"forgejo\"} $((60 * G))" >"$t/services.prom"
is "measured past the box" "$(room)" 0
rm "$t/services.prom"
is "needs past the box" "$(RESERVE=$((80 * G)) room)" 0

# draining: nothing new, measure or not
measured
rm -rf "$t/facts" && mkdir -p "$t/facts"
echo 'dd_box_draining 1' >"$t/facts/draining.prom"
FACTS=$t/facts MEMINFO=$t/meminfo KVM=$t/kvm DIR=$t/dir RESERVE_BYTES=$((48 * G)) \
  MEASURED=$t/services.prom BASE_BYTES=$((2 * G)) HEADROOM=1.5 STALE_SECONDS=1800 bash "$facts_sh"
grep -qx 'dd_computer_max_mem_bytes 0' "$t/facts/computers.prom"
echo "ok draining: 0"
