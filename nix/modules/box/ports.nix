# Every fixed port a service on this box listens on, by service. Two
# services claiming one port fail the build (in CI), not the box's switch -
# release 172 rolled back on node1 because dd-control took the transcoder's
# 4190. Services from nixpkgs are listed here when this box has them; our
# own modules say theirs. An hourly check compares the list with what is
# actually listening, so a service that never said is found (dd-ports-check).
# Ports from 32768 up are the kernel's to hand out and are not listed.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  has = name: config.systemd.services ? ${name};
  declared = config.dd.ports;
  owners = lib.foldlAttrs (
    acc: svc: ports:
    lib.foldl' (a: p: a // { ${toString p} = (a.${toString p} or [ ]) ++ [ svc ]; }) acc ports
  ) { } declared;
  clashes = lib.filterAttrs (_: svcs: lib.length svcs > 1) owners;
  allowed = lib.sort lib.lessThan (lib.unique (lib.flatten (lib.attrValues declared)));
in
{
  options.dd.ports = lib.mkOption {
    type = lib.types.attrsOf (lib.types.listOf lib.types.port);
    default = { };
    description = "service -> the fixed ports it listens on (tcp or udp), below 32768";
  };

  config = {
    assertions = [
      {
        assertion = clashes == { };
        message =
          "two services on ${config.networking.hostName} claim one port: "
          + lib.concatStringsSep "; " (
            lib.mapAttrsToList (p: svcs: "${p} by ${lib.concatStringsSep " and " svcs}") clashes
          );
      }
    ];

    # what nixpkgs runs here, when it does
    dd.ports = lib.mkMerge [
      (lib.mkIf (has "sshd") { ssh = [ 22 ]; })
      (lib.mkIf (has "nginx") {
        nginx = [
          80
          443
        ];
      })
      (lib.mkIf (has "postgresql") { postgresql = [ config.services.postgresql.settings.port ]; })
      (lib.mkIf (has "garage") {
        garage = [
          3900
          3901
          3903
        ];
      })
      (lib.mkIf (has "prometheus") { prometheus = [ config.services.prometheus.port ]; })
      (lib.mkIf (has "prometheus-node-exporter") {
        node-exporter = [ config.services.prometheus.exporters.node.port ];
      })
      (lib.mkIf (has "prometheus-smartctl-exporter") {
        smartctl-exporter = [ config.services.prometheus.exporters.smartctl.port ];
      })
      (lib.mkIf (has "headscale") { headscale = [ config.services.headscale.port ]; })
      (lib.mkIf (has "unbound") { unbound = [ 5335 ]; })
      (lib.mkIf (has "searx") { searxng = [ 4196 ]; })
      (lib.mkIf (has "vaultwarden") { vaultwarden = [ config.services.vaultwarden.config.ROCKET_PORT ]; })
      (lib.mkIf (has "pihole-ftl") {
        pihole = [
          53
          8053
        ];
      })
      (lib.mkIf (has "thanos-sidecar") {
        thanos-sidecar = [
          10901
          10902
        ];
      })
      (lib.mkIf (has "thanos-store") {
        thanos-store = [
          10905
          10906
        ];
      })
      (lib.mkIf (has "thanos-query") {
        thanos-query = [
          10907
          10903
        ];
      })
      (lib.mkIf (has "thanos-compact") { thanos-compact = [ 10908 ]; })
      # its metrics, which nothing reads yet
      (lib.mkIf (lib.any (lib.hasPrefix "cloudflared-tunnel-") (lib.attrNames config.systemd.services)) {
        cloudflared = [ 20241 ];
      })
      # ours (box/*): each where its module runs it
      (lib.mkIf (has "dd-verify") { verify = [ config.dd.verify.port ]; })
      (lib.mkIf (has "dd-transcode") {
        transcode = [
          4190
          4191
        ];
      })
      # the pool worlds are handed ports from, and every port a game insists
      # on (data/games/catalogue.json fixed_ports): no service of ours may
      # take one, or that game could never start here
      (lib.mkIf (has "dd-games") {
        games =
          lib.range config.dd.games.portBase (config.dd.games.portBase + config.dd.games.portCount - 1)
          ++ lib.unique (
            lib.concatMap (
              g: lib.attrValues (g.fixed_ports or { })
            ) (lib.importJSON ../../../data/games/catalogue.json).games
          );
      })
      (lib.mkIf (has "dd-control") { control = [ 4195 ]; })
      (lib.mkIf (has "dd-ci-cancel") { ci-cancel = [ 3003 ]; })
      (lib.mkIf (has "ente") {
        museum = [
          8080
          2112
        ];
      })
      # the public listener nginx keeps for the tunnel (gate/public.nix)
      (lib.mkIf (has "nginx" && config.dd ? public && config.dd.public.enable) { tunnel = [ 8443 ]; })
    ];

    systemd.services.dd-ports-check = {
      description = "Every listening port is one a service declared";
      path = [
        pkgs.iproute2
        pkgs.gawk
      ];
      serviceConfig.Type = "oneshot";
      script = ''
        allowed=" ${lib.concatMapStringsSep " " toString allowed} "
        stray=$(ss -Hltnu | awk '{n=split($5,a,":"); print a[n]}' | sort -nu | while read -r p; do
          # (an if, not &&: a high last port would end the loop non-zero, and
          # set -e would quit here with nothing said)
          if [ "$p" -lt 32768 ]; then case "$allowed" in *" $p "*) ;; *) echo "$p" ;; esac; fi
        done)
        if [ -n "$stray" ]; then
          for p in $stray; do echo "port $p: $(ss -Hltnup "sport = :$p" | grep -o 'users:(("[^"]*' | head -1 | cut -d'"' -f2) listens, and no service declared it (dd.ports)"; done
          exit 1
        fi
      '';
    };
    systemd.timers.dd-ports-check = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "15min";
        OnUnitActiveSec = "1h";
      };
    };
  };
}
