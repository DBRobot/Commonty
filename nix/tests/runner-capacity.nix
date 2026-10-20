# The runner's capacity (forge/runner-capacity.sh) on the boxes as they
# were measured on 2026-10-20, and on a bigger one: node1 two jobs, node2
# one (it ran out of memory four times in a week at two), a fresh box
# by its needs until it has measured, and never none.
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
    run() { # threads memKiB facts fallback members expected
      printf 'MemTotal: %s kB\n' "$2" > meminfo
      THREADS=$1 MEMINFO=meminfo FACTS=$3 FALLBACK_GIB=$4 MEMBERS_GIB=$5 JOB_THREADS=4 JOB_GIB=6 BUILD_GIB=6 \
        IN=in.yaml OUT=$PWD/out/config.yaml bash ${../modules/forge/runner-capacity.sh}
      grep -qx "  capacity: $6" out/config.yaml || { echo "expected $6"; cat out/config.yaml; exit 1; }
    }
    run 8 65488000 node1.prom 37 8 2
    run 8 15588000 node2.prom 2 8 1
    run 8 65488000 none 37 8 1
    run 2 4000000 none 1 0 1
    run 32 131072000 none 4 8 8
    touch $out
  ''
