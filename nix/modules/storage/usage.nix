# What each member keeps in a service, counted by the service itself: a
# query it runs as its own database user against its own database, every
# hour, into /var/lib/dd-usage/<service>/usage.tsv ("<member>\t<bytes>").
# Each count also goes into this box's facts (/var/lib/dd-facts), so its
# heartbeat carries it and every box's gate adds the fleet's up
# (storage/view.nix): no service's login reaches another's tables, and no
# box reads another.
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
            ${pkgs.gawk}/bin/awk -F '\t' -v svc=${u.counts} -v suffix=${lib.escapeShellArg u.memberSuffix} '
              { m = $1; if (suffix != "" && substr(m, length(m) - length(suffix) + 1) == suffix) m = substr(m, 1, length(m) - length(suffix)) }
              m ~ /^[A-Za-z0-9._-]+$/ && $2 ~ /^[0-9]+$/ { printf "dd_usage{service=\"%s\",member=\"%s\"} %s\n", svc, m, $2 }
            ' /var/lib/dd-usage/${name}/usage.tsv > /var/lib/dd-facts/usage-${u.counts}.prom.tmp
            chmod 0644 /var/lib/dd-facts/usage-${u.counts}.prom.tmp
            mv /var/lib/dd-facts/usage-${u.counts}.prom.tmp /var/lib/dd-facts/usage-${u.counts}.prom
          ''}";
        };
        script = ''
          out=/var/lib/dd-usage/${name}/usage.tsv
          psql -X -tA -F "$(printf '\t')" -v ON_ERROR_STOP=1 -d ${u.database} -c ${lib.escapeShellArg u.query} > "$out.new"
          mv "$out.new" "$out"
        '';
      }
    ) cfg;
    systemd.timers = lib.mapAttrs' (
      name: _:
      lib.nameValuePair "dd-usage-${name}" {
        wantedBy = [ "timers.target" ];
        timerConfig = {
          OnBootSec = "90s";
          OnUnitActiveSec = "1h";
        };
      }
    ) cfg;
  };
}
