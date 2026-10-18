# Every service's needs, typed (needs-type.nix) and in one set: placement,
# the fit check and the release read this. A service lists its needs beside
# its module, in needs.nix.
{ lib }:
let
  files = [
    ../modules/adblock/needs.nix
    ../modules/box/needs.nix
    ../modules/forge/needs.nix
    ../modules/games/needs.nix
    ../modules/gate/needs.nix
    ../modules/library/needs.nix
    ../modules/llm/needs.nix
    ../modules/net/needs.nix
    ../modules/observe/needs.nix
    ../modules/photos/needs.nix
    ../modules/storage/needs.nix
    ../modules/vault/needs.nix
    ../modules/vms/needs.nix
  ];
  checked = lib.evalModules {
    modules = [
      { options.needs = lib.mkOption { type = import ./needs-type.nix { inherit lib; }; }; }
      { needs = lib.foldl' (a: f: a // import f) { } files; }
    ];
  };
  # services switched by the control plane are, until the fleet's owner
  # turns failover on (fleet/policy.json failover), where their roles put
  # them, as before: one switch for every box at once
  on = (lib.importJSON ../../fleet/policy.json).failover or false;
in
lib.mapAttrs (
  _: n:
  n
  // {
    placed = n.placed && on;
    lease = n.lease && on;
  }
) checked.config.needs
