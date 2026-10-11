# What each member keeps in a service, counted by the service itself: a
# query it runs as its own database user against its own database, every
# hour, into /var/lib/dd-usage/<service>/usage.tsv ("<member>\t<bytes>").
# Each count also goes into this box's facts (/var/lib/dd-facts), so its
# heartbeat carries it and every box's gate adds the fleet's up
# (storage/view.nix): no service's login reaches another's tables, and no
# box reads another. Never a .prom file: per-member figures stay out of
# Prometheus (checks.usage-facts).
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.usage;
in
{
  options.dd.usage = lib.mkOption {
    type = lib.types.attrsOf (
      lib.types.submodule {
        options = {
          user = lib.mkOption {
            type = lib.types.str;
            description = "the service's own user, whose database login is peer auth";
          };
          database = lib.mkOption { type = lib.types.str; };
          query = lib.mkOption {
            type = lib.types.str;
            description = "rows of member and bytes";
          };
          counts = lib.mkOption {
            type = lib.types.enum [
              "code"
              "passwords"
            ];
            description = "what of a member's allowance it is";
          };
          memberSuffix = lib.mkOption {
            type = lib.types.str;
            default = "";
            description = "what the service adds to a member's name in its rows (an address's @<domain>), taken off";
          };
        };
      }
    );
    default = { };
  };

  config = lib.mkIf (cfg != { }) {
    users.groups.dd-usage = { };
    systemd.services = lib.mapAttrs' (
      name: u:
      lib.nameValuePair "dd-usage-${name}" {
        description = "Count what each member keeps in ${name}";
        after = [
          "postgresql.service"
          "${name}.service"
        ];
        path = [ config.services.postgresql.package ];
        # never run by a switch: a failure there rolls the release back
        restartIfChanged = false;
        serviceConfig = {
          Type = "oneshot";
          User = u.user;
          Group = "dd-usage";
          StateDirectory = "dd-usage/${name}";
          StateDirectoryMode = "0750";
          UMask = "0027";
          NoNewPrivileges = true;
          ProtectSystem = "strict";
          ProtectHome = true;
          PrivateTmp = true;
          # into the heartbeat: as root, where node-exporter's facts are
          ExecStartPost = "+${pkgs.writeShellScript "dd-usage-${name}-facts" ''
            [ -f /var/lib/dd-usage/${name}/usage.tsv ] || exit 0
            out=/var/lib/dd-facts/usage-${u.counts}.facts
            ${pkgs.gawk}/bin/awk -F '\t' -v svc=${u.counts} -v suffix=${lib.escapeShellArg u.memberSuffix} \
              -f ${./usage-facts.awk} /var/lib/dd-usage/${name}/usage.tsv > "$out.tmp"
            chmod 0644 "$out.tmp"
            mv "$out.tmp" "$out"
            # the metric it once was, which node-exporter would still serve
            rm -f /var/lib/dd-facts/usage-${u.counts}.prom
          ''}";
        };
        script = ''
          out=/var/lib/dd-usage/${name}/usage.tsv
          # postgres not answering yet (restarting under a switch) is next
          # time: the last count stands
          if ! psql -X -tA -F "$(printf '\t')" -v ON_ERROR_STOP=1 -d ${u.database} -c ${lib.escapeShellArg u.query} > "$out.new"; then
            echo "the database did not answer; counting next time"
            rm -f "$out.new"
            exit 0
          fi
          mv "$out.new" "$out"
        '';
      }
    ) cfg;
    systemd.timers = lib.mapAttrs' (
      name: _:
      lib.nameValuePair "dd-usage-${name}" {
        wantedBy = [ "timers.target" ];
        timerConfig = {
          # from the timer starting, not boot: a switch that adds it starts
          # it long after boot, and OnBootSec would fire at once
          OnActiveSec = "90s";
          OnUnitActiveSec = "1h";
        };
      }
    ) cfg;
  };
}
