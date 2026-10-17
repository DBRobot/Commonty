# Every hour, a calendar timer that has lost its next run is restarted
# (timer-watch.sh): a stuck timer once stopped node2's backups for five days.
# The count is a fact, so the fleet view sees a box where it happened.
{ pkgs, ... }:
{
  systemd.services.dd-timer-watch = {
    description = "Restart any calendar timer that has lost its next run";
    # never run by a switch: a switch is when a timer can lose its next run,
    # and a failure there rolls the release back
    restartIfChanged = false;
    path = [
      pkgs.systemd
      pkgs.coreutils
    ];
    serviceConfig.Type = "oneshot";
    script = builtins.readFile ./timer-watch.sh;
  };
  systemd.timers.dd-timer-watch = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnActiveSec = "5min";
      OnUnitActiveSec = "1h";
    };
  };
}
