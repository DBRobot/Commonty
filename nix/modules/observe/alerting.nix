{
  config,
  lib,
  pkgs,
  ddScript,
  ...
}:
let
  # Not an address: the mail Worker sends "alerts" to the owner's own
  # (modules/mail.nix), so no address is in this public repo or on the box.
  to = "alerts";
in
{
  # Everything here ends at an email. metrics.nix records; this interrupts.
  # alertmanager belongs here too when it arrives.
  services.zfs.zed = {
    enableMail = false; # dd-sendmail (modules/mail.nix), not the mail package
    settings = {
      ZED_EMAIL_ADDR = [ to ];
      ZED_EMAIL_PROG = config.dd.mail.sendmail;
      # no @SUBJECT@: zed then writes a Subject header itself
      ZED_EMAIL_OPTS = "@ADDRESS@";
      ZED_NOTIFY_VERBOSE = true; # also report scrub completions, not just faults
      ZED_NOTIFY_INTERVAL_SECS = 3600;
      ZED_USE_ENCLOSURE_LEDS = false; # usb dock, no backplane
    };
  };

  # Every box's disks and pools, and every box's backups, from the fleet
  # view: each box writes its own facts (observe/disks.nix, storage/backup.nix)
  # and this, the box that can mail, reads them all and mails what is new.
  # Up to a block's length late for another box (thanos.blockDuration); that
  # is the price of no box telling another anything.
  systemd.services.dd-fleet-alerts = {
    description = "Mail what is wrong on any box, from the fleet view";
    after = [ "thanos-query.service" ];
    path = [
      pkgs.curl
      pkgs.jq
      pkgs.coreutils
    ];
    serviceConfig = {
      Type = "oneshot";
      StateDirectory = "dd-fleet-alerts";
    };
    script = ddScript ./fleet-alerts.sh {
      QUERY = "http://127.0.0.1:10903/api/v1/query";
      STATE = "/var/lib/dd-fleet-alerts/sent.json";
      WIRED = "/var/lib/dd-fleet-alerts/wired.json";
      TO = to;
      SENDMAIL = config.dd.mail.sendmail;
      # a block's length and an hour: every box's newest numbers are in it
      WINDOW =
        let
          b = config.dd.thanos.blockDuration;
        in
        if lib.hasSuffix "h" b then "${toString (lib.toInt (lib.removeSuffix "h" b) + 1)}h" else "3h";
    };
  };
  systemd.timers.dd-fleet-alerts = {
    wantedBy = [ "timers.target" ];
    timerConfig = {
      OnActiveSec = "5min";
      OnUnitActiveSec = "15min";
    };
  };
}
