# What each member keeps in a service, counted by the service itself: a
# query it runs as its own database user against its own database, every
# hour, into /var/lib/dd-usage/<service>/usage.tsv ("<member>\t<bytes>").
# The storage ledger reads those files and no database: no service's login
# reaches another's tables (ledger.nix).
{ config, lib, ... }:
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
