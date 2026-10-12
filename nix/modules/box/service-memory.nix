# What each service on this box holds in memory, as systemd counts it for
# its units (the needs' units, nix/lib/needs.nix): the most it has held
# since it started, every 5 minutes, into /var/lib/dd-facts/services.prom.
# dd-control carries it in the heartbeat; placement plans with it in place
# of the needs' estimate (servers/system/control/place).
{
  lib,
  pkgs,
  ...
}:
let
  needs = import ../../lib/needs.nix { inherit lib; };
  # "service claim" per line; a claim ending in * is a prefix
  claims = pkgs.writeText "service-claims" (
    lib.concatMapStrings (s: lib.concatMapStrings (c: "${s} ${c}\n") ([ s ] ++ needs.${s}.units)) (
      builtins.attrNames needs
    )
  );
  measure = pkgs.writeShellScript "dd-service-memory" ''
    set -u
    export PATH=${
      lib.makeBinPath [
        pkgs.systemd
        pkgs.gawk
        pkgs.coreutils
      ]
    }
    out=/var/lib/dd-facts/services.prom
    units=$(systemctl list-units --type=service --state=active --no-legend --plain | awk '{print $1}')
    [ -n "$units" ] || exit 0
    # a block per unit, its properties in systemd's order, a blank line after
    systemctl show -p Id,MemoryPeak,MemoryCurrent $units | awk -v claims=${claims} '
      BEGIN { while ((getline l < claims) > 0) { split(l, f, " "); n++; svc[n] = f[1]; pat[n] = f[2] } }
      function flush(   u, i, p, m) {
        if (id == "") return
        u = id; sub(/\.service$/, "", u)
        m = (peak ~ /^[0-9]+$/) ? peak : cur
        if (m ~ /^[0-9]+$/ && m > 0)
          for (i = 1; i <= n; i++) {
            p = pat[i]
            if (p ~ /\*$/ ? index(u, substr(p, 1, length(p) - 1)) == 1 : u == p) { mem[svc[i]] += m; break }
          }
        id = peak = cur = ""
      }
      /^$/ { flush() }
      /^Id=/ { id = substr($0, 4) }
      /^MemoryPeak=/ { peak = substr($0, 12) }
      /^MemoryCurrent=/ { cur = substr($0, 15) }
      END { flush(); for (s in mem) printf "dd_service_memory_bytes{service=\"%s\"} %d\n", s, mem[s] }
    ' | sort > "$out.new" && mv "$out.new" "$out"
  '';
in
{
  # /var/lib/dd-facts is node-exporter's (observe/metrics.nix): a second
  # rule naming another owner took it from the units that write there as
  # node-exporter, and both boxes rolled release 249 back
  systemd.services.dd-service-memory = {
    description = "Measure what each service holds in memory, for placement";
    restartIfChanged = false;
    serviceConfig = {
      Type = "oneshot";
      ExecStart = measure;
      Nice = 10;
    };
  };
  systemd.timers.dd-service-memory = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnActiveSec = "3min";
      OnUnitActiveSec = "5min";
    };
  };
}
