# fleet/boxes.json against the needs (nix/lib/place.nix): every box can
# carry what its roles bring, by what it reported of itself and what its
# owner said, with room for it; and fleet/roles.json says truly what each
# role brings - every service a box runs comes with one of its roles (a
# service with units is found by them, nix/lib/needs-type.nix `units`).
# Fails while the flake is evaluated, naming each problem.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  place = import ../lib/place.nix { inherit lib; };
  needs = import ../lib/needs.nix { inherit lib; };
  claims = s: [ s ] ++ needs.${s}.units;
  matches = u: c: if lib.hasSuffix "*" c then lib.hasPrefix (lib.removeSuffix "*" c) u else c == u;
  # the services a box really runs: those with a unit there
  running =
    box:
    let
      c = self.nixosConfigurations.${box}.config;
      units = builtins.attrNames c.systemd.services ++ builtins.attrNames c.systemd.timers;
    in
    lib.filter (s: builtins.any (u: builtins.any (matches u) (claims s)) units) (
      builtins.attrNames needs
    );
  unlisted = lib.concatMap (
    box:
    map (s: "${box}: runs ${s}, which none of its roles brings (fleet/roles.json services)") (
      lib.filter (s: !(builtins.elem s place.boxes.${box}.services)) (running box)
    )
  ) (builtins.attrNames place.boxes);
  problems = place.problems ++ unlisted;
in
if problems == [ ] then
  pkgs.writeText "placement.json" (builtins.toJSON place)
else
  throw ("\n" + lib.concatStringsSep "\n" problems)
