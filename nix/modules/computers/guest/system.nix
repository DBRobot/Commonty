# The system a computer's guest builds for itself, from base.nix and its
# owner's Nix file, evaluated as its own configuration.nix has it (one
# module importing the two, in that order: the order of its packages
# follows the order of the imports). The guest names the bridge by store
# path where this names a derivation, so its first rebuild makes the few
# derivations that hold it again, for the same contents. The image's
# (computers.nix), and what the computers test hands its apply.
{
  pkgs,
  netbridge,
  member,
}:
import "${pkgs.path}/nixos" {
  system = pkgs.stdenv.hostPlatform.system;
  configuration = {
    imports = [
      ./base.nix
      member
    ];
    dd.guest.base = ./base.nix;
    dd.guest.nixpkgs = pkgs.path;
    dd.guest.netbridge = netbridge;
  };
}
