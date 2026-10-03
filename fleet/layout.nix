# The one disk layout every box gets. No sizes to decide: an ESP the
# firmware needs, and the rest of the boot disk is a ZFS pool that root is a
# dataset on. Root keeps a reservation so a full pool can never brick the
# box; everything else is datasets with no size at all. Other disks become
# a pool each, named by what they are (modules/zfs-datasets.nix adds the
# datasets a role wants).
#
#   imports = [ (import ../../fleet/layout.nix { device = "/dev/disk/by-id/..."; }) ];
{
  device,
  pool ? "tank",
  # root holds /etc and little else (/nix, /var, /home are their own
  # datasets); this only has to keep it writable when the pool fills
  rootReservation ? "2G",
  # every dataset under <pool>/enc, encrypted with the key the initrd makes
  # from the TPM's half and the unlock Worker's (nix/modules/box/disk-unlock.nix)
  encrypted ? false,
}:
{ lib, ... }:
let
  under = if encrypted then "enc/" else "";
in
{
  disko.devices = {
    disk.boot = {
      type = "disk";
      inherit device;
      content = {
        type = "gpt";
        partitions = {
          ESP = {
            size = "1G";
            type = "EF00";
            content = {
              type = "filesystem";
              format = "vfat";
              mountpoint = "/boot";
              mountOptions = [
                "fmask=0022"
                "dmask=0022"
              ];
            };
          };
          pool = {
            size = "100%";
            content = {
              type = "zfs";
              inherit pool;
            };
          };
        };
      };
    };
    zpool.${pool} = {
      type = "zpool";
      mode = "";
      options = {
        ashift = "12";
        autotrim = "on";
        cachefile = "none";
      };
      rootFsOptions = {
        compression = "zstd";
        acltype = "posixacl";
        xattr = "sa";
        atime = "off";
        mountpoint = "none";
        "com.sun:auto-snapshot" = "false";
      };
      datasets =
        lib.optionalAttrs encrypted {
          # the encryption root: everything below takes its key
          "enc" = {
            type = "zfs_fs";
            options = {
              mountpoint = "none";
              encryption = "aes-256-gcm";
              keyformat = "raw";
              keylocation = "file:///run/dd/disk.key";
            };
          };
        }
        // {
          # the system, as three datasets: a rollback of / never touches /nix
          # (the store is content-addressed; rolling it back gains nothing and
          # would drop paths newer generations need) or /home
          "${under}system" = {
            type = "zfs_fs";
            options.mountpoint = "none";
          };
          "${under}system/root" = {
            type = "zfs_fs";
            mountpoint = "/";
            options = {
              mountpoint = "legacy";
              reservation = rootReservation;
              "com.sun:auto-snapshot" = "true";
            };
          };
          "${under}system/nix" = {
            type = "zfs_fs";
            mountpoint = "/nix";
            options = {
              mountpoint = "legacy";
              atime = "off";
            };
          };
          "${under}system/var" = {
            type = "zfs_fs";
            mountpoint = "/var";
            options = {
              mountpoint = "legacy";
              "com.sun:auto-snapshot" = "true";
            };
          };
          "${under}home" = {
            type = "zfs_fs";
            mountpoint = "/home";
            options = {
              mountpoint = "legacy";
              "com.sun:auto-snapshot" = "true";
            };
          };
        };
    };
  };
}
