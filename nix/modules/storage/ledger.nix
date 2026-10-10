# The box with Photos counts what each member keeps in Photos and in the
# libraries, into its heartbeat (client/cli/src/storage_ledger.rs), and
# holds each member's Photos to what the rest of their allowance leaves,
# from the fleet's count (storage/view.nix). A unit of its own: it holds a
# Photos admin login, which the gate - the box's front door - never does.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.dd.storage;
  base = config.dd.domain;
  user = "dd-storage";
  dir = "/var/lib/dd-storage";
  dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
in
{
  config = lib.mkIf (cfg.budgetGb != null) {
    sops.secrets.ente-storage-password.owner = user;

    # It once had a database login with grants on the forge's and the
    # vault's tables; it reads their counts now (storage/usage.nix). The
    # old login goes, grants and all.
    systemd.services.dd-storage-forget-login = {
      description = "Drop the storage ledger's old database login";
      wantedBy = [ "multi-user.target" ];
      after = [ "postgresql.service" ];
      path = [ config.services.postgresql.package ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        User = "postgres";
      };
      script = ''
        [ "$(psql -tAc "select 1 from pg_roles where rolname = '${user}'")" = 1 ] || exit 0
        for db in forgejo vaultwarden ente; do
          [ "$(psql -tAc "select 1 from pg_database where datname = '$db'")" = 1 ] || continue
          psql -d "$db" -v ON_ERROR_STOP=1 -q -c 'REVOKE ALL ON ALL TABLES IN SCHEMA public FROM "${user}"' -c 'DROP OWNED BY "${user}"'
        done
        psql -v ON_ERROR_STOP=1 -q -c 'DROP ROLE "${user}"'
        echo "the ledger's database login is gone"
      '';
    };

    systemd.services.dd-storage-ledger = {
      description = "Count what each member keeps, and hold Photos to what is left";
      after = [
        "dd-verify.service"
        "dd-storage-view.service"
      ];
      serviceConfig = {
        Type = "oneshot";
        User = user;
        Group = user;
        StateDirectory = "dd-storage";
        StateDirectoryMode = "0750";
        UMask = "0027";
        NoNewPrivileges = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        PrivateTmp = true;
        ExecStart = lib.escapeShellArgs [
          dd
          "storage-ledger"
          "--ente-origin"
          "https://api.${base}"
          "--photos-suffix"
          "@users.${base}"
          "--password-file"
          config.sops.secrets.ente-storage-password.path
          "--budget-gb"
          (toString cfg.budgetGb)
          "--view"
          "${dir}/ledger.json"
          "--facts-out"
          "${dir}/usage.prom"
        ];
        # into this box's heartbeat: as root, where the facts are
        ExecStartPost = "+${pkgs.coreutils}/bin/install -m 0644 ${dir}/usage.prom /var/lib/dd-facts/usage-ledger.prom";
      };
    };
    systemd.timers.dd-storage-ledger = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "2min";
        OnUnitActiveSec = "1h";
      };
    };

  };
}
