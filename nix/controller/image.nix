# The card image: the seed system on NIXOS_SD, left at its own size so the
# rest of the card is free for the locked partition first-boot.nix makes.
#
# nixpkgs builds the image with aarch64 tools, which CI's x86 runners run
# under qemu: minutes of mkfs and zstd over the whole closure. Its steps
# only copy files, so cardImage builds the same image with x86 tools.
{
  config,
  lib,
  modulesPath,
  pkgsX86,
  ...
}:
let
  extlinux =
    import (modulesPath + "/system/boot/loader/generic-extlinux-compatible/extlinux-conf-builder.nix")
      {
        inherit lib;
        pkgs = pkgsX86;
      };
  # populateCmd is "<aarch64 builder> <args>": the same args, an x86 builder
  args = lib.concatStringsSep " " (
    lib.tail (lib.splitString " " config.boot.loader.generic-extlinux-compatible.populateCmd)
  );
  # nixpkgs fixes the filesystem's uuid but lets mkfs pick a random
  # directory hash seed, so two boxes made different bytes; the seed is the
  # uuid too
  uuid = "44444444-4444-4444-8888-888888888888";
  mkfs = "mkfs.ext4 -L ${config.sdImage.rootVolumeLabel} -U ${uuid}";
  rootFs = pkgsX86.callPackage (modulesPath + "/../lib/make-ext4-fs.nix") {
    inherit (config.sdImage) storePaths compressImage;
    inherit uuid;
    volumeLabel = config.sdImage.rootVolumeLabel;
    populateImageCommands = ''
      mkdir -p ./files/boot
      ${extlinux} ${args} -c ${config.system.build.toplevel} -d ./files/boot
    '';
  };
in
{
  imports = [ (modulesPath + "/installer/sd-card/sd-image-aarch64.nix") ];
  image.baseName = "commonty-home-controller";
  sdImage = {
    expandOnBoot = false;
    compressImage = true;
    rootFilesystemImage = rootFs.overrideAttrs (o: {
      buildCommand =
        assert lib.hasInfix mkfs o.buildCommand;
        lib.replaceStrings [ mkfs ] [ "${mkfs} -E hash_seed=${uuid}" ] o.buildCommand;
    });
  };
  system.build.cardImage =
    let
      nixpkgs = config.system.build.sdImage;
    in
    pkgsX86.stdenv.mkDerivation {
      inherit (nixpkgs) name;
      inherit (nixpkgs.drvAttrs) buildCommand compressImage;
      nativeBuildInputs = with pkgsX86; [
        dosfstools
        e2fsprogs
        libfaketime
        mtools
        util-linux
        zstd
      ];
    };
}
