# The VM a joined box boots in (nix/tests/joined-box.nix): the test's own
# machine, so nothing here.
{ lib, ... }:
{
  nixpkgs.hostPlatform = lib.mkDefault "x86_64-linux";
}
