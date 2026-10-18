# The box installer's screen (docs/dev/joining.md): dd-box-setup, its
# pages on loopback, and the kiosk browser that fills the screen with them.
# The steps it runs are the installer's own: dd-box-backup (backup.sh)
# and dd-join-install (join-install.sh, iso.nix).
{
  config,
  lib,
  pkgs,
  self,
  ...
}:
let
  sys = pkgs.stdenv.hostPlatform.system;
  agent = self.packages.${sys}.agent;
  backup = pkgs.writeShellApplication {
    name = "dd-box-backup";
    runtimeInputs = [
      agent
      pkgs.restic
      pkgs.rclone
      pkgs.ntfs3g
      pkgs.util-linux
      pkgs.coreutils
      pkgs.gawk
      config.boot.zfs.package
    ];
    text = builtins.readFile ./backup.sh;
  };
in
{
  options.dd.setup.screen = lib.mkEnableOption "the full-screen browser showing the installer's steps";

  config = {
    environment.systemPackages = [ backup ];
    networking.networkmanager.enable = true;
    # the screen's secret, made at each boot: root's and the kiosk's alone.
    # The kiosk opens the page with it; the page sends it with every ask
    systemd.services.dd-box-setup-token = {
      description = "Make the box installer screen's secret";
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
      };
      script = ''
        install -d -m 0750 -g ${if config.dd.setup.screen then "setup" else "root"} /run/dd-setup-token
        umask 027
        od -An -vtx1 -N32 /dev/urandom | tr -d ' \n' > /run/dd-setup-token/token
        chgrp ${if config.dd.setup.screen then "setup" else "root"} /run/dd-setup-token/token
      '';
    };
    systemd.services.dd-box-setup = {
      description = "The box installer's steps, on loopback";
      wantedBy = [ "multi-user.target" ];
      requires = [ "dd-box-setup-token.service" ];
      after = [
        "network.target"
        "dd-box-setup-token.service"
      ];
      path = [
        "/run/current-system/sw"
        "/run/wrappers"
      ];
      environment =
        lib.getAttrs [
          "RELEASE_URL"
          "RELEASE_KEY"
          "SOURCE"
          "DETECT"
          "NOT_BEFORE"
          "FILES"
        ] config.environment.variables
        // {
          SETUP_WEB = "${./web}";
          SETUP_SHARED = "${../../servers/web-shared}";
        };
      serviceConfig = {
        ExecStart = "${agent}/bin/dd-box-setup";
        Restart = "always";
        RuntimeDirectory = "dd-setup";
      };
    };
    # the screen: the steps, and nothing else to click away to
    users.users.setup = lib.mkIf config.dd.setup.screen { isNormalUser = true; };
    systemd.services."cage-tty1" = lib.mkIf config.dd.setup.screen {
      after = [ "dd-box-setup-token.service" ];
      requires = [ "dd-box-setup-token.service" ];
    };
    services.cage = lib.mkIf config.dd.setup.screen {
      enable = true;
      user = "setup";
      program = pkgs.writeShellScript "dd-box-screen" ''
        exec ${pkgs.firefox}/bin/firefox --kiosk --private-window \
          "http://127.0.0.1:8090/#t=$(cat /run/dd-setup-token/token)"
      '';
      environment.MOZ_ENABLE_WAYLAND = "1";
    };
  };
}
