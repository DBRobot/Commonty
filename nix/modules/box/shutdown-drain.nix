# A box that joined, when its owner presses the power button or shuts it
# down: what it serves is handed on first, for as long as that takes and no
# longer than `seconds`. logind holds a clean shutdown back for a delay lock,
# and ends the wait itself however this goes, so nothing here can hold a
# shutdown past that. Not on a box of the release (the owner's dd-drain, by
# hand, there): its services are the fleet's and it comes back.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  cfg = config.dd.shutdownDrain;
in
{
  options.dd.shutdownDrain.seconds = lib.mkOption {
    type = lib.types.ints.positive;
    default = 60;
    description = "how long a shutdown waits for what this box runs to move: a heartbeat (30 s) to say it is leaving, then the plan's next round";
  };

  config = lib.mkIf (config.dd.box.joined or false) {
    services.logind.settings.Login = {
      # every other box ignores the button (host.nix): this one is its
      # owner's, and pressing it is how they turn it off
      HandlePowerKey = lib.mkForce "poweroff";
      InhibitDelayMaxSec = cfg.seconds + 15;
    };
    systemd.services.dd-shutdown-drain = {
      description = "Hand what this box runs on before it powers off";
      wantedBy = [ "multi-user.target" ];
      after = [
        "dbus.service"
        "systemd-logind.service"
      ];
      # a restart lets go of the lock for a moment and nothing else
      restartIfChanged = false;
      path = [
        pkgs.coreutils
        pkgs.curl
        pkgs.glib.bin # gdbus
        pkgs.jq
        pkgs.systemd
      ];
      environment.DRAIN_SECS = toString cfg.seconds;
      serviceConfig = {
        ExecStart = "${pkgs.systemd}/bin/systemd-inhibit --what=shutdown --mode=delay --who=commonty --why=handing-on-its-work ${pkgs.bash}/bin/bash ${./shutdown-drain.sh}";
        Restart = "on-failure";
        RestartSec = 10;
      };
      unitConfig.StartLimitIntervalSec = 0;
    };
  };
}
