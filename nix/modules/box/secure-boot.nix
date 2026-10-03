# Secure Boot. Each box signs what it boots with keys of its own
# (/var/lib/sbctl, made on first switch), so its firmware runs nothing else
# once the keys are enrolled - a step at the box's own screen, once
# (docs/secure-boot.md). The TPM unseals the disk key's half only under
# that signed boot (PCR 7, nix/modules/box/disk-unlock.nix), and a release
# signed by the same keys leaves PCR 7 as it was.
{ lib, pkgs, ... }:
{
  boot.loader.systemd-boot.enable = lib.mkForce false;
  boot.lanzaboote = {
    enable = true;
    pkiBundle = "/var/lib/sbctl";
    autoGenerateKeys.enable = true;
    settings.editor = false;
    # each entry carries its own kernel and initrd: eight fit the 1 GB ESP
    configurationLimit = 8;
  };
  environment.systemPackages = [ pkgs.sbctl ];
  # A firmware or revocation-list update changes PCR 7, and the TPM half
  # would not open until the paper key is used once: updates are the
  # owner's to choose, never automatic
  services.fwupd.enable = lib.mkForce false;
}
