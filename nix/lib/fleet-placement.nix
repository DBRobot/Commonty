# The fleet's own capability table and placement inputs, from its files:
# the needs, fleet/boxes.json, each box's signed report and box.json, the
# roles and the policy. One place for the check (tests/placement.nix) and
# the release (placement-inputs) to read.
{ lib }:
let
  needs = import ./needs.nix { inherit lib; };
  list = lib.importJSON ../../fleet/boxes.json;
  roles = lib.filterAttrs (_: v: builtins.isAttrs v) (lib.importJSON ../../fleet/roles.json);
  policy = lib.importJSON ../../fleet/policy.json;
  capability = import ./capability.nix {
    inherit lib needs policy;
    boxes = lib.mapAttrs (n: b: {
      hw = lib.importJSON ../../fleet/boxes/${n}/hardware.json;
      own = lib.importJSON ../../fleet/boxes/${n}/box.json;
      site = b.siteId or n;
      region = b.regionId or n;
    }) list;
  };
in
{
  inherit
    needs
    list
    roles
    capability
    ;
  inputs = import ./placement-inputs.nix {
    inherit
      lib
      needs
      capability
      list
      roles
      policy
      ;
  };
}
