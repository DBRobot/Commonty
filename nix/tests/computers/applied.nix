# An owner's Nix file, applied live in the computers test
{ ... }:
{
  environment.etc."dd-applied".text = "applied\n";
}
