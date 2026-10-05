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
  # properties of whichever dataset is mounted at a path; a quota never
  # below what the dataset already holds
  atMountpoint =
    mountpoint: props:
    ''
      ds=$(${zfs} list -H -o name,mountpoint | ${pkgs.gawk}/bin/awk -v m=${lib.escapeShellArg mountpoint} '$2 == m {print $1; exit}')
      if [ -n "$ds" ]; then
    ''
    + lib.concatStrings (
      lib.mapAttrsToList (
        k: v:
        if k == "quota" then
          ''
            if [ "$(${zfs} get -H -o value quota "$ds")" != ${lib.escapeShellArg v} ]; then
              used=$(${zfs} get -Hp -o value used "$ds")
              want=$(${pkgs.coreutils}/bin/numfmt --from=iec ${lib.escapeShellArg v})
              if [ "$used" -lt "$want" ]; then
                ${zfs} set quota=${lib.escapeShellArg v} "$ds"
              else
                echo "not capping $ds at ${v}: it holds more already" >&2
              fi
            fi
          ''
        else
          ''
            [ "$(${zfs} get -H -o value ${k} "$ds")" = ${lib.escapeShellArg v} ] || ${zfs} set ${k}=${lib.escapeShellArg v} "$ds"
          ''
      ) props
    )
    + "fi\n";
in
{
  options.dd.zfs.atMountpoint = lib.mkOption {
    type = lib.types.attrsOf (lib.types.attrsOf lib.types.str);
    default = { };
    description = "mountpoint = { property = value; }: set on whichever dataset is mounted there, for a module that knows its directory but not the box's pools. A quota is the most it may hold, so what a service keeps cannot take the room the box itself needs; it is never set below what is already there.";
  };

  options.dd.zfs.datasets = lib.mkOption {
    type = lib.types.attrsOf (lib.types.attrsOf lib.types.str);
    default = { };
    description = "pool/dataset = { property = value; } to exist on this box.";
  };

  config.systemd.services.zfs-datasets =
    lib.mkIf (config.dd.zfs.datasets != { } || config.dd.zfs.atMountpoint != { })
      {
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
        script = lib.concatStringsSep "\n" (
          lib.mapAttrsToList mkDataset config.dd.zfs.datasets
          ++ lib.mapAttrsToList atMountpoint config.dd.zfs.atMountpoint
        );
      };
}
