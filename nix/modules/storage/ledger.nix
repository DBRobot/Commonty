# One allowance per member across every service (box/verify/src/storage.rs,
# client/cli/src/storage_ledger.rs). A unit of its own counts, every ten
# minutes: it holds a Photos admin login, which the gate - the box's front
# door - never does, and reads only the columns it adds up. The gate reads
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
    };
    users.groups.${user} = { };
    sops.secrets.ente-storage-password.owner = user;

    # the ledger's database login may read what it adds up and nothing else
    services.postgresql.ensureUsers = [ { name = user; } ];
    systemd.services.dd-storage-grants = {
      description = "Let the storage ledger read repository and attachment sizes";
      wantedBy = [ "multi-user.target" ];
      after = [
        "postgresql.service"
        "forgejo.service"
        "vaultwarden.service"
      ];
      path = [ config.services.postgresql.package ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        User = "postgres";
      };
      script = ''
        # the tables exist once their services have started once
        for _ in $(seq 1 120); do
          [ "$(psql -d forgejo -tAc "select to_regclass('public.repository') is not null")" = t ] \
            && [ "$(psql -d vaultwarden -tAc "select to_regclass('public.attachments') is not null")" = t ] && break
          sleep 1
        done
        psql -d forgejo -v ON_ERROR_STOP=1 -q -c 'GRANT SELECT (owner_name, size, lfs_size) ON repository TO "${user}"'
        psql -d vaultwarden -v ON_ERROR_STOP=1 -q \
          -c 'GRANT SELECT (uuid, email) ON users TO "${user}"' \
          -c 'GRANT SELECT (uuid, user_uuid) ON ciphers TO "${user}"' \
          -c 'GRANT SELECT (cipher_uuid, file_size) ON attachments TO "${user}"'
      '';
    };

    systemd.services.dd-storage-ledger = {
      description = "Count what each member keeps, and hold Photos to what is left";
      after = [
        "dd-verify.service"
        "dd-storage-grants.service"
      ];
      wants = [ "dd-storage-grants.service" ];
      path = [ config.services.postgresql.package ];
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
