# Every service's needs, typed (needs-type.nix) and in one set: placement,
# the fit check and the release read this. A service lists its needs beside
# its module, in needs.nix.
{ lib }:
let
  files = [
    ../modules/adblock/needs.nix
    ../modules/forge/needs.nix
    ../modules/games/needs.nix
    ../modules/library/needs.nix
    ../modules/llm/needs.nix
    ../modules/photos/needs.nix
    ../modules/storage/needs.nix
    ../modules/vault/needs.nix
  ];
  checked = lib.evalModules {
    modules = [
      { options.needs = lib.mkOption { type = import ./needs-type.nix { inherit lib; }; }; }
      { needs = lib.foldl' (a: f: a // import f) { } files; }
    ];
  };
in
checked.config.needs
