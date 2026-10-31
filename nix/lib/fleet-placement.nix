# The fleet's own capability table and placement inputs, from its files:
# the needs, fleet/boxes.json, each box's signed report and box.json, the
# module bundles and the policy. One place for the check
# (tests/placement.nix) and the release (placement-inputs) to read.
#
# What a box is built with comes from its capability, never a list kept by
# hand: every bundle of modules (nix/roles/bundles.json) of which it can
# run a service that does not run everywhere (capability.nix: its own
# report, its owner's box.json and the policy), whose secrets its own sops
# file holds (bundles.json secrets: what the modules read whatever else),
# and whose companions (with) it is built with too; core always. The
# control plane decides where each placed service runs (box/placed.nix).
{ lib }:
let
  needs = import ./needs.nix { inherit lib; };
  list = lib.importJSON ../../fleet/boxes.json;
  bundles = lib.filterAttrs (_: v: builtins.isAttrs v) (lib.importJSON ../roles/bundles.json);
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
      policy
      built
      homes
      ;
    inherit list;
  };
  # the boxes with a store of their own: every one with a secrets key
  # (box/host.nix)
  stores = lib.filter (
    n: ((lib.importJSON ../../fleet/boxes/${n}/hardware.json).secretsKey or null) != null
  ) (builtins.attrNames list);
  # a shared bucket (needs shared: one copy, which every box writes) is kept
  # in one store, its home: the first store, by name, of a box built with
  # its service and able to run it. The plan keeps it there, wherever the
  # service runs (place.Bucket Box); none such, none
  homes = lib.filterAttrs (_: h: h != null) (
    lib.foldl' (a: b: a // b) { } (
      lib.mapAttrsToList (
        s: n:
        lib.mapAttrs (
          _: _: lib.findFirst (x: builtins.elem s built.${x} && capability.boxes.${x}.can ? ${s}) null stores
        ) (lib.filterAttrs (_: b: b.shared) n.buckets)
      ) needs
    )
  );
  # whether a box's own sops file holds a secret (its names are in the
  # clear there): what roles/_sops.nix dd.hasSecret says on the box
  hasSecret =
    box: name:
    let
      f = ../../secrets + "/${box}.yaml";
    in
    builtins.pathExists f && lib.hasInfix "\n${name}:" ("\n" + builtins.readFile f);
  # the order the bundles' modules are imported in, which lists like home's
  # tiles keep; one not named here comes after, by name
  order = [
    "core"
    "gateway"
    "cache-door"
    "photos"
    "media"
    "llm"
    "game"
    "forge"
    "observe"
    "runner"
    "library"
    "vault"
    "adblock"
    "compute"
  ];
  ordered = rs: lib.filter (r: builtins.elem r rs) order ++ lib.filter (r: !builtins.elem r order) rs;
  # the bundles for a box, from what it can run (a capability row's can)
  # and what its own file holds; `allowed`: of its services, which the box
  # may be given at all (a joined box: only its tier's)
  bundlesFor =
    {
      can,
      has,
      allowed ? (_: true),
    }:
    let
      able =
        r:
        r == "core"
        || builtins.any (s: needs ? ${s} && !needs.${s}.everyBox && can ? ${s} && allowed s) (
          bundles.${r}.services or [ ]
        );
      buildable = r: able r && lib.all has (bundles.${r}.secrets or [ ]);
      # drop what lacks a companion, until nothing more drops
      withCompanions =
        rs:
        let
          kept = lib.filter (r: lib.all (w: builtins.elem w rs) (bundles.${r}."with" or [ ])) rs;
        in
        if kept == rs then rs else withCompanions kept;
    in
    ordered (withCompanions (lib.filter buildable (builtins.attrNames bundles)));
  servicesOf = rs: lib.unique (lib.concatMap (r: bundles.${r}.services or [ ]) rs);
  able = lib.mapAttrs (
    box: _:
    bundlesFor {
      inherit (capability.boxes.${box}) can;
      has = hasSecret box;
    }
  ) list;
  # a bundle built on one box alone (bundles.json one: the libraries, whose
  # buckets have one writer and no lease): the first store, by name, able
  # to build it
  oneBox = r: lib.findFirst (x: builtins.elem r able.${x}) null stores;
  carried = lib.mapAttrs (
    box: rs: lib.filter (r: !(bundles.${r}.one or false) || oneBox r == box) rs
  ) able;
  # the services each box is built with: what the rest of the release picks
  # a box by (the storage pool, the mail boxes, the ports a member's device
  # may reach, CI's builders), never a name given by hand
  built = lib.mapAttrs (_: servicesOf) carried;
  # the placed services a box carries: of those it is built with, each it
  # is able to run (capability); the plan gives it only these
  carriesOf =
    can: services:
    lib.filter (s: needs ? ${s} && (needs.${s}.placed || needs.${s}.lease) && can ? ${s}) services;
  carries = lib.mapAttrs (box: _: carriesOf capability.boxes.${box}.can built.${box}) list;
  # the boxes built with service s, by name
  builtWith = s: builtins.filter (n: builtins.elem s built.${n}) (builtins.attrNames list);
  # A box that joined on its own key, built from its own report and box.json
  # (its dir), with no secrets file: the bundles of the services its tier
  # allows (placement-inputs joined) that it can run, and nothing else. It
  # carries those; the plan gives it one only once joined boxes are attested
  joined =
    { dir, name }:
    let
      tier = s: builtins.elem s (inputs.joined.services ++ inputs.joined.named ++ inputs.joined.own);
      cap =
        (import ./capability.nix {
          inherit lib needs policy;
          boxes.${name} = {
            hw = lib.importJSON (dir + "/hardware.json");
            own = lib.importJSON (dir + "/box.json");
            site = "joined";
            region = "joined";
          };
        }).boxes.${name};
      # never a bundle of one box: that is a release box's
      rs = lib.filter (r: !(bundles.${r}.one or false)) (bundlesFor {
        inherit (cap) can;
        has = _: false;
        allowed = tier;
      });
    in
    {
      bundles = rs;
      built = servicesOf rs;
      carries = lib.filter tier (carriesOf cap.can (servicesOf rs));
    };
in
{
  inherit
    needs
    list
    bundles
    capability
    inputs
    carried
    carries
    built
    builtWith
    hasSecret
    joined
    stores
    homes
    ;
}
