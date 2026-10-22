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
  efiArch = pkgs.stdenv.hostPlatform.efiArch;
  top = config.system.build.toplevel;
  closure = pkgs.closureInfo { rootPaths = [ top ]; };
  # the uuid systemd-repart gave it from the image's seed, kept
  uuid = "7981aaf4-2a9a-4f86-9f1f-5b42c8f28bbf";
  # Both file systems are made here, in the image's own build, and
  # systemd-repart only lays them out (CopyBlocks=). Made by repart, their
  # bytes depended on the builder's disk: it sized the ext4 by SEEK_DATA
  # over a sparse trial image (ZFS answers per 128K record, ext4 per block)
  # and copied the store with the builder's hard links, xattrs and reflinks.
  # Here the copy is plain, the size counts the files' own lengths, mkfs and
  # mcopy add things in a fixed order, and every time is SOURCE_DATE_EPOCH
  # (docs/dev/joining.md). Kept in one build, so no image of several GB is
  # left in the store beside the drive.
  makeRoot = ''
    mkdir -p root/nix/store
    xargs -a ${closure}/store-paths cp -r --preserve=mode,timestamps --reflink=never -t root/nix/store
    cp ${closure}/registration root/nix-path-registration
    # blocks for file data, one per directory and long symlink, the
    # directories' entries; an inode each and an eighth more
    read -r n blocks < <(find root -printf '%y %s %f\n' | awk '
      { n++; ent += length($3) + 12 }
      $1 == "f" { b += int(($2 + 4095) / 4096) }
      $1 == "d" || ($1 == "l" && $2 > 59) { b++ }
      END { print n, b + int(ent / 4096) }')
    inodes=$(( n + n / 8 ))
    # a tenth more, the journal, the inode tables and 64 MiB, in MiB
    truncate -s "$(( (blocks * 11 / 10 + 32768 + inodes / 16) / 256 + 64 ))M" root.img
    # under --map-root-user the copy is root's, as the image needs
    unshare --map-root-user mkfs.ext4 -q -F -b 4096 -I 256 -m 0 -N $inodes -J size=128 \
      -L installer -U ${uuid} \
      -E hash_seed=${uuid},lazy_itable_init=0,lazy_journal_init=0,nodiscard \
      -d root root.img
    e2fsck -fn root.img
    chmod -R u+w root
    rm -rf root
  '';
  espFiles = {
    "EFI/BOOT/BOOT${lib.toUpper efiArch}.EFI" =
      "${pkgs.systemd}/lib/systemd/boot/efi/systemd-boot${efiArch}.efi";
    "EFI/Linux/${config.system.boot.loader.ukiFile}" =
      "${config.system.build.uki}/${config.system.boot.loader.ukiFile}";
    "loader/loader.conf" = pkgs.writeText "loader.conf" "timeout 0\neditor no\n";
    "commonty/version" = pkgs.writeText "version" "${self.shortRev or "dirty"}\n";
  };
  espDirs = lib.unique (
    lib.concatMap (
      to:
      let
        parts = lib.init (lib.splitString "/" to);
      in
      lib.genList (i: lib.concatStringsSep "/" (lib.take (i + 1) parts)) (lib.length parts)
    ) (lib.attrNames espFiles)
  );
  # repart's own options for it (FAT32, its volume id), then in name order,
  # each directory before what is in it, so the order of the entries is
  # this list's and never a directory listing's
  makeEsp = ''
    truncate -s 256M esp.img
    mkfs.fat -i 793A442B -n ESP -F 32 -S 512 esp.img >/dev/null
    ${lib.concatMapStrings (d: "MTOOLS_SKIP_CHECK=1 mmd -i esp.img ::/${d}\n") espDirs}
    ${lib.concatStrings (
      lib.mapAttrsToList (
        to: from: "MTOOLS_SKIP_CHECK=1 mcopy -p -Q -m -i esp.img ${from} ::/${to}\n"
      ) espFiles
    )}
    fsck.fat -n esp.img
  '';
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
      "10-esp".repartConfig = {
        Type = "esp";
        Label = "esp";
      };
      "20-installer".repartConfig = {
        Type = "root";
        Label = "installer";
      };
    };
  };
  system.build.image = lib.mkForce (
    config.image.repart.image.overrideAttrs (o: {
      nativeBuildInputs = o.nativeBuildInputs ++ [
        pkgs.e2fsprogs
        pkgs.dosfstools
        pkgs.mtools
      ];
      preBuild = ''
        export SOURCE_DATE_EPOCH=315532800 TZ=UTC
        ${makeRoot}
        ${makeEsp}
        echo "CopyBlocks=$PWD/esp.img" >> $finalRepartDefinitions/10-esp.conf
        echo "CopyBlocks=$PWD/root.img" >> $finalRepartDefinitions/20-installer.conf
      '';
    })
  );
  system.stateVersion = "26.05";
}
