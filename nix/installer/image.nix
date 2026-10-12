# The box installer as a drive (docs/dev/joining.md): a GPT disk image the
# app's installer window writes to a USB drive. Partition 1 is the FAT boot
# partition: the boot loader, the installer, and commonty/, where the app
# puts the member's join ticket, the backup's key, the Wi-Fi and the
# version. Partition 2 is the installer's system. It boots to the screen
# (setup.nix) and nothing else.
{
  config,
  lib,
  pkgs,
  modulesPath,
  self,
  ...
}:
let
  efiArch = config.nixpkgs.hostPlatform.efiArch;
  top = config.system.build.toplevel;
in
{
  imports = [
    "${modulesPath}/image/repart.nix"
    "${modulesPath}/profiles/all-hardware.nix"
    ./iso.nix
    ./setup.nix
  ];

  dd.setup.screen = true;
  boot.loader.grub.enable = false;
  hardware.enableRedistributableFirmware = true;
  fileSystems."/" = {
    device = "/dev/disk/by-partlabel/installer";
    fsType = "ext4";
    autoResize = true;
  };
  # the system's partition made as small as it fits, then grown to the
  # drive at the first boot
  boot.initrd.systemd.enable = true;
  boot.initrd.systemd.repart.enable = true;
  systemd.repart.partitions."20-installer".Type = "root";
  fileSystems."/boot" = {
    device = "/dev/disk/by-partlabel/esp";
    fsType = "vfat";
    options = [ "umask=0077" ];
  };
  boot.tmp.useTmpfs = true;
  services.journald.storage = "volatile";
  # the store as the image holds it, known to nix on the first boot
  boot.postBootCommands = ''
    if [ -f /nix-path-registration ]; then
      ${config.nix.package.out}/bin/nix-store --load-db < /nix-path-registration
      rm -f /nix-path-registration
    fi
  '';

  image.repart = {
    name = "commonty-box-installer";
    # OVMF and many PCs want 512-byte sectors on a USB drive
    sectorSize = 512;
    compression = {
      enable = true;
      algorithm = "zstd";
    };
    partitions = {
      "10-esp" = {
        contents = {
          "/EFI/BOOT/BOOT${lib.toUpper efiArch}.EFI".source =
            "${pkgs.systemd}/lib/systemd/boot/efi/systemd-boot${efiArch}.efi";
          "/EFI/Linux/${config.system.boot.loader.ukiFile}".source =
            "${config.system.build.uki}/${config.system.boot.loader.ukiFile}";
          "/loader/loader.conf".source = pkgs.writeText "loader.conf" "timeout 0\neditor no\n";
          "/commonty/version".source = pkgs.writeText "version" "${self.shortRev or "dirty"}\n";
        };
        repartConfig = {
          Type = "esp";
          Format = "vfat";
          Label = "esp";
          SizeMinBytes = "256M";
        };
      };
      "20-installer" = {
        storePaths = [ top ];
        contents."/nix-path-registration".source = "${
          pkgs.closureInfo { rootPaths = [ top ]; }
        }/registration";
        repartConfig = {
          Type = "root";
          Format = "ext4";
          Label = "installer";
          Minimize = "guess";
        };
      };
    };
  };
  system.stateVersion = "26.05";
}
