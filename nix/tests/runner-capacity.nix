# The runner's capacity (forge/runner-capacity.sh) on the boxes as they
# were measured on 2026-10-22, and on others: node1 two jobs (its threads
# hold it there), node2 two (its work slice holds two 4G jobs, counted
# once), a fresh box by its needs until it has measured, a box with no work
# slice by its memory alone, and never none.
{ pkgs, ... }:
pkgs.runCommand "runner-capacity"
  {
    nativeBuildInputs = [
      pkgs.gawk
      pkgs.gnused
      pkgs.gnugrep
    ];
  }
  ''
    printf 'runner:\n  capacity: 0\n' > in.yaml
    # node1's services: llama-swap, forgejo, garage and the rest; its runner's own jobs are left out
    printf 'dd_service_memory_bytes{service="llama-swap"} 24100573184\ndd_service_memory_bytes{service="forgejo"} 1215758336\ndd_service_memory_bytes{service="garage"} 969256960\ndd_service_memory_bytes{service="rest"} 1900000000\ndd_service_memory_bytes{service="forgejo-runner"} 13007646720\n' > node1.prom
    printf 'dd_service_memory_bytes{service="garage"} 698822656\ndd_service_memory_bytes{service="rest"} 600000000\ndd_service_memory_bytes{service="forgejo-runner"} 3177631744\n' > node2.prom
    run() { # threads memKiB facts fallback workSliceMax expected
      printf 'MemTotal: %s kB\n' "$2" > meminfo
      echo "$5" > work.max
      THREADS=$1 MEMINFO=meminfo FACTS=$3 FALLBACK_GIB=$4 WORK_MAX=work.max JOB_THREADS=4 JOB_GIB=4 \
        IN=in.yaml OUT=$PWD/out/config.yaml bash ${../modules/forge/runner-capacity.sh}
      grep -qx "  capacity: $6" out/config.yaml || { echo "expected $6"; cat out/config.yaml; exit 1; }
    }
    # node1: 30G of room past its services, but 8 threads
    run 8 65488000 node1.prom 37 53646052557 2
    # node2: min(11.9G slice, 14.9G - 1.2G - 2G) = 11.7G
    run 8 15588000 node2.prom 2 12741173248 2
    # node2's memory with twice the threads: still two
    run 16 15588000 node2.prom 2 12741173248 2
    # a fresh 62G box on the needs' sizes
    run 8 65488000 none 37 53646052557 2
    # no work slice: the memory alone
    run 8 15588000 node2.prom 2 max 2
    # services that leave less than a job: never none
    run 8 15588000 none 11 12741173248 1
    run 2 4000000 none 1 max 1
    run 32 131072000 none 4 107374182400 8
    touch $out
  ''
