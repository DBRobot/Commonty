# a week of ZFS snapshots, never weekly or monthly (docs/dev/servers.md, Retention)
{
  config,
  lib,
  pkgs,
  ...
}:
let
  zfs = "${pkgs.zfs}/bin/zfs";
  maxDays = 7;
in
{
  options.dd.zfs.snapshotMaxDays = lib.mkOption {
    type = lib.types.int;
    default = maxDays;
    readOnly = true;
    description = "the oldest an automatic snapshot is kept, in days: dd-snapshot-trim destroys any older (nix/tests/retention.nix reads it)";
  };

  config = lib.mkIf config.services.zfs.autoSnapshot.enable {
    services.zfs.autoSnapshot = {
      frequent = 4;
      hourly = 24;
      daily = 7;
    };
    # masked, so no dataset's com.sun:auto-snapshot:weekly can bring them back
    systemd.timers.zfs-snapshot-weekly.enable = false;
    systemd.timers.zfs-snapshot-monthly.enable = false;
    systemd.services.zfs-snapshot-weekly.enable = false;
    systemd.services.zfs-snapshot-monthly.enable = false;

    # what the weekly and monthly runs left behind, and anything automatic
    # past a week that a stopped timer did not prune; nothing named by hand
    systemd.services.dd-snapshot-trim = {
      description = "Destroy automatic ZFS snapshots older than ${toString maxDays} days";
      after = [ "zfs-import.target" ];
      restartIfChanged = false;
      serviceConfig.Type = "oneshot";
      script = ''
        cutoff=$(( $(date +%s) - ${toString maxDays} * 86400 ))
        ${zfs} list -Hp -t snapshot -o name,creation | while read -r name created; do
          case "$name" in *@zfs-auto-snap_*) ;; *) continue ;; esac
          case "$created" in "" | *[!0-9]*) continue ;; esac
          if [ "$created" -lt "$cutoff" ]; then
            ${zfs} destroy "$name" && echo "destroyed $name" || echo "could not destroy $name" >&2
          fi
        done
        exit 0
      '';
    };
    systemd.timers.dd-snapshot-trim = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnActiveSec = "10min";
        OnUnitActiveSec = "6h";
      };
    };
  };
}
