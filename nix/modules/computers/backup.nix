# Each remote computer's /home disk, each night (docs/dev/remote-computers.md):
# a ZFS snapshot of the dataset its directory is on, made for the run, read by
# restic into a repository of their own beside the box's backup, at computers/
# of its bucket, so the copy of them can be held back alone (dd.store.guarded).
# The snapshot is all that touches the disks. The system disk stays out: the
# release's image and the owner's Nix file make it again.
{
  config,
  lib,
  options,
  pkgs,
  ...
}:
let
  b = config.dd.backup;
  box = config.networking.hostName;
  bucket = "backups-${box}";
  dir = "/var/lib/private/dd-computer";
  facts = "/var/lib/dd-facts";
  snap = "dd-computers";
  zfs = "${config.boot.zfs.package}/bin/zfs";
  findmnt = "${pkgs.util-linux}/bin/findmnt";
  dataset = "$(${findmnt} -no SOURCE -T ${dir})";
  # is there a computer, a dataset to snapshot and room in the store? Not: the
  # run is skipped, never failed, and a store short of room says so
  room = pkgs.writeShellScript "dd-computers-backup-room" ''
    set -u
    export PATH=${
      lib.makeBinPath [
        pkgs.coreutils
        pkgs.gawk
        pkgs.findutils
      ]
    }
    out=${facts}/computers-backup.prom
    homes=$(find -P ${dir} -mindepth 2 -maxdepth 2 -type f -name home.qcow2)
    if [ -z "$homes" ]; then rm -f "$out"; exit 1; fi
    if [ "$(${findmnt} -no FSTYPE -T ${dir})" != zfs ]; then
      echo "the computers' directory is not on ZFS: no snapshot to read"
      exit 1
    fi
    # on disk: what it can cost, the first night at most
    need=$(printf '%s\n' "$homes" | xargs -d '\n' du -B1 -P | awk '{ s += $1 } END { printf "%.0f", s }')
    free=$(awk '/^dd_store_free_bytes/ { print $2 }' ${facts}/store.prom 2>/dev/null)
    size=$(awk '/^dd_store_size_bytes/ { print $2 }' ${facts}/store.prom 2>/dev/null)
    case "$free$size" in "" | *[!0-9]*) free=0 size=0 ;; esac
    floor=$((size / 10 > ${toString config.dd.store.copyFloorBytes} ? size / 10 : ${toString config.dd.store.copyFloorBytes}))
    if [ $((free - floor)) -lt "$need" ]; then
      echo "the store is short of room: $need bytes of disks, $free free, $floor to stay free"
      echo "dd_store_copy_skipped{bucket=\"${bucket}\",box=\"${box}\"} 1" > "$out.tmp"
      chmod 0644 "$out.tmp"
      mv "$out.tmp" "$out"
      exit 1
    fi
    rm -f "$out"
  '';
in
{
  config = lib.mkIf (b.passwordFile != null && b.envFile != null && config.dd.store.enable) {
    dd.store.guarded.${bucket} = [ "computers/" ];
    services.restic.backups.dd-computers = {
      repository = "s3:${b.endpoint}/${bucket}/computers";
      environmentFile = toString b.envFile;
      passwordFile = toString b.passwordFile;
      # (below: a new one only where no other store keeps one)
      initialize = false;
      backupPrepareCommand = ''
        ${zfs} destroy ${dataset}@${snap} 2>/dev/null || true
        ${zfs} snapshot ${dataset}@${snap}
      '';
      backupCleanupCommand = "${zfs} destroy ${dataset}@${snap} || true";
      # never through a link: the guest's QEMU writes in these directories
      dynamicFilesFrom = ''
        mnt=$(${findmnt} -no TARGET -T ${dir})
        d=${dir}
        ${pkgs.findutils}/bin/find -P "$mnt/.zfs/snapshot/${snap}/''${d#"$mnt"}" -mindepth 2 -maxdepth 2 -type f \( -name home.qcow2 -o -name record.json \)
      '';
      timerConfig = {
        # after the box's own backup, not beside it
        OnCalendar = "05:00";
        RandomizedDelaySec = "1h";
        Persistent = true;
      };
      # as the box's own (nix/tests/retention.nix)
      pruneOpts = [
        "--keep-daily 7"
        "--keep-weekly 4"
        "--group-by host"
      ];
      checkOpts = [ "--read-data-subset=2%" ];
    };
    systemd.services.restic-backups-dd-computers = {
      environment.AWS_DEFAULT_REGION = "us-east-1";
      path = [
        pkgs.curl
        pkgs.age
        pkgs.rclone
        pkgs.coreutils
      ]
      ++ config.dd.store.agePlugins;
      preStart = lib.mkBefore ''
        ${b.newRepoGuard "/computers"}
      '';
      after = [
        "network-online.target"
        "dd-store-setup.service"
      ];
      wants = [ "network-online.target" ];
      serviceConfig.ExecCondition = "${room}";
      onFailure = lib.optional (options.dd ? mail) "dd-alert@restic-backups-dd-computers.service";
    };
  };
}
