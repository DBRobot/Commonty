{
  config,
  lib,
  pkgs,
  ...
}:
{
  # whether this box's own file holds a secret: its names are in the clear
  # there, its values not. What a box has no secret for it runs without (a
  # box that took a service over has none of another box's)
  options.dd.hasSecret = lib.mkOption {
    type = lib.types.functionTo lib.types.bool;
    readOnly = true;
    default =
      name:
      lib.hasInfix "\n${name}:" (
        "\n" + builtins.readFile (../../secrets + "/${config.networking.hostName}.yaml")
      );
  };
  config = {
    # Every role that declares a secret imports this; core does, so every box
    # has sops, each reading its own file only.
    sops.defaultSopsFile = ../../secrets + "/${config.networking.hostName}.yaml";
    # The box decrypts with the key in its TPM (modules/box/report.nix makes
    # it): the file is a handle only this box's TPM can use, so a copy of it,
    # or of the disk, opens nothing. The ssh host key opens nothing either.
    sops.age.plugins = [ pkgs.age-plugin-tpm ];
    sops.age.keyFile = "/var/lib/dd/secrets-key.txt";
    sops.age.sshKeyPaths = [ ];
  };
}
