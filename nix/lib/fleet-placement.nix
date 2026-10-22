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
  # With failover (fleet/policy.json), a box carries every role whose
  # writers it is able to run, by its own report and the owner's rules
  # (capability), whatever fleet/boxes.json lists: a box that joins is ready
  # to take them over, and the control plane decides where each runs
  writersOf = r: lib.filter (s: needs ? ${s} && needs.${s}.lease) (roles.${r}.services or [ ]);
  ableTo = box: r: writersOf r != [ ] && lib.all (s: inputs.boxes.${box}.can ? ${s}) (writersOf r);
  carried = lib.mapAttrs (
    box: b:
    b.roles
    ++ lib.optionals (policy.failover or false) (
      lib.filter (r: !builtins.elem r b.roles && ableTo box r) (builtins.attrNames roles)
    )
  ) list;
  # the services each box is built with, by what it carries: what the rest
  # of the release picks a box by (the storage pool, the mail boxes, the
  # ports a member's device may reach), never a role's name
  built = lib.mapAttrs (_: rs: lib.unique (lib.concatMap (r: roles.${r}.services or [ ]) rs)) carried;
  # the boxes built with service s, by name
  builtWith = s: builtins.filter (n: builtins.elem s built.${n}) (builtins.attrNames list);
in
{
  inherit
    needs
    list
    roles
    capability
    inputs
    carried
    built
    builtWith
    ;
}
