# What every box can run (nix/lib/capability.nix), from the needs, its
# signed report and its owner's box.json, against fleet/boxes.json: every
# box joins (it can run what every box runs), every service it carries is
# one it can run, and nix/roles/bundles.json says truly what each bundle
# brings (a service with units is found on a box by them). Fails while the flake is
# evaluated, naming each problem; built, it is the whole table as JSON - what
# the control plane chooses from.
{
  pkgs,
  lib,
  self,
  ...
}:
let
  inherit (import ../lib/fleet-placement.nix { inherit lib; })
    needs
    list
    capability
    built
    carries
    ;
  every = lib.attrNames (lib.filterAttrs (_: n: n.everyBox) needs);
  # what a box is built with, from its capability (fleet-placement.nix built)
  brought = box: built.${box};
  claims = s: [ s ] ++ needs.${s}.units;
  matches = u: c: if lib.hasSuffix "*" c then lib.hasPrefix (lib.removeSuffix "*" c) u else c == u;
  running =
    box:
    let
      c = self.nixosConfigurations.${box}.config;
      units = builtins.attrNames c.systemd.services ++ builtins.attrNames c.systemd.timers;
    in
    lib.filter (s: builtins.any (u: builtins.any (matches u) (claims s)) units) (
      builtins.attrNames needs
    );
  # each box judges itself on the needs as JSON, the release's needs.json
  # (box/capability.nix): on the report the release has, it must come to
  # the release's own table
  judged = import ../lib/capability.nix {
    inherit lib;
    needs = builtins.fromJSON (builtins.toJSON needs);
    policy = lib.importJSON ../../fleet/policy.json;
    boxes = lib.mapAttrs (n: _: {
      hw = lib.importJSON ../../fleet/boxes/${n}/hardware.json;
      own = lib.importJSON ../../fleet/boxes/${n}/box.json;
    }) list;
  };
  scores = t: lib.mapAttrs (_: c: c.score) t.can;
  # a name follows one service (needs hosts), placed or under a lease: two
  # claiming one name, or one that nothing places, could not be followed
  named = lib.concatLists (lib.mapAttrsToList (s: n: map (h: { inherit s h; }) n.hosts) needs);
  hostProblems =
    lib.concatMap (
      h:
      let
        by = map (x: x.s) (lib.filter (x: x.h == h) named);
      in
      lib.optional (builtins.length by > 1)
        "${h}.<domain> is claimed by ${lib.concatStringsSep " and " by} (needs hosts): a name follows one service"
    ) (lib.unique (map (x: x.h) named))
    ++ map (x: "${x.s}: its hosts follow nothing, as it is neither placed nor under a lease") (
      lib.filter (x: (policyOn) && !(needs.${x.s}.placed || needs.${x.s}.lease)) named
    )
    # one box to point at, and what answers under it goes there too
    ++ map (s: "${s}: a name follows one box, so it runs once (instances = 1)") (
      lib.unique (map (x: x.s) (lib.filter (x: needs.${x.s}.instances != 1) named))
    );
  policyOn = (lib.importJSON ../../fleet/policy.json).failover or false;
  problems =
    hostProblems
    ++ map (b: "${b}: judged on the box, its capability is not the release's") (
      lib.filter (b: scores judged.boxes.${b} != scores capability.boxes.${b}) (builtins.attrNames list)
    )
    ++ lib.concatMap (
      box:
      let
        t = capability.boxes.${box};
      in
      map (w: "${box}: cannot join the fleet: ${w}") t.why
      ++ lib.concatMap (
        s: map (w: "${box}: it carries ${s}, which it cannot run: ${w}") (t.cannot.${s} or [ ])
      ) carries.${box}
      ++ map (
        s: "${box}: runs ${s}, which none of its bundles brings (nix/roles/bundles.json services)"
      ) (lib.filter (s: !(builtins.elem s (brought box ++ every))) (running box))
    ) (builtins.attrNames list);
in
if problems == [ ] then
  pkgs.writeText "placement.json" (builtins.toJSON capability)
else
  throw ("\n" + lib.concatStringsSep "\n" problems)
