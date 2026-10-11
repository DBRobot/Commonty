# Every box watches its own disks. smartd tests each disk the box reported
# (fleet/boxes/<box>/hardware.json) and says what it finds in its journal;
# a week of that, with the pools' health, becomes two numbers in this box's facts,
# which reach the fleet view like every other metric. The observe box reads
# them for every box and mails (alerting.nix): the mail credentials stay on
# that one box, and no box tells another anything.
{
  config,
  pkgs,
  lib,
  ...
}:
let
  facts = "/var/lib/dd-facts";
in
{
  config = lib.mkIf (config.dd.box.disks != [ ]) {
    services.smartd = {
      enable = true;
      notifications.mail.enable = lib.mkForce false;
      defaults.monitored = "-a -o on -S on -s (S/../.././02|L/../../6/03)";
      devices = map (d: { device = d; }) config.dd.box.disks;
    };

    # a week of disk events and the pools' health, as facts, every ten minutes
    systemd.services.dd-disk-health = {
      description = "This box's disks and pools, as facts";
      path = [
        pkgs.coreutils
        pkgs.gnugrep
        pkgs.systemd
        config.boot.zfs.package
      ];
      serviceConfig.Type = "oneshot";
      script = ''
        set -eu
        # what smartd logs when a disk is worse than it was: a failed or
        # failing attribute, a self-test or error log that grew, sectors it
        # cannot read (temperature changes are not news)
        warnings=$(journalctl -u smartd --since "-7 days" -o cat --no-pager 2>/dev/null \
          | grep -ciE 'FAILED|failing|prefail|error count increased|unreadable|uncorrectable|self-test.*fail' || true)
        if [ "$(zpool status -x 2>&1)" = "all pools are healthy" ]; then pools=0; else pools=1; fi
        printf '# disk events in the last week (smartd), and whether a pool is not healthy\ndd_disk_warnings %s\ndd_pool_unhealthy %s\n' \
          "$warnings" "$pools" > ${facts}/disks.prom.tmp
        chmod 0644 ${facts}/disks.prom.tmp
        mv ${facts}/disks.prom.tmp ${facts}/disks.prom
      '';
    };
    systemd.timers.dd-disk-health = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "2min";
        OnUnitActiveSec = "10min";
      };
    };
  };
}
