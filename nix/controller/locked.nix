# The controller as it runs from its locked partition. The card:
#
#   FIRMWARE   (FAT)    the Pi's firmware and u-boot
#   NIXOS_SD   (ext4)   the seed: the system the image first booted, and /boot,
#                       where u-boot finds the kernels and extlinux.conf
#   controller-root (LUKS2 -> ext4)   this system, and everything of the home
#
# The early boot brings up the wired network and asks the house box's Tang
# to open controller-root (Clevis, bound by first-boot.nix: the binding lives
# in the LUKS header, so nothing about it is in this system). Away from the
# house there is no Tang to ask, and the card stays locked.
{
  config,
  lib,
  pkgs,
  ...
}:
let
  unlock = pkgs.writeShellScript "dd-unlock" ''
    export PATH=${
      lib.makeBinPath [
        pkgs.clevis
        pkgs.jose
        pkgs.curl
        pkgs.cryptsetup
        pkgs.coreutils
        pkgs.gnugrep
        pkgs.gnused
        pkgs.bash
      ]
    }
    part=/dev/disk/by-label/controller-root
    until [ -e "$part" ]; do sleep 1; done
    # Tang may be a box still starting after the same power cut: keep asking
    until clevis luks unlock -d "$part" -n root; do
      echo "the house box's Tang did not open the card; asking again in 10 s"
      sleep 10
    done
  '';
in
{
  fileSystems."/" = {
    device = "/dev/mapper/root";
    fsType = "ext4";
  };
  fileSystems."/seed" = {
    device = "/dev/disk/by-label/NIXOS_SD";
    fsType = "ext4";
    neededForBoot = true;
  };
  fileSystems."/boot" = {
    device = "/seed/boot";
    fsType = "none";
    options = [ "bind" ];
    depends = [ "/seed" ];
  };

  boot.initrd.systemd = {
    enable = true;
    network = {
      enable = true;
      networks."10-wired" = {
        matchConfig.Name = "en* eth*";
        networkConfig.DHCP = "yes";
      };
    };
    storePaths = [ unlock ];
    services.dd-unlock = {
      description = "Open the card through the house box's Tang";
      wantedBy = [ "initrd.target" ];
      before = [
        "sysroot.mount"
        "initrd-root-device.target"
      ];
      after = [ "network-online.target" ];
      wants = [ "network-online.target" ];
      unitConfig.DefaultDependencies = false;
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = unlock;
      };
    };
  };
  boot.initrd.kernelModules = [ "dm_crypt" ];
  boot.initrd.availableKernelModules = [
    "dm_crypt"
    "dm_mod"
  ];
}
