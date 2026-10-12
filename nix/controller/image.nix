# The card image: the seed system on NIXOS_SD, left at its own size so the
# rest of the card is free for the locked partition first-boot.nix makes.
{ modulesPath, ... }:
{
  imports = [ (modulesPath + "/installer/sd-card/sd-image-aarch64.nix") ];
  image.baseName = "commonty-home-controller";
  sdImage = {
    expandOnBoot = false;
    compressImage = true;
  };
}
