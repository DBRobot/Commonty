# golang.org/x/exp/cmd/apidiff: whether two versions of a Go package's
# exported API differ. Not in nixpkgs; built here from a pinned commit.
{ pkgs }:
pkgs.buildGoModule {
  pname = "apidiff";
  version = "0-unstable-2026-09-08";
  src = pkgs.fetchFromGitHub {
    owner = "golang";
    repo = "exp";
    rev = "85c1c2202aba13ab812be55279024d8594344c1f";
    hash = "sha256-w/KukTKFgLeIEXCcZlFhZEq45txYaOT42Tqqw+PTVIM=";
  };
  vendorHash = "sha256-HBB34uM+91E9vWI3t9BVJEWsS7b9c255ewNCKWUQDVQ=";
  subPackages = [ "cmd/apidiff" ];
  # its tests load packages from the network
  doCheck = false;
}
