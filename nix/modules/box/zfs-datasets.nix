# Declared ZFS datasets. A box's hardware file says which pool holds what;
# the oneshot below creates a dataset if missing and sets any property that
# differs. It never destroys or remounts anything that is already right, so
# it is safe to run against live data.
{
  config,
  pkgs,
  lib,
  ...
}:
let
  zfs = "${pkgs.zfs}/bin/zfs";
  mkDataset =
    name: props:
    lib.concatStringsSep "\n" (
      [ "${zfs} list -H ${name} >/dev/null 2>&1 || ${zfs} create ${name}" ]
      # only what differs: setting even the same mountpoint unmounts the
      # dataset, and one in use (forgejo's) fails, taking its service down
      ++ lib.mapAttrsToList (
        k: v: ''[ "$(${zfs} get -H -o value ${k} ${name})" = "${v}" ] || ${zfs} set ${k}=${v} ${name}''
      ) props
    );
in
{
  options.dd.zfs.datasets = lib.mkOption {
    type = lib.types.attrsOf (lib.types.attrsOf lib.types.str);
    default = { };
    description = "pool/dataset = { property = value; } to exist on this box.";
  };

  config.systemd.services.zfs-datasets = lib.mkIf (config.dd.zfs.datasets != { }) {
    description = "Ensure declared ZFS datasets exist with their properties";
    wantedBy = [ "multi-user.target" ];
    after = [
      "zfs-import.target"
      "zfs-mount.service"
    ];
    serviceConfig = {
      Type = "oneshot";
      RemainAfterExit = true;
    };
    script = lib.concatStringsSep "\n" (lib.mapAttrsToList mkDataset config.dd.zfs.datasets);
  };
}
