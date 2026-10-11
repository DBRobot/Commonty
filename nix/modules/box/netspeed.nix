# How fast this box's line is, measured once a day against Cloudflare's
# speed endpoints and carried in its heartbeat (dd-control reads
# /var/lib/dd-facts/net.prom): what placement weighs a service's traffic
# against (nix/lib/capability.nix, the control plane). A metered line is
# never measured - a test would spend someone's allowance - only said to
# be metered.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  measure = pkgs.writeShellScript "dd-netspeed" ''
    set -u
    out=/var/lib/dd-facts/net.prom
    if ${lib.boolToString config.dd.box.metered}; then
      printf 'dd_net_metered 1\n' > "$out.new" && mv "$out.new" "$out"
      exit 0
    fi
    curl=${pkgs.curl}/bin/curl
    # bytes per second, as curl measured them; 0 when the test did not run
    down=$($curl -sS -m 120 -o /dev/null -w '%{speed_download}' 'https://speed.cloudflare.com/__down?bytes=25000000' || echo 0)
    up=$(head -c 10000000 /dev/zero | $curl -sS -m 120 -o /dev/null -w '%{speed_upload}' --data-binary @- 'https://speed.cloudflare.com/__up' || echo 0)
    rtt=$($curl -sS -m 20 -o /dev/null -w '%{time_connect}' 'https://speed.cloudflare.com/__down?bytes=0' || echo 0)
    # not answering is next time: the last measurement stands
    [ "''${down%.*}" -gt 0 ] || exit 0
    ${pkgs.gawk}/bin/awk -v d="$down" -v u="$up" -v r="$rtt" 'BEGIN {
      printf "dd_net_metered 0\ndd_net_down_mbps %.1f\ndd_net_up_mbps %.1f\ndd_net_rtt_ms %.1f\n", d*8/1e6, u*8/1e6, r*1000
    }' > "$out.new" && mv "$out.new" "$out"
  '';
in
{
  systemd.tmpfiles.rules = [ "d /var/lib/dd-facts 0755 root root -" ];
  systemd.services.dd-netspeed = {
    description = "Measure this box's line, for placement";
    # never run by a switch: a failure there rolls the release back
    restartIfChanged = false;
    after = [ "network-online.target" ];
    wants = [ "network-online.target" ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = measure;
      Nice = 10;
    };
  };
  systemd.timers.dd-netspeed = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnActiveSec = "10min";
      OnUnitActiveSec = "1d";
      RandomizedDelaySec = "1h";
    };
  };
}
