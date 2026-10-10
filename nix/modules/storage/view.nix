# The fleet's count of what each member keeps, on every box: every box's
# heartbeat says what its members keep in the services it runs
# (storage/usage.nix, storage/ledger.nix), and this adds them up from the
# control plane's agreed state for this box's gate, which shows it and
# holds uploads to it. Where a service runs, or on how many boxes, does not
# matter; no box reads another.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.dd.storage;
  user = "dd-storage";
  dir = "/var/lib/dd-storage";
  dd = "${self.packages.${pkgs.stdenv.hostPlatform.system}.dd}/bin/dd";
in
{
  options.dd.storage.budgetGb = lib.mkOption {
    type = lib.types.nullOr lib.types.int;
    default = (lib.importJSON ../../../fleet/policy.json).allowanceGb or null;
    description = "each member's allowance across every service and box, in GB (fleet/policy.json allowanceGb); null: no allowance";
  };

  config = lib.mkIf (cfg.budgetGb != null && config.dd.control.enable or false) {
    users.users.${user} = {
      isSystemUser = true;
      group = user;
    };
    users.groups.${user} = { };

    systemd.services.dd-storage-view = {
      description = "Add up what each member keeps, from every box's heartbeat";
      after = [ "dd-control.service" ];
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
          "storage-view"
          "--budget-gb"
          (toString cfg.budgetGb)
          # which box may count what: the release's list, not a box's word
          "--roles"
          (toString (
            pkgs.writeText "box-roles.json" (
              builtins.toJSON (lib.mapAttrs (_: b: b.roles) (lib.importJSON ../../../fleet/boxes.json))
            )
          ))
          "--out"
          "${dir}/ledger.json"
        ];
      };
    };
    systemd.timers.dd-storage-view = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "2min";
        OnUnitActiveSec = "10min";
      };
    };

    # the gate reads the count: the page, and uploads held to it
    users.users.dd-verify.extraGroups = [ user ];
    systemd.services.dd-verify.environment.VERIFY_STORAGE_LEDGER = "${dir}/ledger.json";
  };
}
