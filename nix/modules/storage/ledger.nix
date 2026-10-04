# One allowance per member across every service (box/verify/src/storage.rs,
# client/cli/src/storage_ledger.rs). A unit of its own counts, every
# hour: it holds a Photos admin login, which the gate - the box's front
# door - never does, and reads no database: each service counts
# its own (storage/usage.nix). The gate reads
# what it writes, to show it and to hold library uploads to it.
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
  options.dd.storage.budgetGb = lib.mkOption {
    type = lib.types.nullOr lib.types.int;
    default = null;
    description = "each member's allowance across every service, in GB; null: no ledger and no allowance";
  };

  config = lib.mkIf (cfg.budgetGb != null) {
    users.users.${user} = {
      isSystemUser = true;
      group = user;
      # the services' own counts (storage/usage.nix)
      extraGroups = [ "dd-usage" ];
    };
    users.groups.${user} = { };
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
        "dd-usage-forgejo.service"
        "dd-usage-vaultwarden.service"
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
          "--vault-suffix"
          "@${base}"
          "--password-file"
          config.sops.secrets.ente-storage-password.path
          "--usage"
          "code=/var/lib/dd-usage/forgejo/usage.tsv"
          "--usage"
          "passwords=/var/lib/dd-usage/vaultwarden/usage.tsv"
          "--budget-gb"
          (toString cfg.budgetGb)
          "--out"
          "${dir}/ledger.json"
        ];
      };
    };
    systemd.timers.dd-storage-ledger = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "2min";
        OnUnitActiveSec = "1h";
      };
    };

    # the gate reads the count: the page, and uploads held to it
    users.users.dd-verify.extraGroups = [ user ];
    systemd.services.dd-verify.environment.VERIFY_STORAGE_LEDGER = "${dir}/ledger.json";
  };
}
