{ config, pkgs, ... }:
let
  # Not an address: an msmtp alias, expanded from sops at send time by
  # modules/mail.nix. This repo is public and a real address here is a real
  # address published.
  to = "alerts";
in
{
  # Everything here ends at an email. metrics.nix records; this interrupts.
  # alertmanager belongs here too when it arrives.
  services.zfs.zed = {
    enableMail = false; # we provide sendmail via msmtp, not the mail package
    settings = {
      ZED_EMAIL_ADDR = [ to ];
      ZED_EMAIL_PROG = "${pkgs.msmtp}/bin/msmtp";
      ZED_EMAIL_OPTS = "-a default @ADDRESS@";
      ZED_NOTIFY_VERBOSE = true; # also report scrub completions, not just faults
      ZED_NOTIFY_INTERVAL_SECS = 3600;
      ZED_USE_ENCLOSURE_LEDS = false; # usb dock, no backplane
    };
  };

  services.smartd = {
    enable = true;
    notifications.mail = {
      enable = true;
      sender = "distributed.datacenter@gmail.com";
      recipient = to;
      mailer = "${pkgs.msmtp}/bin/msmtp";
    };
    defaults.monitored = "-a -o on -S on -s (S/../.././02|L/../../6/03)";
    # every disk the box reported (fleet/boxes/<box>/hardware.json)
    devices = map (d: { device = d; }) config.dd.box.disks;
  };
}
