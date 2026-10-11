{ config, pkgs, ... }:
{
  # Every role that declares a secret imports this; core does, so every box
  # has sops, each reading its own file only.
  sops.defaultSopsFile = ../../secrets + "/${config.networking.hostName}.yaml";
  # The box decrypts with the key in its TPM (modules/box/report.nix makes
  # it): the file is a handle only this box's TPM can use, so a copy of it,
  # or of the disk, opens nothing. The ssh host key opens nothing either.
  sops.age.plugins = [ pkgs.age-plugin-tpm ];
  sops.age.keyFile = "/var/lib/dd/secrets-key.txt";
  sops.age.sshKeyPaths = [ ];
}
