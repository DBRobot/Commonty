# Which runner boxes can take each of the runner's variants (its needs,
# modules/forge/needs.nix): a box fits when its reported memory holds the
# runner's own use and what the variant holds besides. CI reads this to
# send a job where it fits (.forgejo/workflows/entry_point.yml).
{ lib }:
let
  needs = import ./needs.nix { inherit lib; };
  # the boxes built with the runner (fleet-placement.nix built)
  fp = import ./fleet-placement.nix { inherit lib; };
  runner = needs.forgejo-runner;
  bytes =
    s:
    let
      m = builtins.match "([0-9.]+)([KMGT]?)" s;
      unit = {
        "" = 1;
        K = 1024;
        M = 1024 * 1024;
        G = 1024 * 1024 * 1024;
        T = 1024 * 1024 * 1024 * 1024;
      };
    in
    builtins.fromJSON (lib.elemAt m 0) * unit.${lib.elemAt m 1};
  runners = lib.sort (a: b: a < b) (fp.builtWith "forgejo-runner");
  memoryOf = name: (lib.importJSON ../../fleet/boxes/${name}/hardware.json).memoryBytes;
  holds =
    v: name:
    let
      fixed = v.requires.memory.fixed or null;
    in
    fixed == null || memoryOf name >= bytes runner.base.uses.memory + bytes fixed;
in
lib.mapAttrs (_: v: lib.filter (holds v) runners) runner.variants
