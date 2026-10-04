# The control plane on this box (box/control, docs/control-plane.md):
# dd-control heartbeats what the box has and runs, signed by its TPM, and
# keeps the agreed state at http://127.0.0.1:4195/state, checked against the
# validators' signatures. A box named as its house's validator in
# fleet/control.json also votes. It dials out to the relay at
# control.<domain> and nothing dials in. Nothing acts on the state yet.
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  cfg = config.dd.control;
  pkg = self.packages.${pkgs.stdenv.hostPlatform.system};
  roles = lib.concatStringsSep "," cfg.roles;
in
{
  options.dd.control = {
    enable = lib.mkEnableOption "the control plane's heartbeat and agreed state on this box";
    relay = lib.mkOption {
      type = lib.types.str;
      default = "wss://control.${config.dd.domain}/relay";
    };
    roles = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      description = "the roles this box runs, as its heartbeat reports them";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ pkg.dd-control ];
    environment.etc."dd-control/config.json".source = pkg.control-config;
    systemd.services.dd-control = {
      description = "The control plane: heartbeats and the agreed state";
      wantedBy = [ "multi-user.target" ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      path = [ pkgs.tpm2-tools ];
      environment.TPM2TOOLS_TCTI = "device:/dev/tpmrm0";
      serviceConfig = {
        ExecStart = lib.concatStringsSep " " [
          "${pkg.dd-control}/bin/dd-control"
          "-config /etc/dd-control/config.json"
          "-box ${config.networking.hostName}"
          "-relay ${cfg.relay}"
          "-state /var/lib/dd-control"
          "-tpm-keys /boot/dd/unlock"
          "-listen 127.0.0.1:4195"
          (lib.optionalString (roles != "") "-roles ${roles}")
        ];
        StateDirectory = "dd-control";
        StateDirectoryMode = "0700";
        Restart = "always";
        RestartSec = 10;
        # root for the TPM and the key files on /boot; nothing else is written
        ProtectSystem = "strict";
        ReadWritePaths = [ "/var/lib/dd-control" ];
        ProtectHome = true;
        PrivateTmp = true;
        NoNewPrivileges = true;
      };
    };
  };
}
