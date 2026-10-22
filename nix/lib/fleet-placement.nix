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
      roles
      policy
      ;
    # each box with the roles it is built with (carried, below)
    list = lib.mapAttrs (n: b: b // { roles = carried.${n}; }) list;
  };
  # whether a box's own sops file holds a secret (its names are in the
  # clear there): what roles/_sops.nix dd.hasSecret says on the box
  hasSecret =
    box: name:
    let
      f = ../../secrets + "/${box}.yaml";
    in
    builtins.pathExists f && lib.hasInfix "\n${name}:" ("\n" + builtins.readFile f);
  # With failover (fleet/policy.json), what a box is built with comes from
  # its capability, never a hand-set list: every role of which it can run
  # a service that does not run everywhere (by its own report, its owner's
  # box.json and the policy: capability.nix), whose secrets its own file
  # holds (roles.json secrets: what its modules read whatever else), and
  # whose companions (roles.json with) it is built with too. core always.
  # The control plane decides where each service runs (box/placed.nix).
  able =
    box: r:
    r == "core"
    || builtins.any (s: needs ? ${s} && !needs.${s}.everyBox && capability.boxes.${box}.can ? ${s}) (
      roles.${r}.services or [ ]
    );
  buildable = box: r: able box r && lib.all (hasSecret box) (roles.${r}.secrets or [ ]);
  # drop what lacks a companion, until nothing more drops
  withCompanions =
    rs:
    let
      kept = lib.filter (r: lib.all (w: builtins.elem w rs) (roles.${r}."with" or [ ])) rs;
    in
    if kept == rs then rs else withCompanions kept;
  fromCapability = box: withCompanions (lib.filter (buildable box) (builtins.attrNames roles));
  # (in fleet/boxes.json's order where it still lists them: the order the
  # modules are imported in, which lists like home's tiles keep)
  carried = lib.mapAttrs (
    box: b:
    if policy.failover or false then
      let
        rs = fromCapability box;
      in
      lib.filter (r: builtins.elem r rs) (b.roles or [ ])
      ++ lib.filter (r: !builtins.elem r (b.roles or [ ])) rs
    else
      b.roles
  ) list;
  # the placed services a box carries: of those it is built with, each it
  # is able to run (capability); the plan gives it only these
  carries = lib.mapAttrs (
    box: _:
    lib.filter (
      s: needs ? ${s} && (needs.${s}.placed || needs.${s}.lease) && capability.boxes.${box}.can ? ${s}
    ) built.${box}
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
    carries
    built
    builtWith
    hasSecret
    ;
}
