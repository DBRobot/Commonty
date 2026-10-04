# Every service's needs, typed (needs-type.nix) and in one set: placement,
# the fit check and the release read this. A service lists its needs beside
# its module, in needs.nix.
{ lib }:
let
  files = [
    ../modules/llm/needs.nix
  ];
  checked = lib.evalModules {
    modules = [
      { options.needs = lib.mkOption { type = import ./needs-type.nix { inherit lib; }; }; }
      { needs = lib.foldl' (a: f: a // import f) { } files; }
    ];
  };
in
checked.config.needs
