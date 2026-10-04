{ config, pkgs, ... }:
{
  # Every role that declares a secret imports this. A box with no such role
  # has no sops at all - node2 is a recipient of nothing.
  sops.defaultSopsFile = ../../secrets + "/${config.networking.hostName}.yaml";
  # The box decrypts with the key in its TPM (modules/box/report.nix makes
  # it): the file is a handle only this box's TPM can use, so a copy of it,
  # or of the disk, opens nothing. The ssh host key still does, until every
  # file is encrypted to the TPM keys alone.
  sops.age.plugins = [ pkgs.age-plugin-tpm ];
  sops.age.keyFile = "/var/lib/dd/secrets-key.txt";
  sops.age.sshKeyPaths = [ "/etc/ssh/ssh_host_ed25519_key" ];
}
